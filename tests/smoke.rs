use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

use lore::config::Config;
use lore::database::KnowledgeDB;
use lore::embeddings::{FailingEmbedder, FakeEmbedder};
use lore::ingest;

/// Set up a temp directory with a config, database, and ingested test patterns.
///
/// Returns the config file path so callers can pass `--config <path>` to CLI
/// commands.
fn setup_populated_env(dir: &Path) -> std::path::PathBuf {
    let knowledge_dir = dir.join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    std::fs::write(
        knowledge_dir.join("rust-testing.md"),
        "# Rust Testing\n\n\
         tags: rust, testing\n\n\
         Prefer integration tests that exercise real dependencies over mocks.\n\
         Use deterministic fakes only for external services.\n",
    )
    .unwrap();

    std::fs::write(
        knowledge_dir.join("error-handling.md"),
        "# Error Handling\n\n\
         tags: rust, errors\n\n\
         Always use anyhow for application-level error propagation.\n",
    )
    .unwrap();

    let db_path = dir.join("knowledge.db");
    let config = Config::default_with(knowledge_dir.clone(), db_path, "nomic-embed-text");

    let config_path = dir.join("lore.toml");
    config.save(&config_path).unwrap();

    let embedder = FakeEmbedder::new();
    let db = KnowledgeDB::open(&config.database).unwrap();
    db.init().unwrap();
    db.ensure_vector_table(embedder.dimensions()).unwrap();

    ingest::ingest(&db, &embedder, &knowledge_dir, "heading", &|_| {});

    config_path
}

#[test]
fn help_exits_successfully() {
    Command::cargo_bin("lore")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("lore"));
}

#[test]
fn help_shows_all_subcommands() {
    let output = Command::cargo_bin("lore")
        .unwrap()
        .arg("--help")
        .assert()
        .success();

    output
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("ingest"))
        .stdout(predicate::str::contains("serve"))
        .stdout(predicate::str::contains("search"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("status"));
}

#[test]
fn version_shows_version() {
    Command::cargo_bin("lore")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("lore"));
}

#[test]
fn search_without_query_shows_error() {
    Command::cargo_bin("lore")
        .unwrap()
        .args(["search", "--config", "/tmp/nonexistent-lore.toml"])
        .assert()
        .failure();
}

#[test]
fn init_without_repo_shows_error() {
    Command::cargo_bin("lore")
        .unwrap()
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--repo"));
}

#[test]
fn no_knowledge_mcp_in_help_output() {
    Command::cargo_bin("lore")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("knowledge-mcp").not());
}

#[test]
fn init_help_shows_database_flag() {
    Command::cargo_bin("lore")
        .unwrap()
        .args(["init", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--database"));
}

#[test]
fn init_against_plain_directory_emits_git_advisory() {
    // The advisory is printed by `cmd_init` before Ollama provisioning, so we
    // can capture it from stderr regardless of whether the rest of `lore init`
    // succeeds (it will fail if Ollama is not running locally — that is fine).
    let tmp = tempfile::tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    let config_path = tmp.path().join("lore.toml");
    let db_path = tmp.path().join("knowledge.db");

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "init",
            "--repo",
            knowledge_dir.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
            "--database",
            db_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("is not a git repository"),
        "expected git advisory on stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("delta ingest"),
        "expected advisory to mention delta ingest, got: {stderr}"
    );
    assert!(
        stderr.contains("docs/configuration.md#git-integration"),
        "expected advisory to point at the documentation reference, got: {stderr}"
    );
}

#[test]
fn init_against_git_repo_does_not_emit_git_advisory() {
    // When the target is a git repository, the advisory must not appear.
    let tmp = tempfile::tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    // Initialise an empty git repository in the knowledge dir.
    std::process::Command::new("git")
        .arg("init")
        .arg("--quiet")
        .current_dir(&knowledge_dir)
        .status()
        .unwrap();

    let config_path = tmp.path().join("lore.toml");
    let db_path = tmp.path().join("knowledge.db");

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "init",
            "--repo",
            knowledge_dir.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
            "--database",
            db_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("is not a git repository"),
        "git advisory should not appear for a git repository, got: {stderr}"
    );
}

#[test]
fn init_help_restates_the_model_and_bind_defaults() {
    Command::cargo_bin("lore")
        .unwrap()
        .args(["init", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "[default with Ollama: nomic-embed-text]",
        ))
        .stdout(predicate::str::contains("[default: localhost:3100]"));
}

/// A knowledge directory and a hand-edited config pointing at it, with a
/// layout and comments `Config::save` would not reproduce.
struct ExistingConfig {
    _tmp: tempfile::TempDir,
    knowledge_dir: std::path::PathBuf,
    database: std::path::PathBuf,
    path: std::path::PathBuf,
    contents: String,
    no_ollama: std::path::PathBuf,
}

impl ExistingConfig {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let knowledge_dir = std::fs::canonicalize(tmp.path()).unwrap().join("knowledge");
        std::fs::create_dir_all(&knowledge_dir).unwrap();
        std::fs::write(knowledge_dir.join("logging.md"), "# Logging\n\nLog.\n").unwrap();
        let database = knowledge_dir.with_file_name("knowledge.db");
        let contents = format!(
            "# Hand-edited.\n\
             knowledge_dir = \"{}\"\n\
             database = \"{}\"\n\
             bind = \"localhost:3100\"\n\n\
             [search]\n\
             top_k = 9\n\
             hybrid = true\n\n\
             [ollama]\n\
             model = \"nomic-embed-text\"   # pinned below\n\
             host = \"http://127.0.0.1:1\"\n\
             dimensions = 768\n\n\
             [chunking]\n\
             strategy = \"heading\"\n\
             max_tokens = 1024\n",
            knowledge_dir.display(),
            database.display()
        );
        let path = tmp.path().join("lore.toml");
        std::fs::write(&path, &contents).unwrap();
        let no_ollama = tmp.path().join("empty-path");
        std::fs::create_dir_all(&no_ollama).unwrap();
        Self {
            _tmp: tmp,
            knowledge_dir,
            database,
            path,
            contents,
            no_ollama,
        }
    }

    /// Run `lore init` with no `ollama` on `PATH`, so provisioning fails
    /// fast wherever init gets that far.
    fn init(&self, flags: &[&str]) -> (Option<i32>, String) {
        self.init_at(&self.knowledge_dir, flags)
    }

    fn init_at(&self, repo: &Path, flags: &[&str]) -> (Option<i32>, String) {
        let output = Command::cargo_bin("lore")
            .unwrap()
            .env("PATH", &self.no_ollama)
            .arg("--config")
            .arg(&self.path)
            .arg("init")
            .arg("--repo")
            .arg(repo)
            .args(flags)
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }

    fn assert_untouched(&self) {
        assert_eq!(std::fs::read_to_string(&self.path).unwrap(), self.contents);
    }
}

#[test]
fn init_rerun_leaves_a_hand_edited_config_byte_identical() {
    let existing = ExistingConfig::new();

    let (code, stderr) = existing.init(&[]);

    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("Using existing config at"), "{stderr}");
    assert!(stderr.contains("Ollama is not installed"), "{stderr}");
    existing.assert_untouched();
}

#[test]
fn init_flags_that_agree_with_the_existing_config_proceed() {
    let existing = ExistingConfig::new();
    let database = existing.database.to_str().unwrap();

    let (code, stderr) = existing.init(&[
        "--model",
        "nomic-embed-text",
        "--bind",
        "localhost:3100",
        "--database",
        database,
    ]);

    assert_eq!(code, Some(1), "{stderr}");
    assert!(!stderr.contains("disagrees"), "{stderr}");
    assert!(stderr.contains("--- Provisioning ---"), "{stderr}");
    existing.assert_untouched();
}

#[test]
fn init_model_flag_matches_the_file_by_ollama_tag_rule() {
    let existing = ExistingConfig::new();

    let (code, stderr) = existing.init(&["--model", "nomic-embed-text:latest"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(!stderr.contains("disagrees"), "{stderr}");
    assert!(stderr.contains("--- Provisioning ---"), "{stderr}");

    let (code, stderr) = existing.init(&["--model", "nomic-embed-text:v1.5"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "[ollama] model is \"nomic-embed-text\" in the file, \"nomic-embed-text:v1.5\" from --model"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("Provisioning"), "{stderr}");
    existing.assert_untouched();
}

#[test]
fn init_database_flag_spelled_through_dotdot_matches_a_database_not_yet_created() {
    let existing = ExistingConfig::new();
    assert!(!existing.database.exists());
    let database = existing.knowledge_dir.join("..").join("knowledge.db");

    let (code, stderr) = existing.init(&["--database", database.to_str().unwrap()]);

    assert_eq!(code, Some(1), "{stderr}");
    assert!(!stderr.contains("disagrees"), "{stderr}");
    assert!(stderr.contains("--- Provisioning ---"), "{stderr}");
    existing.assert_untouched();
}

/// Run a first `lore init` with no `ollama` on `PATH` and return the config
/// it saved before provisioning failed.
fn first_init(flags: &[&str]) -> Config {
    let tmp = tempfile::tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();
    let no_ollama = tmp.path().join("empty-path");
    std::fs::create_dir_all(&no_ollama).unwrap();
    let config_path = tmp.path().join("lore.toml");

    let output = Command::cargo_bin("lore")
        .unwrap()
        .env("PATH", &no_ollama)
        .arg("--config")
        .arg(&config_path)
        .arg("init")
        .arg("--repo")
        .arg(&knowledge_dir)
        .arg("--database")
        .arg(tmp.path().join("knowledge.db"))
        .args(flags)
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Config written to"), "{stderr}");

    Config::load(&config_path).unwrap()
}

#[test]
fn first_init_saves_the_default_model_and_bind() {
    let config = first_init(&[]);

    assert_eq!(config.ollama.unwrap().model, "nomic-embed-text");
    assert_eq!(config.bind, "localhost:3100");
}

#[test]
fn first_init_saves_the_model_and_bind_flags() {
    let config = first_init(&["--model", "mxbai-embed-large", "--bind", "localhost:3200"]);

    assert_eq!(config.ollama.unwrap().model, "mxbai-embed-large");
    assert_eq!(config.bind, "localhost:3200");
}

#[test]
fn init_flag_that_contradicts_the_existing_config_errors_and_leaves_it_untouched() {
    let existing = ExistingConfig::new();
    let other_repo = existing.knowledge_dir.with_file_name("other");
    std::fs::create_dir_all(&other_repo).unwrap();
    let database = existing.database.display().to_string();
    let other_database = existing.database.with_file_name("other.db");
    let other_database = other_database.to_str().unwrap();
    let knowledge_dir = existing.knowledge_dir.display().to_string();
    let other_repo = other_repo.to_str().unwrap();

    for (flags, conflict) in [
        (
            vec!["--model", "mxbai-embed-large"],
            "[ollama] model is \"nomic-embed-text\" in the file, \"mxbai-embed-large\" from --model"
                .to_string(),
        ),
        (
            vec!["--bind", "localhost:3200"],
            "bind is \"localhost:3100\" in the file, \"localhost:3200\" from --bind".to_string(),
        ),
        (
            vec!["--database", other_database],
            format!("database is \"{database}\" in the file, \"{other_database}\" from --database"),
        ),
    ] {
        let (code, stderr) = existing.init(&flags);

        assert_eq!(code, Some(1), "{stderr}");
        assert!(stderr.contains(&conflict), "{stderr}");
        assert!(stderr.contains("remove it to start fresh"), "{stderr}");
        assert!(!stderr.contains("Provisioning"), "{stderr}");
        existing.assert_untouched();
    }

    let (code, stderr) = existing.init_at(Path::new(other_repo), &[]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "knowledge_dir is \"{knowledge_dir}\" in the file, \"{other_repo}\" from --repo"
        )),
        "{stderr}"
    );
    existing.assert_untouched();
}

#[test]
fn init_leaves_a_malformed_config_untouched() {
    let mut existing = ExistingConfig::new();
    existing.contents = "knowledge_dir = [unclosed\n".to_string();
    std::fs::write(&existing.path, &existing.contents).unwrap();

    let (code, stderr) = existing.init(&[]);

    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("could not be loaded and was left untouched"),
        "{stderr}"
    );
    assert!(stderr.contains("TOML parse error"), "{stderr}");
    assert_eq!(
        stderr
            .matches(existing.path.display().to_string().as_str())
            .count(),
        1,
        "{stderr}"
    );
    assert!(!stderr.contains("Config not found"), "{stderr}");
    assert!(!stderr.contains("Run 'lore init' first"), "{stderr}");
    assert!(!stderr.contains("Provisioning"), "{stderr}");
    existing.assert_untouched();
}

#[test]
fn init_with_a_directory_at_the_config_path_names_the_read_error() {
    let existing = ExistingConfig::new();
    std::fs::remove_file(&existing.path).unwrap();
    std::fs::create_dir(&existing.path).unwrap();

    let (code, stderr) = existing.init(&[]);

    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "Cannot read config at {}: ",
            existing.path.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("Config not found"), "{stderr}");
    assert!(!stderr.contains("Run 'lore init' first"), "{stderr}");
    assert!(!stderr.contains("Provisioning"), "{stderr}");
    assert!(existing.path.is_dir());
}

#[cfg(unix)]
#[test]
fn init_with_an_unreadable_config_names_the_read_error() {
    use std::os::unix::fs::PermissionsExt;

    let existing = ExistingConfig::new();
    let set_mode = |mode| {
        std::fs::set_permissions(&existing.path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    set_mode(0o000);
    // Root reads the file regardless of its mode, so there is nothing to test.
    if std::fs::read(&existing.path).is_ok() {
        return;
    }

    let (code, stderr) = existing.init(&[]);
    set_mode(0o644);

    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "Cannot read config at {}: ",
            existing.path.display()
        )),
        "{stderr}"
    );
    assert!(stderr.contains("ermission denied"), "{stderr}");
    assert!(!stderr.contains("Config not found"), "{stderr}");
    assert!(!stderr.contains("Run 'lore init' first"), "{stderr}");
    existing.assert_untouched();
}

#[test]
fn status_reports_a_malformed_config_instead_of_a_missing_one() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("lore.toml");
    std::fs::write(&config_path, "knowledge_dir = [unclosed\n").unwrap();

    let output = Command::cargo_bin("lore")
        .unwrap()
        .arg("--config")
        .arg(&config_path)
        .arg("status")
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "✗ Cannot parse config at {}: TOML parse error",
            config_path.display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("not found"), "{stderr}");
    assert!(!stderr.contains("lore init"), "{stderr}");
}

#[test]
fn hook_with_a_malformed_config_stays_silent_on_stdout_and_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("lore.toml");
    std::fs::write(&config_path, "knowledge_dir = [unclosed\n").unwrap();
    let input = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "test-session"
    });

    Command::cargo_bin("lore")
        .unwrap()
        .arg("--config")
        .arg(&config_path)
        .arg("hook")
        .write_stdin(input.to_string())
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(format!(
            "lore hook: Cannot parse config at {}",
            config_path.display()
        )));
}

#[test]
fn status_without_config_shows_init_hint() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("lore")
        .unwrap()
        .arg("status")
        .env("XDG_CONFIG_HOME", tmp.path())
        .env("HOME", tmp.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("lore init"));
}

#[test]
fn status_renders_languages_line_for_mixed_declared_and_undeclared_kb() {
    // End-to-end coverage for the wiring between the SQL aggregation,
    // the formatter, and the `eprintln!` in `cmd_status`. Unit tests
    // already pin the helpers; this catches stream/label regressions
    // (e.g. a stray `println!`, a typo'd label, or a reorder against
    // `Sources:`).
    let tmp = tempfile::tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    // Three patterns: one declared rust, one declared multi-language
    // (rust + typescript — bucket sum exceeds source count), one with
    // no `language:` declaration.
    std::fs::write(
        knowledge_dir.join("rust-only.md"),
        "---\n\
         language: rust\n\
         tags: [rust]\n\
         ---\n\n\
         # Rust only\n\nBody text long enough to ingest.\n",
    )
    .unwrap();
    std::fs::write(
        knowledge_dir.join("multi-lang.md"),
        "---\n\
         language: [rust, typescript]\n\
         tags: [rust, typescript]\n\
         ---\n\n\
         # Multi-language\n\nBody text long enough to ingest.\n",
    )
    .unwrap();
    std::fs::write(
        knowledge_dir.join("untagged.md"),
        "---\n\
         tags: [cli]\n\
         ---\n\n\
         # Untagged\n\nBody text long enough to ingest.\n",
    )
    .unwrap();

    let db_path = tmp.path().join("knowledge.db");
    let config = Config::default_with(knowledge_dir.clone(), db_path, "nomic-embed-text");
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();

    let embedder = FakeEmbedder::new();
    let db = KnowledgeDB::open(&config.database).unwrap();
    db.init().unwrap();
    db.ensure_vector_table(embedder.dimensions()).unwrap();
    ingest::ingest(&db, &embedder, &knowledge_dir, "heading", &|_| {});

    let assert = Command::cargo_bin("lore")
        .unwrap()
        .args(["--config", config_path.to_str().unwrap(), "status"])
        .assert()
        .success();

    // Rendered with display names, sorted by count desc with alpha
    // tiebreak on display name (Rust 2, TypeScript 1), undeclared last.
    assert.stderr(predicate::str::contains(
        "Languages:    Rust 2, TypeScript 1, undeclared 1",
    ));
}

#[test]
fn status_suppresses_languages_line_on_empty_knowledge_dir() {
    // Empty knowledge dir → empty DB → Languages line suppressed.
    // Sources line is also `0` but should still render.
    let tmp = tempfile::tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    let db_path = tmp.path().join("knowledge.db");
    let config = Config::default_with(knowledge_dir.clone(), db_path, "nomic-embed-text");
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();

    let embedder = FakeEmbedder::new();
    let db = KnowledgeDB::open(&config.database).unwrap();
    db.init().unwrap();
    db.ensure_vector_table(embedder.dimensions()).unwrap();
    // No ingest call — DB stays empty.

    Command::cargo_bin("lore")
        .unwrap()
        .args(["--config", config_path.to_str().unwrap(), "status"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Sources:      0"))
        .stderr(predicate::str::contains("Languages:").not());
}

// -- top-k ------------------------------------------------------------------

#[test]
fn search_top_k_limits_results() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    // Without --top-k, we should get both patterns.
    let out = Command::cargo_bin("lore")
        .unwrap()
        .args(["search", "--config", config_path.to_str().unwrap(), "rust"])
        .assert()
        .success();
    // Both patterns mention "rust" so at least 2 results expected.
    out.stdout(predicate::str::contains("[2]"));

    // With --top-k 1, only one result.
    let out = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--config",
            config_path.to_str().unwrap(),
            "--top-k",
            "1",
            "rust",
        ])
        .assert()
        .success();
    out.stdout(predicate::str::contains("[1]"))
        .stdout(predicate::str::contains("[2]").not());
}

#[test]
fn search_top_k_zero_returns_no_results() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--config",
            config_path.to_str().unwrap(),
            "--top-k",
            "0",
            "rust",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("No results found."));
}

/// Build an index with no vector table, configured against a host that
/// refuses connections.
fn text_only_index(tmp: &Path) -> (Config, std::path::PathBuf) {
    let knowledge_dir = tmp.join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();
    std::fs::write(
        knowledge_dir.join("rust-testing.md"),
        "# Rust Testing\n\nPrefer integration tests over mocks.\n",
    )
    .unwrap();
    let mut config = Config::default_with(
        knowledge_dir.clone(),
        tmp.join("knowledge.db"),
        "nomic-embed-text",
    );
    config.ollama.as_mut().unwrap().host = "http://127.0.0.1:1".to_string();
    let config_path = tmp.join("lore.toml");
    config.save(&config_path).unwrap();

    let db = KnowledgeDB::open(&config.database).unwrap();
    db.init().unwrap();
    ingest::full_ingest(
        &db,
        &FailingEmbedder::new(768),
        &knowledge_dir,
        "heading",
        &|_| {},
    );
    assert_eq!(db.vector_dimensions().unwrap(), None);
    (config, config_path)
}

#[test]
fn search_on_text_only_index_returns_text_results_without_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, config_path) = text_only_index(tmp.path());

    // An embed attempt against the refusing host would also return text
    // results, but with a warning; no warning proves the embedder was skipped.
    Command::cargo_bin("lore")
        .unwrap()
        .args(["search", "--config", config_path.to_str().unwrap(), "mocks"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rust Testing"))
        .stderr(predicate::str::contains("Warning").not());
    Command::cargo_bin("lore")
        .unwrap()
        .args(["status", "--config", config_path.to_str().unwrap()])
        .assert()
        .success();

    let db = KnowledgeDB::open(&config.database).unwrap();
    assert_eq!(
        db.vector_dimensions().unwrap(),
        None,
        "read paths must not create the vector table"
    );
}

#[test]
fn ingest_file_on_text_only_index_lands_text_and_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, config_path) = text_only_index(tmp.path());
    let draft = config.knowledge_dir.join("draft.md");
    std::fs::write(&draft, "# Draft\n\nQuokka handling notes.\n").unwrap();

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "ingest",
            "--config",
            config_path.to_str().unwrap(),
            "--file",
            draft.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("Done (single-file): draft.md"))
        .stderr(predicate::str::contains("embedding failure").not())
        .stderr(predicate::str::contains(
            "Note: the index has no vectors, so draft.md is searchable by text only",
        ))
        .stderr(predicate::str::contains("Run 'lore ingest'"));

    let db = KnowledgeDB::open(&config.database).unwrap();
    assert_eq!(db.vector_dimensions().unwrap(), None);
    assert!(
        db.search_fts("quokka", 10)
            .unwrap()
            .iter()
            .any(|r| r.source_file == "draft.md")
    );
}

#[test]
fn ingest_with_hybrid_search_off_prints_no_text_only_note() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut config, config_path) = text_only_index(tmp.path());
    config.search.hybrid = false;
    config.save(&config_path).unwrap();
    let draft = config.knowledge_dir.join("draft.md");
    std::fs::write(&draft, "# Draft\n\nQuokka handling notes.\n").unwrap();
    let config_arg = config_path.to_str().unwrap();

    for args in [
        vec!["ingest", "--config", config_arg],
        vec![
            "ingest",
            "--config",
            config_arg,
            "--file",
            draft.to_str().unwrap(),
        ],
    ] {
        Command::cargo_bin("lore")
            .unwrap()
            .args(&args)
            .assert()
            .success()
            .stderr(predicate::str::contains("Note: the index has no vectors").not())
            .stderr(predicate::str::contains("hybrid").not());
    }
}

#[test]
fn search_on_an_index_of_another_model_warns_until_a_forced_rebuild() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, config_path) = text_only_index(tmp.path());
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        ingest::full_ingest(
            &db,
            &FakeEmbedder::with_model("other-model", 768),
            &config.knowledge_dir,
            "heading",
            &|_| {},
        );
        assert_eq!(db.vector_dimensions().unwrap(), Some(768));
    }
    let config_arg = config_path.to_str().unwrap();
    let warning = "Warning: the index was built with other-model but lore is configured for \
                   nomic-embed-text, so searches use text only. Run 'lore ingest --force' to \
                   rebuild the index.";

    // The configured host refuses connections, so an embed attempt would
    // add its own warning; its absence proves the embedder was skipped.
    Command::cargo_bin("lore")
        .unwrap()
        .args(["search", "--config", config_arg, "mocks"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rust Testing"))
        .stderr(predicate::str::contains(warning))
        .stderr(predicate::str::contains("Ollama embed failed").not());
    let json = Command::cargo_bin("lore")
        .unwrap()
        .args(["--json", "search", "--config", config_arg, "mocks"])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let results: serde_json::Value = serde_json::from_slice(&json.get_output().stdout).unwrap();
    assert_eq!(results[0]["title"], "Rust Testing");
    Command::cargo_bin("lore")
        .unwrap()
        .args(["status", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Index model:  ✗ the index was built with other-model but lore is configured \
             for nomic-embed-text",
        ));

    Command::cargo_bin("lore")
        .unwrap()
        .args(["ingest", "--force", "--config", config_arg])
        .assert()
        .success();

    Command::cargo_bin("lore")
        .unwrap()
        .args(["search", "--config", config_arg, "mocks"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rust Testing"))
        .stderr(predicate::str::contains("lore ingest --force").not());
    Command::cargo_bin("lore")
        .unwrap()
        .args(["status", "--config", config_arg])
        .assert()
        .success()
        .stderr(predicate::str::contains("Index model:").not());
}

#[test]
fn ingest_and_ingest_file_refuse_an_index_of_another_model_and_write_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (config, config_path) = text_only_index(tmp.path());
    {
        let db = KnowledgeDB::open(&config.database).unwrap();
        ingest::full_ingest(
            &db,
            &FakeEmbedder::with_model("other-model", 768),
            &config.knowledge_dir,
            "heading",
            &|_| {},
        );
    }
    let draft = config.knowledge_dir.join("draft.md");
    std::fs::write(&draft, "# Draft\n\nQuokka handling notes.\n").unwrap();
    let config_arg = config_path.to_str().unwrap();
    let refusal = "Error: Refusing to ingest: the index was built with other-model but lore is \
                   configured for nomic-embed-text, and vectors from two models cannot be \
                   searched together. Run 'lore ingest --force' to rebuild the index";

    for args in [
        vec!["ingest", "--config", config_arg],
        vec![
            "ingest",
            "--config",
            config_arg,
            "--file",
            draft.to_str().unwrap(),
        ],
        vec![
            "ingest",
            "--config",
            config_arg,
            "--file",
            draft.to_str().unwrap(),
            "--force",
        ],
    ] {
        Command::cargo_bin("lore")
            .unwrap()
            .args(&args)
            .assert()
            .code(1)
            .stderr(predicate::str::contains(refusal))
            .stderr(predicate::str::contains("Done").not());
    }

    let db = KnowledgeDB::open(&config.database).unwrap();
    assert!(db.search_fts("quokka", 10).unwrap().is_empty());
    assert_eq!(db.stats().unwrap().sources, 1);
    assert_eq!(
        db.recorded_embedding_model().unwrap().unwrap().model,
        "other-model"
    );
    assert_eq!(db.vector_dimensions().unwrap(), Some(768));
}

// -- lore list --------------------------------------------------------------

#[test]
fn list_outputs_pattern_titles() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    Command::cargo_bin("lore")
        .unwrap()
        .args(["list", "--config", config_path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rust Testing"))
        .stdout(predicate::str::contains("Error Handling"));
}

#[test]
fn list_empty_database_exits_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("empty.db");
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    let config = Config::default_with(knowledge_dir, db_path, "nomic-embed-text");
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();

    // Ensure the database tables exist (init creates them).
    let embedder = FakeEmbedder::new();
    let db = KnowledgeDB::open(&config.database).unwrap();
    db.init().unwrap();
    db.ensure_vector_table(embedder.dimensions()).unwrap();
    drop(db);

    Command::cargo_bin("lore")
        .unwrap()
        .args(["list", "--config", config_path.to_str().unwrap()])
        .assert()
        .success()
        // No output (empty database).
        .stdout(predicate::str::is_empty());
}

// -- FTS5 sanitization ------------------------------------------------------

#[test]
fn search_with_dots_does_not_crash() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--config",
            config_path.to_str().unwrap(),
            "file.with.dots",
        ])
        .assert()
        .success();
}

#[test]
fn search_with_path_does_not_crash() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--config",
            config_path.to_str().unwrap(),
            "path/to/file.ts",
        ])
        .assert()
        .success();
}

// -- LORE_DEBUG -------------------------------------------------------------

#[test]
fn lore_debug_emits_to_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    let output = Command::cargo_bin("lore")
        .unwrap()
        .env("LORE_DEBUG", "1")
        .args(["search", "--config", config_path.to_str().unwrap(), "rust"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("[lore debug]"),
        "expected debug output on stderr, got: {stderr}"
    );
}

#[test]
fn lore_debug_off_no_debug_output() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    let output = Command::cargo_bin("lore")
        .unwrap()
        .env_remove("LORE_DEBUG")
        .args(["search", "--config", config_path.to_str().unwrap(), "rust"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("[lore debug]"),
        "unexpected debug output on stderr: {stderr}"
    );
}

// -- --json flag ------------------------------------------------------------

#[test]
fn search_json_outputs_valid_array() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "rust",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = parsed.as_array().unwrap();

    assert!(arr.len() >= 2, "expected at least 2 results");

    // Verify expected fields are present.
    let first = &arr[0];
    assert!(first.get("title").is_some());
    assert!(first.get("body").is_some());
    assert!(first.get("tags").is_some());
    assert!(first.get("source_file").is_some());
    assert!(first.get("score").is_some());
}

#[test]
fn search_json_includes_full_body() {
    let tmp = tempfile::tempdir().unwrap();
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    // Create a pattern with body longer than the 500-byte human truncation.
    let long_body = "x".repeat(800);
    std::fs::write(
        knowledge_dir.join("long-body.md"),
        format!("# Long Body\n\ntags: test\n\n{long_body}\n"),
    )
    .unwrap();

    let db_path = tmp.path().join("knowledge.db");
    let config = Config::default_with(knowledge_dir.clone(), db_path, "nomic-embed-text");
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();

    let embedder = FakeEmbedder::new();
    let db = KnowledgeDB::open(&config.database).unwrap();
    db.init().unwrap();
    db.ensure_vector_table(embedder.dimensions()).unwrap();
    ingest::ingest(&db, &embedder, &knowledge_dir, "heading", &|_| {});
    drop(db);

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "long body",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = parsed.as_array().unwrap();
    assert!(!arr.is_empty(), "expected at least 1 result");

    // Body should not be truncated.
    let body = arr[0]["body"].as_str().unwrap();
    assert!(
        body.len() >= 800,
        "JSON body should not be truncated (got {} bytes)",
        body.len()
    );
}

#[test]
fn search_json_empty_results() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "xyznonexistent",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.trim(), "[]");
}

#[test]
fn search_json_respects_top_k() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args([
            "search",
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "--top-k",
            "1",
            "rust",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1, "expected exactly 1 result with --top-k 1");
}

#[test]
fn list_json_outputs_valid_array() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_populated_env(tmp.path());

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["list", "--json", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = parsed.as_array().unwrap();

    assert!(arr.len() >= 2, "expected at least 2 patterns");

    let first = &arr[0];
    assert!(first.get("title").is_some());
    assert!(first.get("source_file").is_some());
    assert!(first.get("tags").is_some());
}

// -- lore ingest --file ------------------------------------------------------
//
// These tests exercise the CLI binary end-to-end for error paths that do not
// require a running Ollama embedder (all fail before any embed call). The
// happy path is covered by tests/single_file_ingest.rs at the library level
// with FakeEmbedder.

fn setup_empty_knowledge(dir: &Path) -> std::path::PathBuf {
    let knowledge_dir = dir.join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();
    let db_path = dir.join("knowledge.db");
    let config = Config::default_with(knowledge_dir, db_path, "nomic-embed-text");
    let config_path = dir.join("lore.toml");
    config.save(&config_path).unwrap();
    config_path
}

#[test]
fn ingest_file_rejects_unsupported_extension_with_exit_code_1() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_empty_knowledge(tmp.path());
    let txt = tmp.path().join("knowledge").join("notes.txt");
    std::fs::write(&txt, "not markdown").unwrap();

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "ingest",
            "--config",
            config_path.to_str().unwrap(),
            "--file",
            txt.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unsupported extension"))
        .stderr(predicate::str::contains("single-file ingest failed"));
}

#[test]
fn ingest_file_rejects_path_outside_knowledge_dir_with_exit_code_1() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_empty_knowledge(tmp.path());
    // File is a sibling of knowledge_dir, not inside it.
    let outside = tmp.path().join("outside.md");
    std::fs::write(
        &outside,
        "# Outside\n\nBody that is long enough for chunking.\n",
    )
    .unwrap();

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "ingest",
            "--config",
            config_path.to_str().unwrap(),
            "--file",
            outside.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("escapes the knowledge directory"));
}

#[test]
fn ingest_file_rejects_missing_file_with_cwd_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = setup_empty_knowledge(tmp.path());
    let missing = tmp.path().join("knowledge").join("does-not-exist.md");

    Command::cargo_bin("lore")
        .unwrap()
        .args([
            "ingest",
            "--config",
            config_path.to_str().unwrap(),
            "--file",
            missing.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Cannot access"))
        .stderr(predicate::str::contains("cwd:"));
}

#[test]
fn ingest_help_shows_file_flag_and_exit_codes() {
    Command::cargo_bin("lore")
        .unwrap()
        .args(["ingest", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--file"))
        .stdout(predicate::str::contains("EXIT CODES"))
        .stdout(predicate::str::contains("EXAMPLES"));
}

#[test]
fn list_json_empty_database() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("empty.db");
    let knowledge_dir = tmp.path().join("knowledge");
    std::fs::create_dir_all(&knowledge_dir).unwrap();

    let config = Config::default_with(knowledge_dir, db_path, "nomic-embed-text");
    let config_path = tmp.path().join("lore.toml");
    config.save(&config_path).unwrap();

    let embedder = FakeEmbedder::new();
    let db = KnowledgeDB::open(&config.database).unwrap();
    db.init().unwrap();
    db.ensure_vector_table(embedder.dimensions()).unwrap();
    drop(db);

    let output = Command::cargo_bin("lore")
        .unwrap()
        .args(["list", "--json", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.trim(), "[]");
}
