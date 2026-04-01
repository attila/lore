// SPDX-License-Identifier: MIT OR Apache-2.0

use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};

use lore::config::{Config, default_config_path, default_database_path};
use lore::database::KnowledgeDB;
use lore::embeddings::{Embedder, OllamaClient};
use lore::{ingest, provision, server};

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
    Ingest,

    /// Start the MCP server (stdio transport for Claude Code)
    Serve,

    /// Search the knowledge base from the command line
    Search {
        /// Search query
        query: Vec<String>,
    },

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
        Commands::Ingest => cmd_ingest(&config_path),
        Commands::Serve => cmd_serve(&config_path),
        Commands::Search { query } => cmd_search(&config_path, &query.join(" ")),
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

    let ingest_result = ingest::ingest(
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

    // MCP setup instructions
    eprintln!("\nTo use with Claude Code, add this to your MCP config:\n");
    eprintln!("  {{");
    eprintln!("    \"mcpServers\": {{");
    eprintln!("      \"lore\": {{");
    eprintln!("        \"command\": \"lore\",");
    if user_provided_config {
        eprintln!(
            "        \"args\": [\"serve\", \"--config\", \"{}\"]",
            config_path.display()
        );
    } else {
        eprintln!("        \"args\": [\"serve\"]");
    }
    eprintln!("      }}");
    eprintln!("    }}");
    eprintln!("  }}");

    eprintln!("\nOr run:\n");
    if user_provided_config {
        eprintln!("  claude mcp add --scope user --transport stdio lore -- \\",);
        eprintln!("    lore serve --config {}", config_path.display());
    } else {
        eprintln!("  claude mcp add --scope user --transport stdio lore -- lore serve");
    }

    Ok(())
}

fn cmd_ingest(config_path: &Path) -> anyhow::Result<()> {
    let config = Config::load(config_path)?;
    let ollama = OllamaClient::new(&config.ollama.host, &config.ollama.model);
    let db = KnowledgeDB::open(&config.database, ollama.dimensions())?;
    db.init()?;

    eprintln!("Ingesting knowledge base...\n");

    let result = ingest::ingest(
        &db,
        &ollama,
        &config.knowledge_dir,
        &config.chunking.strategy,
        &|msg| {
            eprintln!("{msg}");
        },
    );

    eprintln!(
        "\nDone: {} files → {} chunks",
        result.files_processed, result.chunks_created
    );
    if !result.errors.is_empty() {
        eprintln!("Errors: {}", result.errors.len());
        for err in &result.errors {
            eprintln!("  ✗ {err}");
        }
    }

    Ok(())
}

fn cmd_serve(config_path: &Path) -> anyhow::Result<()> {
    let config = Config::load(config_path)?;
    let ollama = OllamaClient::new(&config.ollama.host, &config.ollama.model);
    server::start_mcp_server(&config, &ollama)
}

fn cmd_search(config_path: &Path, query: &str) -> anyhow::Result<()> {
    if query.is_empty() {
        anyhow::bail!("Usage: lore search <query>");
    }

    let config = Config::load(config_path)?;
    let ollama = OllamaClient::new(&config.ollama.host, &config.ollama.model);
    let db = KnowledgeDB::open(&config.database, ollama.dimensions())?;
    db.init()?;

    let query_embedding = if config.search.hybrid {
        ollama.embed(query).ok()
    } else {
        None
    };

    let results = db.search_hybrid(query, query_embedding.as_deref(), config.search.top_k)?;

    if results.is_empty() {
        eprintln!("No results found.");
    } else {
        for (i, r) in results.iter().enumerate() {
            eprintln!("\n{}", "─".repeat(60));
            eprintln!("[{}] {}", i + 1, r.title);
            eprintln!("    source: {}", r.source_file);
            if !r.heading_path.is_empty() {
                eprintln!("    path:   {}", r.heading_path);
            }
            if !r.tags.is_empty() {
                eprintln!("    tags:   {}", r.tags);
            }
            eprintln!("    score:  {:.4}", r.score);
            eprintln!();
            let body = if r.body.len() > 500 {
                let truncate_at = floor_char_boundary(&r.body, 500);
                format!("{}...", &r.body[..truncate_at])
            } else {
                r.body.clone()
            };
            eprintln!("{body}");
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
    }

    Ok(())
}
