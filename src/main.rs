// SPDX-License-Identifier: MIT OR Apache-2.0

use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};

use lore::config::{Config, default_config_path, default_database_path};
use lore::database::KnowledgeDB;
use lore::embeddings::{Embedder, OllamaClient};
use lore::hook;
use lore::lockfile::{WriteLock, lock_path_for};
use lore::lore_debug;
use lore::{git, ingest, provision, server};

#[derive(Parser)]
#[command(
    name = "lore",
    about = "Local semantic search for your software patterns",
    version
)]
struct Cli {
    /// Path to config file [default: ~/.config/lore/lore.toml]
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Output results as JSON (for search and list commands)
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Provision Ollama, pull model, create config, run first ingestion
    Init {
        /// Path to your markdown knowledge base (git repo)
        #[arg(long)]
        repo: PathBuf,

        /// Ollama embedding model to use
        #[arg(long, default_value = "nomic-embed-text")]
        model: String,

        /// MCP server bind address
        #[arg(long, default_value = "localhost:3100")]
        bind: String,

        /// Path to database file [default: ~/.local/share/lore/knowledge.db]
        #[arg(long)]
        database: Option<PathBuf>,
    },

    /// Re-index the knowledge base from markdown files
    #[command(after_help = "EXAMPLES:
  lore ingest                          Delta ingest (only changed files since the last commit)
  lore ingest --force                  Full re-index of the whole knowledge base
  lore ingest --file patterns/foo.md   Index one file without a git commit
  lore ingest --file patterns/foo.md --force
                                       Index one file, overriding .loreignore

EXIT CODES:
  0  Success. Delta and full ingest may list per-file errors on stderr and still exit 0.
  1  Single-file ingest failed (atomic), or an unrecoverable error (config, database,
     embedding service). Relative paths are resolved against the current working directory,
     not the knowledge directory.

NOTES:
  - `--file` takes one path per invocation; globs are not expanded.
  - Single-file ingest does not update delta-ingest state, so a subsequent
    `lore ingest` still sees real git changes.")]
    Ingest {
        /// Force a full re-index instead of delta. When combined with
        /// `--file`, overrides `.loreignore` for that single file.
        #[arg(long)]
        force: bool,

        /// Index a single markdown file without requiring a git commit.
        /// Respects `.loreignore` unless `--force` is also passed. Does not
        /// touch delta-ingest state, so the next `lore ingest` still sees
        /// real git changes. Relative paths are resolved against the current
        /// working directory, not the knowledge directory.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
    },

    /// Start the MCP server (stdio transport for Claude Code)
    Serve,

    /// Search the knowledge base from the command line
    Search {
        /// Search query
        query: Vec<String>,

        /// Override the number of results to return
        #[arg(long)]
        top_k: Option<usize>,
    },

    /// Process a Claude Code lifecycle hook (reads JSON from stdin)
    Hook,

    /// Simulate hook query extraction for a synthetic tool call.
    ///
    /// Reads a JSON object with `tool_name` and `tool_input` from stdin and
    /// prints the FTS5 query the hook would derive for it (or nothing if no
    /// meaningful terms survive). Used by the coverage-check skill to
    /// materialize production-realistic queries from inferred tool calls.
    #[command(after_help = "EXAMPLES:
  echo '{\"tool_name\":\"Edit\",\"tool_input\":{\"file_path\":\"src/lib.rs\"}}' \\
    | lore extract-queries

  echo '{\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"cargo deny check\"}}' \\
    | lore extract-queries

EXIT CODES:
  0  Success. Stdout carries the query, or is empty when no terms survived.
  1  Malformed JSON on stdin.")]
    ExtractQueries,

    /// List all patterns in the knowledge base
    List,

    /// Check health of all components
    Status,
}

fn main() {
    let cli = Cli::parse();

    let user_provided_config = cli.config.is_some();
    let config_path = match cli.config {
        Some(p) => std::path::absolute(p).map_err(anyhow::Error::from),
        None => default_config_path(),
    };

    let config_path = match config_path {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    let json = cli.json;

    let result = match cli.command {
        Commands::Init {
            repo,
            model,
            bind,
            database,
        } => cmd_init(
            &config_path,
            user_provided_config,
            database.as_deref(),
            &repo,
            &model,
            &bind,
        ),
        Commands::Ingest { force, file } => cmd_ingest(&config_path, force, file.as_deref()),
        Commands::Serve => cmd_serve(&config_path),
        Commands::Search { query, top_k } => {
            cmd_search(&config_path, &query.join(" "), top_k, json)
        }
        Commands::Hook => cmd_hook(&config_path),
        Commands::ExtractQueries => cmd_extract_queries(),
        Commands::List => cmd_list(&config_path, json),
        Commands::Status => cmd_status(&config_path),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}

fn cmd_init(
    config_path: &Path,
    user_provided_config: bool,
    database_override: Option<&Path>,
    repo: &Path,
    model: &str,
    bind: &str,
) -> anyhow::Result<()> {
    eprintln!("=== lore init ===\n");

    let knowledge_dir = std::fs::canonicalize(repo)
        .map_err(|_| anyhow::anyhow!("Directory not found: {}", repo.display()))?;

    if !knowledge_dir.is_dir() {
        anyhow::bail!("{} is not a directory", knowledge_dir.display());
    }

    if !git::is_git_repo(&knowledge_dir) {
        eprintln!("Note: {} is not a git repository.", knowledge_dir.display());
        eprintln!("  Lore will work, but delta ingest, the inbox branch workflow, and version");
        eprintln!("  history will be unavailable. Run `git init` in this directory to enable");
        eprintln!("  them. See docs/configuration.md#git-integration for details.\n");
    }

    let db_path = match database_override {
        Some(p) => std::path::absolute(p)?,
        None => default_database_path()?,
    };

    // Create parent directories for config and database
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut config = Config::default_with(knowledge_dir, db_path, model);
    config.bind = bind.to_string();

    config.save(config_path)?;
    // Canonicalize after save so the output path is clean (no ".." hops).
    let config_path =
        std::fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
    eprintln!("Config written to {}\n", config_path.display());

    // provision
    eprintln!("--- Provisioning ---\n");
    let result = provision::provision(&config.ollama.host, &config.ollama.model, &|msg| {
        eprintln!("{msg}");
    });

    if !result.errors.is_empty() {
        eprintln!("\nProvisioning errors:");
        for err in &result.errors {
            eprintln!("  ✗ {err}");
        }
        process::exit(1);
    }

    if !result.actions.is_empty() {
        eprintln!("\nActions taken:");
        for action in &result.actions {
            eprintln!("  ✓ {action}");
        }
    }

    // run initial ingestion
    eprintln!("\n--- Ingesting knowledge base ---\n");

    let ollama = OllamaClient::new(&config.ollama.host, &config.ollama.model);
    let db = KnowledgeDB::open(&config.database, ollama.dimensions())?;
    db.init()?;

    eprintln!("Search mode: hybrid (FTS5 + vector)\n");

    let mut write_lock = WriteLock::open(&lock_path_for(&config.database))?;
    let _lock_guard = write_lock.acquire()?;

    let ingest_result = ingest::full_ingest(
        &db,
        &ollama,
        &config.knowledge_dir,
        &config.chunking.strategy,
        &|msg| {
            eprintln!("{msg}");
        },
    );

    eprintln!("\n--- Done ---");
    eprintln!("  Files: {}", ingest_result.files_processed);
    eprintln!("  Chunks: {}", ingest_result.chunks_created);
    if !ingest_result.errors.is_empty() {
        eprintln!("  Errors: {}", ingest_result.errors.len());
    }

    // Claude Code plugin instructions
    eprintln!("\nTo use with Claude Code, install the lore plugin:\n");
    eprintln!("  claude --plugin-dir <lore-repo>/integrations/claude-code/\n");
    eprintln!("Replace <lore-repo> with the path to your lore source checkout.");
    eprintln!("This includes the MCP server, lifecycle hooks, and the /search skill.");

    if user_provided_config {
        eprintln!();
        eprintln!(
            "Note: you are using a custom config at {}",
            config_path.display()
        );
        eprintln!("The plugin's mcp.json uses the default config. Either:");
        eprintln!(
            "  1. Edit integrations/claude-code/mcp.json to add: \"--config\", \"{}\"",
            config_path.display()
        );
        eprintln!("  2. Or add the MCP server manually (without hooks or skills):\n");
        eprintln!("     claude mcp add --scope user --transport stdio lore -- \\");
        eprintln!("       lore serve --config {}", config_path.display());
    }

    Ok(())
}

fn cmd_ingest(config_path: &Path, force: bool, file: Option<&Path>) -> anyhow::Result<()> {
    let config = Config::load(config_path)?;
    let mode_label = match (file.is_some(), force) {
        (true, true) => "single-file (force override .loreignore)",
        (true, false) => "single-file",
        (false, true) => "full",
        (false, false) => "delta",
    };
    lore_debug!(
        "ingest: dir={} mode={} strategy={}",
        config.knowledge_dir.display(),
        mode_label,
        config.chunking.strategy,
    );

    let ollama = OllamaClient::new(&config.ollama.host, &config.ollama.model);
    let db = KnowledgeDB::open(&config.database, ollama.dimensions())?;
    db.init()?;

    let on_progress = &|msg: &str| {
        eprintln!("{msg}");
    };

    let mut write_lock = WriteLock::open(&lock_path_for(&config.database))?;
    let _lock_guard = write_lock.acquire()?;

    let result = dispatch_ingest(&db, &ollama, &config, force, file, on_progress)?;

    lore_debug!(
        "ingest: processed={} chunks_created={} errors={}",
        result.files_processed,
        result.chunks_created,
        result.errors.len(),
    );

    print_ingest_summary(&result);

    if !result.errors.is_empty() {
        eprintln!("Errors: {}", result.errors.len());
        for err in &result.errors {
            eprintln!("  ✗ {err}");
        }
        // Single-file ingest is atomic: any error means the one requested
        // file did not land, so propagate as a non-zero exit. Delta and
        // full ingests may collect per-file errors while still making
        // progress on the rest, so their behaviour is unchanged.
        if matches!(result.mode, ingest::IngestMode::SingleFile { .. }) {
            anyhow::bail!("single-file ingest failed");
        }
    }

    Ok(())
}

/// Route the ingest request to the right library entry point based on the
/// flags passed to `cmd_ingest`.
fn dispatch_ingest(
    db: &KnowledgeDB,
    ollama: &OllamaClient,
    config: &Config,
    force: bool,
    file: Option<&Path>,
    on_progress: &dyn Fn(&str),
) -> anyhow::Result<ingest::IngestResult> {
    if let Some(path) = file {
        // Resolve relative paths against the current working directory so
        // `lore ingest --file ./patterns/foo.md` works from anywhere.
        // ingest_single_file canonicalises again internally, enforces the
        // knowledge-directory containment check, and emits its own
        // "Single-file ingest: {rel_path}" progress line via on_progress —
        // so no banner is needed here.
        let resolved = std::path::absolute(path)
            .map_err(|e| anyhow::anyhow!("Cannot resolve path {}: {e}", path.display()))?;
        return Ok(ingest::ingest_single_file(
            db,
            ollama,
            &config.knowledge_dir,
            &resolved,
            &config.chunking.strategy,
            force,
            on_progress,
        ));
    }

    if force {
        eprintln!("Full ingest (--force)...\n");
        Ok(ingest::full_ingest(
            db,
            ollama,
            &config.knowledge_dir,
            &config.chunking.strategy,
            on_progress,
        ))
    } else {
        eprintln!("Ingesting knowledge base...\n");
        Ok(ingest::ingest(
            db,
            ollama,
            &config.knowledge_dir,
            &config.chunking.strategy,
            on_progress,
        ))
    }
}

/// Print the one-line summary for a completed ingest, shaped by mode.
fn print_ingest_summary(result: &ingest::IngestResult) {
    match &result.mode {
        ingest::IngestMode::Full => {
            eprintln!(
                "\nDone (full): {} files → {} chunks",
                result.files_processed, result.chunks_created
            );
        }
        ingest::IngestMode::SingleFile { path } => {
            // Skip the "Done" line when the single-file ingest failed —
            // otherwise stderr reads `Done (single-file): … → 0 chunks`
            // immediately followed by the error list, which is contradictory.
            // The error loop below still prints the details, and cmd_ingest
            // bails with exit 1, so silence here is not silent.
            if result.errors.is_empty() {
                eprintln!(
                    "\nDone (single-file): {path} → {chunks} chunks",
                    chunks = result.chunks_created
                );
            }
        }
        ingest::IngestMode::Delta { unchanged } => {
            lore_debug!("ingest delta: unchanged={unchanged}");
            let removed = result.reconciled_removed;
            let added = result.reconciled_added;
            let processed = result.files_processed;
            let reconcile_summary = match (removed, added) {
                (0, 0) => String::new(),
                (r, 0) => format!("{r} reconciled (removed)"),
                (0, a) => format!("{a} reconciled (re-indexed)"),
                (r, a) => format!("{r} reconciled (removed), {a} reconciled (re-indexed)"),
            };

            match (processed, reconcile_summary.is_empty()) {
                (0, true) => eprintln!("\nAlready up to date."),
                (0, false) => {
                    eprintln!("\nDone (delta): {reconcile_summary}, no other changes");
                }
                (changed, true) => {
                    eprintln!("\nDone (delta): {changed} files changed, {unchanged} unchanged");
                }
                (changed, false) => eprintln!(
                    "\nDone (delta): {changed} files changed, {reconcile_summary}, {unchanged} unchanged"
                ),
            }
        }
    }

    print_universal_advisories(result);
}

/// Emit the always-on `Universal patterns: N` summary line plus the three
/// optional advisories (>3 patterns, oversized body, near-miss spelling).
///
/// Suppressed for single-file ingest failures so stderr does not interleave
/// "Universal patterns: 0" with the error list.
fn print_universal_advisories(result: &ingest::IngestResult) {
    if matches!(result.mode, ingest::IngestMode::SingleFile { .. }) && !result.errors.is_empty() {
        return;
    }

    eprintln!("Universal patterns: {}", result.universal_sources.len());

    if result.universal_sources.len() > 3 {
        eprintln!(
            "Note: {} patterns tagged `universal`. Consider whether all of them need always-on visibility:",
            result.universal_sources.len()
        );
        for source in &result.universal_sources {
            eprintln!("  - {source}");
        }
    }

    for source in &result.oversized_universal_bodies {
        eprintln!(
            "Note: universal pattern `{source}` has a body larger than 1KB. \
             Universal patterns re-inject on every relevant tool call; consider trimming."
        );
    }

    for entry in &result.near_miss_universal_tags {
        eprintln!(
            "Note: tag `{entry}` looks like a misspelling of `universal` (case-sensitive exact match required)."
        );
    }
}

fn cmd_serve(config_path: &Path) -> anyhow::Result<()> {
    let config = Config::load(config_path)?;
    let ollama = OllamaClient::new(&config.ollama.host, &config.ollama.model);
    server::start_mcp_server(&config, &ollama)
}

fn cmd_search(
    config_path: &Path,
    query: &str,
    top_k: Option<usize>,
    json: bool,
) -> anyhow::Result<()> {
    if query.is_empty() {
        anyhow::bail!("Usage: lore search <query>");
    }

    let mut config = Config::load(config_path)?;
    if let Some(k) = top_k {
        config.search.top_k = k;
    }

    lore_debug!(
        "search: query={query:?} top_k={} hybrid={} min_relevance={:.4}",
        config.search.top_k,
        config.search.hybrid,
        config.search.min_relevance,
    );

    let ollama = OllamaClient::new(&config.ollama.host, &config.ollama.model);
    let db = KnowledgeDB::open(&config.database, ollama.dimensions())?;
    db.init()?;

    let results = hook::search_with_threshold(&db, &ollama, &config, query)?;
    lore_debug!("search: {} results", results.len());

    if json {
        println!("{}", serde_json::to_string(&results)?);
    } else if results.is_empty() {
        eprintln!("No results found.");
    } else {
        for (i, r) in results.iter().enumerate() {
            println!("\n{}", "─".repeat(60));
            println!("[{}] {}", i + 1, r.title);
            println!("    source: {}", r.source_file);
            if !r.heading_path.is_empty() {
                println!("    path:   {}", r.heading_path);
            }
            if !r.tags.is_empty() {
                println!("    tags:   {}", r.tags);
            }
            println!("    score:  {:.4}", r.score);
            println!();
            let body = if r.body.len() > 500 {
                let truncate_at = floor_char_boundary(&r.body, 500);
                format!("{}...", &r.body[..truncate_at])
            } else {
                r.body.clone()
            };
            println!("{body}");
        }
    }

    Ok(())
}

/// Find the largest byte index at or before `index` that is a valid UTF-8
/// char boundary. Equivalent to `str::floor_char_boundary` (stabilized in
/// Rust 1.86) but works on Rust 1.85.
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Process a Claude Code lifecycle hook.
///
/// **Error handling contract**: Unlike all other `cmd_*` functions, this one
/// catches ALL errors and exits 0 regardless. Hooks must never break Claude
/// Code. On any error, a diagnostic is logged to stderr and no stdout output
/// is produced.
#[allow(clippy::unnecessary_wraps)]
fn cmd_hook(config_path: &Path) -> anyhow::Result<()> {
    if let Err(e) = cmd_hook_inner(config_path) {
        eprintln!("lore hook: {e}");
        lore_debug!("hook pipeline error (swallowed): {e:#}");
    }
    Ok(())
}

fn cmd_hook_inner(config_path: &Path) -> anyhow::Result<()> {
    let input = hook::read_input()?;
    lore_debug!(
        "hook stdin: event={} tool={}",
        input.hook_event_name,
        input.tool_name.as_deref().unwrap_or("none"),
    );

    let config = Config::load(config_path)?;
    let ollama = OllamaClient::new(&config.ollama.host, &config.ollama.model);
    let db = KnowledgeDB::open(&config.database, ollama.dimensions())?;
    db.init()?;

    if let Some(output) = hook::handle_hook(&input, &db, &ollama, &config)? {
        let json = serde_json::to_string(&output)?;
        println!("{json}");
    }

    Ok(())
}

/// Simulate the hook's FTS5 query extraction for a synthetic tool call.
///
/// Reads a thin JSON envelope (`tool_name` + `tool_input`) from stdin, wraps
/// it into a `PreToolUse` `HookInput`, and runs the same `extract_query`
/// logic the hook uses. Prints the resulting query to stdout (no trailing
/// output when extraction yields nothing).
fn cmd_extract_queries() -> anyhow::Result<()> {
    use std::io::Read;

    #[derive(serde::Deserialize)]
    struct Envelope {
        tool_name: Option<String>,
        tool_input: Option<serde_json::Value>,
    }

    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| anyhow::anyhow!("failed to read stdin: {e}"))?;

    let envelope: Envelope =
        serde_json::from_str(&buf).map_err(|e| anyhow::anyhow!("invalid JSON on stdin: {e}"))?;

    let input = hook::HookInput {
        hook_event_name: "PreToolUse".to_string(),
        session_id: None,
        tool_name: envelope.tool_name,
        tool_input: envelope.tool_input,
        agent_type: None,
        transcript_path: None,
        tool_response: None,
    };

    if let Some(query) = hook::extract_query(&input) {
        println!("{query}");
    }
    Ok(())
}

fn cmd_list(config_path: &Path, json: bool) -> anyhow::Result<()> {
    let config = Config::load(config_path)?;
    let ollama = OllamaClient::new(&config.ollama.host, &config.ollama.model);
    let db = KnowledgeDB::open(&config.database, ollama.dimensions())?;
    db.init()?;

    let patterns = db.list_patterns()?;

    if json {
        println!("{}", serde_json::to_string(&patterns)?);
    } else {
        for p in &patterns {
            if p.tags.is_empty() {
                println!("{}", p.title);
            } else {
                println!("{} [{}]", p.title, p.tags);
            }
        }
    }

    Ok(())
}

#[allow(clippy::unnecessary_wraps)]
fn cmd_status(config_path: &Path) -> anyhow::Result<()> {
    let Ok(config) = Config::load(config_path) else {
        eprintln!("✗ No config found. Run 'lore init' first.");
        process::exit(1);
    };

    let status = provision::check_status(&config.ollama.host, &config.ollama.model);

    eprintln!("=== lore status ===\n");
    eprintln!("  Config:       {}", config_path.display());
    eprintln!("  Knowledge:    {}", config.knowledge_dir.display());
    eprintln!("  Database:     {}", config.database.display());
    eprintln!("  Bind:         {}", config.bind);
    eprintln!();
    eprintln!(
        "  Ollama:       {}",
        if status.ollama_installed {
            "✓ installed"
        } else {
            "✗ not found"
        }
    );
    eprintln!(
        "  Ollama svc:   {}",
        if status.ollama_running {
            "✓ running"
        } else {
            "✗ not running"
        }
    );
    eprintln!(
        "  Model:        {} {}",
        if status.model_available { "✓" } else { "✗" },
        config.ollama.model
    );
    eprintln!("  sqlite-vec:   ✓ bundled");

    let ollama = OllamaClient::new(&config.ollama.host, &config.ollama.model);
    if let Ok(db) = KnowledgeDB::open(&config.database, ollama.dimensions())
        && db.init().is_ok()
        && let Ok(stats) = db.stats()
    {
        eprintln!();
        eprintln!("  Chunks:       {}", stats.chunks);
        eprintln!("  Sources:      {}", stats.sources);

        if let Ok(Some(sha)) = db.get_metadata("last_ingested_commit") {
            let short = git::short_sha(&config.knowledge_dir, &sha);
            eprintln!("  Last commit:  {short}");
        }
        eprintln!();
    }

    Ok(())
}
