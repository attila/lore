// SPDX-License-Identifier: MIT OR Apache-2.0

//! `SQLite` + FTS5 + sqlite-vec database layer.
//!
//! Stores markdown chunks in three tables:
//! - `chunks` — relational metadata
//! - `patterns_fts` — FTS5 full-text index
//! - `patterns_vec` — vec0 vector index (cosine distance)
//!
//! Search supports full-text, vector, and hybrid (Reciprocal Rank Fusion).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Once;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use serde::Serialize;

use crate::chunking::{Chunk, PatternRow};

// ---------------------------------------------------------------------------
// sqlite-vec FFI registration
// ---------------------------------------------------------------------------

static SQLITE_VEC_INIT: Once = Once::new();

/// Register the sqlite-vec extension as an auto-extension so every new
/// connection gets it automatically.
#[allow(unsafe_code)]
fn register_sqlite_vec() {
    SQLITE_VEC_INIT.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is the documented entrypoint for the sqlite-vec
        // extension, re-exported by the `sqlite-vec` crate. `transmute` converts the
        // bare function pointer into the type expected by `sqlite3_auto_extension`.
        // This is the pattern used in the sqlite-vec crate's own test suite.
        unsafe {
            type AutoExtFn = unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::ffi::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> std::ffi::c_int;
            rusqlite::ffi::sqlite3_auto_extension(Some(
                std::mem::transmute::<*const (), AutoExtFn>(
                    sqlite_vec::sqlite3_vec_init as *const (),
                ),
            ));
        }
    });
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Handle to the local knowledge database.
pub struct KnowledgeDB {
    conn: Connection,
    dimensions: usize,
}

/// A single search result returned by FTS, vector, or hybrid queries.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: String,
    pub source_file: String,
    pub heading_path: String,
    pub score: f64,
    /// `true` when the chunk's source pattern is tagged `universal` —
    /// re-injected on every relevant `PreToolUse` call (bypasses dedup).
    pub is_universal: bool,
}

/// Aggregate statistics about the database contents.
pub struct DBStats {
    pub chunks: usize,
    pub sources: usize,
}

/// One entry per source document, used by `lore list` and the MCP
/// `list_patterns` tool.
///
/// Intentionally body-free: listing surfaces never need `raw_body` and
/// shipping it over MCP would bloat responses. [`UniversalPattern`] is the
/// dedicated shape for render callers that do need the body.
#[derive(Debug, Clone, Serialize)]
pub struct PatternSummary {
    pub title: String,
    pub source_file: String,
    pub tags: String,
    /// `true` when any chunk from this source pattern is tagged `universal`.
    pub is_universal: bool,
}

/// A universal-tagged pattern with its full authorial body, returned by
/// [`KnowledgeDB::universal_patterns`] for the `## Pinned conventions`
/// render at `SessionStart` / `PostCompact`. Distinct from
/// [`PatternSummary`] because rendering needs the body and listing does
/// not — keeping the shapes separate prevents accidental body leakage
/// into listing surfaces.
#[derive(Debug, Clone)]
pub struct UniversalPattern {
    pub source_file: String,
    pub title: String,
    pub tags: String,
    pub raw_body: String,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl KnowledgeDB {
    /// Open (or create) a database at `db_path` configured for `dimensions`-wide
    /// embeddings.
    ///
    /// Probes the `chunks` table for the `is_universal` column. If the table
    /// exists from a prior version of lore but lacks the column, returns a
    /// friendly upgrade-required error instead of letting individual SELECTs
    /// fail later with a confusing `no such column` message.
    pub fn open(db_path: &Path, dimensions: usize) -> anyhow::Result<Self> {
        Self::open_inner(db_path, dimensions, true)
    }

    /// Open the database without the schema-compatibility probe. Reserved for
    /// `lore ingest --force`: the very next step after opening is `clear_all`,
    /// which drops and recreates the tables with the current DDL, so running
    /// the probe would block the only path that actually fixes the condition
    /// it warns about.
    pub fn open_skipping_schema_check(db_path: &Path, dimensions: usize) -> anyhow::Result<Self> {
        Self::open_inner(db_path, dimensions, false)
    }

    fn open_inner(db_path: &Path, dimensions: usize, check_schema: bool) -> anyhow::Result<Self> {
        register_sqlite_vec();

        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;",
        )?;

        if check_schema {
            check_schema_compatibility(&conn)?;
        }

        Ok(Self { conn, dimensions })
    }

    /// Create all tables if they don't already exist, then stamp the schema
    /// version so `check_schema_compatibility` can detect old databases with
    /// a single-integer read instead of parsing `PRAGMA table_info` columns.
    pub fn init(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(PATTERNS_FTS_DDL)?;
        self.conn.execute_batch(CHUNKS_DDL)?;
        self.conn
            .execute_batch(&patterns_vec_ddl(self.dimensions))?;
        self.conn.execute_batch(INGEST_METADATA_DDL)?;
        self.conn.execute_batch(PATTERNS_DDL)?;
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    /// Drop and recreate all tables so that any column additions, index
    /// changes, or tokenizer updates in the DDL take effect. Used by
    /// `lore ingest --force` as the authoritative path back to a clean
    /// on-disk schema after a binary upgrade.
    ///
    /// `chunks` is dropped and recreated too — `DELETE FROM chunks` would
    /// leave the old column list intact, so an old-schema database that
    /// somehow bypassed `check_schema_compatibility` would still reject
    /// subsequent inserts against the missing `is_universal` column.
    pub fn clear_all(&self) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch("DROP TABLE IF EXISTS patterns_fts")?;
        tx.execute_batch(PATTERNS_FTS_DDL)?;
        tx.execute_batch("DROP TABLE IF EXISTS chunks")?;
        tx.execute_batch(CHUNKS_DDL)?;
        tx.execute_batch("DELETE FROM patterns_vec")?;
        tx.execute_batch("DROP TABLE IF EXISTS patterns")?;
        tx.execute_batch(PATTERNS_DDL)?;
        // Keep the schema stamp in sync with the freshly recreated tables.
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }

    /// Delete all chunks AND the `patterns` row belonging to a specific
    /// source file, atomically. Used for single-file re-indexing after writes
    /// and for deletions observed by delta-ingest.
    ///
    /// The `patterns` row deletion is part of the same transaction as the
    /// chunk deletions — no reader ever observes a state where chunks exist
    /// without a matching patterns row (or vice versa). Enforces the 1:1
    /// invariant per R4.
    pub fn delete_by_source(&self, source_file: &str) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        delete_pattern_and_chunks_in_tx(&tx, source_file)?;
        tx.commit()?;
        Ok(())
    }

    /// Insert a chunk (with optional embedding) into all three tables under
    /// a self-contained transaction. Callers composing multiple writes into
    /// one outer transaction (single-file ingest) should use
    /// [`insert_chunk_in_tx`] instead.
    pub fn insert_chunk(&self, chunk: &Chunk, embedding: Option<&[f32]>) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        insert_chunk_in_tx(&tx, chunk, embedding)?;
        tx.commit()?;
        Ok(())
    }

    /// Full-text search via FTS5. Returns results ordered by weighted BM25 rank.
    ///
    /// Column weights (positional, matching FTS5 column order):
    /// `title`=10, `body`=1, `tags`=5, `source_file`=0
    ///
    /// The query is sanitized before passing to FTS5 `MATCH` so that
    /// arbitrary user input (file paths, dotted names, etc.) never crashes.
    pub fn search_fts(&self, query: &str, limit: usize) -> anyhow::Result<Vec<SearchResult>> {
        let query = sanitize_fts_query(query);
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.title, c.body, c.tags, c.source_file, c.heading_path,
                    bm25(patterns_fts, 10.0, 1.0, 5.0, 0.0) AS score, c.is_universal
             FROM patterns_fts f
             JOIN chunks c ON c.id = f.chunk_id
             WHERE patterns_fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
        )?;

        #[allow(clippy::cast_possible_wrap)]
        let limit_i64 = limit as i64;
        let rows = stmt.query_map(params![query, limit_i64], |row| {
            Ok(SearchResult {
                id: row.get(0)?,
                title: row.get(1)?,
                body: row.get(2)?,
                tags: row.get(3)?,
                source_file: row.get(4)?,
                heading_path: row.get(5)?,
                score: row.get(6)?,
                is_universal: row.get::<_, i64>(7)? != 0,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Vector similarity search via sqlite-vec. Returns results ordered by distance.
    pub fn search_vector(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> anyhow::Result<Vec<SearchResult>> {
        let blob = vec_to_blob(query_embedding);
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.title, c.body, c.tags, c.source_file, c.heading_path,
                    v.distance AS score, c.is_universal
             FROM patterns_vec v
             JOIN chunks c ON c.id = v.id
             WHERE v.embedding MATCH ?1
               AND k = ?2
             ORDER BY v.distance",
        )?;

        #[allow(clippy::cast_possible_wrap)]
        let limit_i64 = limit as i64;
        let rows = stmt.query_map(params![blob, limit_i64], |row| {
            Ok(SearchResult {
                id: row.get(0)?,
                title: row.get(1)?,
                body: row.get(2)?,
                tags: row.get(3)?,
                source_file: row.get(4)?,
                heading_path: row.get(5)?,
                score: row.get(6)?,
                is_universal: row.get::<_, i64>(7)? != 0,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Hybrid search combining FTS5 and vector results via Reciprocal Rank Fusion.
    ///
    /// When `query_embedding` is `None`, falls back to FTS-only results.
    pub fn search_hybrid(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        limit: usize,
    ) -> anyhow::Result<Vec<SearchResult>> {
        let fts_results = self.search_fts(query, limit * 2)?;

        let Some(emb) = query_embedding else {
            return Ok(fts_results.into_iter().take(limit).collect());
        };

        let vec_results = self.search_vector(emb, limit * 2)?;

        Ok(reciprocal_rank_fusion(&fts_results, &vec_results, limit))
    }

    /// Return one entry per source document.
    ///
    /// Used by `lore list` and the MCP `list_patterns` tool to show a
    /// compact pattern index. Queries the `patterns` table directly — one
    /// row per indexed file, keyed on `source_file` — so the result is a
    /// simple ordered scan with no `DISTINCT`/`GROUP BY` gymnastics.
    pub fn list_patterns(&self) -> anyhow::Result<Vec<PatternSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_file, title, tags, is_universal \
             FROM patterns \
             ORDER BY source_file",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(PatternSummary {
                source_file: row.get(0)?,
                title: row.get(1)?,
                tags: row.get(2)?,
                is_universal: row.get::<_, i64>(3)? != 0,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Return one entry per universal-tagged source document, including the
    /// full authorial `raw_body` for rendering. Used by the `SessionStart`
    /// hook to emit the `## Pinned conventions` section without re-reading
    /// the source markdown from disk — the DB is the sole runtime read
    /// surface for indexed content (see `docs/architecture.md`).
    pub fn universal_patterns(&self) -> anyhow::Result<Vec<UniversalPattern>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_file, title, tags, raw_body \
             FROM patterns \
             WHERE is_universal = 1 \
             ORDER BY source_file",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(UniversalPattern {
                source_file: row.get(0)?,
                title: row.get(1)?,
                tags: row.get(2)?,
                raw_body: row.get(3)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Insert or replace a `patterns` row under a self-contained transaction.
    /// Callers composing multiple writes into one outer transaction should
    /// use [`upsert_pattern_in_tx`] instead.
    pub fn upsert_pattern(&self, row: &PatternRow) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        upsert_pattern_in_tx(&tx, row)?;
        tx.commit()?;
        Ok(())
    }

    /// Begin an outer transaction for multi-write ingest composition.
    ///
    /// Opens with `BEGIN DEFERRED` via rusqlite's `unchecked_transaction` —
    /// the plan called for `BEGIN IMMEDIATE` (acquires the write lock at
    /// BEGIN so writer-vs-writer races don't force rollbacks), but rusqlite
    /// exposes `transaction_with_behavior` only on `&mut Connection`, and
    /// `KnowledgeDB` holds an immutable `Connection` so hook reads and
    /// ingest writes can share the handle. The trade-off is acceptable
    /// because ingest is the only writer path in this codebase and the
    /// critical concurrency guarantee (R4b: embedder runs outside the
    /// transaction window) is unaffected by DEFERRED vs IMMEDIATE — the
    /// write lock hold time is measured in milliseconds either way.
    ///
    /// Used by single-file ingest to wrap `delete_pattern_and_chunks_in_tx`,
    /// `upsert_pattern_in_tx`, and per-chunk `insert_chunk_in_tx` in one
    /// transaction so no reader ever observes a mismatched state.
    pub fn begin_immediate_tx(&self) -> anyhow::Result<Transaction<'_>> {
        self.conn.unchecked_transaction().map_err(Into::into)
    }

    /// Return all chunks from the given source files.
    ///
    /// Used by the hook pipeline to expand search results to include all sibling
    /// chunks from matched documents (e.g., if the Error Handling section matched,
    /// also inject Functions, Naming, Exports sections from the same file).
    pub fn chunks_by_sources(&self, source_files: &[&str]) -> anyhow::Result<Vec<SearchResult>> {
        if source_files.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders: Vec<String> = (1..=source_files.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT id, title, body, tags, source_file, heading_path, 0.0 AS score, is_universal \
             FROM chunks WHERE source_file IN ({}) ORDER BY source_file, id",
            placeholders.join(", ")
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = source_files
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();

        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok(SearchResult {
                id: row.get(0)?,
                title: row.get(1)?,
                body: row.get(2)?,
                tags: row.get(3)?,
                source_file: row.get(4)?,
                heading_path: row.get(5)?,
                score: row.get(6)?,
                is_universal: row.get::<_, i64>(7)? != 0,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Return aggregate statistics about the database.
    ///
    /// `sources` counts rows in the `patterns` table directly — the authorial
    /// view of indexed files. `chunks` still counts rows in `chunks`.
    #[allow(clippy::cast_sign_loss)]
    pub fn stats(&self) -> anyhow::Result<DBStats> {
        let chunks: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        let sources: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM patterns", [], |row| row.get(0))?;
        Ok(DBStats {
            // COUNT(*) is always non-negative, so sign loss is not a concern.
            #[allow(clippy::cast_possible_truncation)]
            chunks: chunks as usize,
            #[allow(clippy::cast_possible_truncation)]
            sources: sources as usize,
        })
    }

    /// Return every `source_file` currently indexed, in alphabetical order.
    ///
    /// Used by the `.loreignore` reconciliation pass to find files that need
    /// to be removed when ignore patterns change. Queries the `patterns`
    /// table directly — `source_file` is the primary key so the result is a
    /// simple ordered index scan.
    pub fn source_files(&self) -> anyhow::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT source_file FROM patterns ORDER BY source_file")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Read a metadata value by key.
    pub fn get_metadata(&self, key: &str) -> anyhow::Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM ingest_metadata WHERE key = ?1")?;
        let result = stmt.query_row(params![key], |row| row.get(0)).optional()?;
        Ok(result)
    }

    /// Write a metadata key-value pair (upsert).
    pub fn set_metadata(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO ingest_metadata (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Make arbitrary user input safe for FTS5 `MATCH`.
///
/// FTS5 crashes on dots, slashes, colons, braces, quotes, asterisks, and
/// carets. This function replaces those characters with spaces, strips
/// leading minus from terms (FTS5 `NOT` operator), then collapses runs of
/// whitespace and trims.
///
/// Parentheses and the keywords `AND`, `OR`, `NOT` are preserved so that
/// callers (like the hook pipeline) can construct structured FTS5 queries
/// with grouping. Raw user input from `lore search` never contains these
/// operators in a meaningful way, so preserving them is safe.
pub fn sanitize_fts_query(query: &str) -> String {
    let cleaned: String = query
        .chars()
        .map(|c| match c {
            '.' | '/' | '\\' | ':' | '{' | '}' | '[' | ']' | '"' | '\'' | '*' | '^' | '-' => ' ',
            _ => c,
        })
        .collect();

    // Strip leading minus from each term (FTS5 treats it as a NOT operator).
    let result: Vec<&str> = cleaned
        .split_whitespace()
        .map(|term| term.trim_start_matches('-'))
        .filter(|term| !term.is_empty())
        .collect();

    result.join(" ")
}

/// Current on-disk schema version. Bumped when `CHUNKS_DDL`, the virtual-
/// table DDL, or any semantic ingest invariant changes in a way that
/// requires `lore ingest --force` from existing users.
///
/// Stored as `SQLite`'s `PRAGMA user_version` — a cheap integer slot that
/// `check_schema_compatibility` reads to decide whether the database was
/// written by a build old enough to predate the current DDL.
const SCHEMA_VERSION: u32 = 2;

/// Authoritative DDL for the `chunks` table. Used by both `KnowledgeDB::init`
/// (fresh-DB path, `CREATE TABLE IF NOT EXISTS`) and `KnowledgeDB::clear_all`
/// (post-DROP recreate). Centralising the DDL prevents the two paths from
/// drifting when a column is added or removed.
const CHUNKS_DDL: &str = "\
    CREATE TABLE IF NOT EXISTS chunks (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        body TEXT NOT NULL,
        tags TEXT DEFAULT '',
        source_file TEXT NOT NULL,
        heading_path TEXT DEFAULT '',
        is_universal INTEGER NOT NULL DEFAULT 0 CHECK (is_universal IN (0, 1)),
        ingested_at TEXT DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_chunks_source_file ON chunks(source_file)";

/// Authoritative DDL for the `patterns_fts` virtual table. Shared between
/// `init` and `clear_all` so tokenizer and column-list changes stay in lockstep.
const PATTERNS_FTS_DDL: &str = "\
    CREATE VIRTUAL TABLE IF NOT EXISTS patterns_fts USING fts5(
        title, body, tags, source_file, chunk_id UNINDEXED,
        tokenize = 'porter unicode61'
    )";

/// Authoritative DDL for `ingest_metadata`.
const INGEST_METADATA_DDL: &str =
    "CREATE TABLE IF NOT EXISTS ingest_metadata (key TEXT PRIMARY KEY, value TEXT)";

/// Authoritative DDL for the `patterns` table. Stores one row per pattern
/// file — the authorial view of an indexed document, complementary to the
/// heading-split fragments in `chunks`.
///
/// The table is the sole runtime read surface for pattern bodies: the
/// `SessionStart` / `PostCompact` pinned-section render path reads
/// `raw_body` from here instead of re-opening the source markdown on disk.
/// See `docs/architecture.md` for the "`knowledge.db` is the sole runtime
/// read surface for indexed content" invariant.
///
/// `ingested_at` uses `datetime('now')` to match the `ingest_metadata`
/// convention — ISO-8601-ish `YYYY-MM-DD HH:MM:SS` in UTC.
const PATTERNS_DDL: &str = "\
    CREATE TABLE IF NOT EXISTS patterns (
        source_file  TEXT PRIMARY KEY,
        title        TEXT NOT NULL,
        tags         TEXT NOT NULL,
        is_universal INTEGER NOT NULL DEFAULT 0 CHECK (is_universal IN (0, 1)),
        raw_body     TEXT NOT NULL,
        content_hash TEXT NOT NULL,
        ingested_at  TEXT NOT NULL DEFAULT (datetime('now'))
    )";

/// DDL for the dimensions-bound `patterns_vec` virtual table. Returned as a
/// `String` because the embedding width is a runtime value.
fn patterns_vec_ddl(dimensions: usize) -> String {
    format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS patterns_vec USING vec0(
            id TEXT PRIMARY KEY,
            embedding float[{dimensions}]
        )"
    )
}

/// Probe the on-disk `PRAGMA user_version` against [`SCHEMA_VERSION`].
///
/// Returns `Ok(())` when either the database is fresh (no `chunks` table yet
/// — `init` will stamp the version after DDL) or the stored version is at
/// least the current target. Returns a friendly upgrade-required error for
/// old-schema databases (`user_version = 0` AND `chunks` exists and is
/// non-empty).
///
/// Replaces the earlier `PRAGMA table_info(chunks)` column-parsing probe:
/// both approaches catch the same hazard, but `user_version` is a single
/// integer read and scales to future schema bumps without another column
/// name to cross-reference.
fn check_schema_compatibility(conn: &Connection) -> anyhow::Result<()> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }

    // Fresh DB: the `chunks` table does not exist yet (user_version stays 0
    // until `init` writes the DDL and stamps the version). Nothing to do.
    let chunks_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'chunks'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if chunks_exists == 0 {
        return Ok(());
    }

    // Empty-but-present chunks table: still an upgrade from a pre-schema-
    // version build, but no rows would be lost by `clear_all`. Let the
    // advisory fire anyway — `--force` is the sanctioned path back and
    // the stale-but-blank schema is still stale.
    let chunk_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
        .unwrap_or(0);
    if chunk_count == 0 {
        // Benign case: init will take over. Fresh-ish DB.
        return Ok(());
    }

    let target = SCHEMA_VERSION;
    anyhow::bail!(
        "lore: this database was written by an older version of lore \
         (schema v{version} < v{target}).\n\
         Run `lore ingest --force` to rebuild the index with the new schema.\n\
         This is expected after upgrading; see CHANGELOG for details."
    )
}

// ---------------------------------------------------------------------------
// In-transaction write helpers
// ---------------------------------------------------------------------------

/// Insert or replace a `patterns` row inside a caller-managed transaction.
/// Used by single-file ingest to compose patterns-row and chunk writes
/// atomically — see [`KnowledgeDB::begin_immediate_tx`].
pub fn upsert_pattern_in_tx(tx: &Transaction<'_>, row: &PatternRow) -> anyhow::Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO patterns \
         (source_file, title, tags, is_universal, raw_body, content_hash) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            row.source_file,
            row.title,
            row.tags,
            i64::from(row.is_universal),
            row.raw_body,
            row.content_hash,
        ],
    )?;
    Ok(())
}

/// Delete the `patterns` row plus all FTS / vec / chunk rows belonging to a
/// source file, inside a caller-managed transaction. Enforces the 1:1
/// patterns↔chunks invariant (R4) when composed inside an outer ingest
/// transaction.
pub fn delete_pattern_and_chunks_in_tx(
    tx: &Transaction<'_>,
    source_file: &str,
) -> anyhow::Result<()> {
    tx.execute(
        "DELETE FROM patterns_fts WHERE chunk_id IN \
         (SELECT id FROM chunks WHERE source_file = ?1)",
        params![source_file],
    )?;
    tx.execute(
        "DELETE FROM patterns_vec WHERE id IN \
         (SELECT id FROM chunks WHERE source_file = ?1)",
        params![source_file],
    )?;
    tx.execute(
        "DELETE FROM chunks WHERE source_file = ?1",
        params![source_file],
    )?;
    tx.execute(
        "DELETE FROM patterns WHERE source_file = ?1",
        params![source_file],
    )?;
    Ok(())
}

/// Insert a chunk (with optional embedding) into all three chunk-related
/// tables, inside a caller-managed transaction. Counterpart to
/// [`KnowledgeDB::insert_chunk`] for callers that need to compose multiple
/// writes into one outer transaction (single-file ingest).
pub fn insert_chunk_in_tx(
    tx: &Transaction<'_>,
    chunk: &Chunk,
    embedding: Option<&[f32]>,
) -> anyhow::Result<()> {
    // Delete old FTS and vec rows if they exist to prevent ghost rows.
    tx.execute(
        "DELETE FROM patterns_fts WHERE chunk_id = ?1",
        params![chunk.id],
    )?;
    tx.execute("DELETE FROM patterns_vec WHERE id = ?1", params![chunk.id])?;

    tx.execute(
        "INSERT OR REPLACE INTO chunks (id, title, body, tags, source_file, heading_path, is_universal)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            chunk.id,
            chunk.title,
            chunk.body,
            chunk.tags,
            chunk.source_file,
            chunk.heading_path,
            i64::from(chunk.is_universal),
        ],
    )?;

    tx.execute(
        "INSERT INTO patterns_fts (chunk_id, title, body, tags, source_file)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            chunk.id,
            chunk.title,
            chunk.body,
            chunk.tags,
            chunk.source_file,
        ],
    )?;

    if let Some(emb) = embedding {
        let blob = vec_to_blob(emb);
        tx.execute(
            "INSERT INTO patterns_vec (id, embedding) VALUES (?1, ?2)",
            params![chunk.id, blob],
        )?;
    }

    Ok(())
}

/// Convert an `f32` slice to a little-endian byte blob for sqlite-vec.
fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Convert a little-endian byte blob back to `f32` values.
#[cfg(test)]
fn blob_to_vec(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Merge two ranked result lists using Reciprocal Rank Fusion (k = 60).
fn reciprocal_rank_fusion(
    list_a: &[SearchResult],
    list_b: &[SearchResult],
    limit: usize,
) -> Vec<SearchResult> {
    let k = 60.0_f64;
    let mut scores: HashMap<String, (SearchResult, f64)> = HashMap::new();

    for (i, r) in list_a.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let rrf = 1.0 / (k + i as f64 + 1.0);
        scores
            .entry(r.id.clone())
            .and_modify(|(_, s)| *s += rrf)
            .or_insert_with(|| (r.clone(), rrf));
    }

    for (i, r) in list_b.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let rrf = 1.0 / (k + i as f64 + 1.0);
        scores
            .entry(r.id.clone())
            .and_modify(|(_, s)| *s += rrf)
            .or_insert_with(|| (r.clone(), rrf));
    }

    // Normalize to 0–1 by dividing by the max possible RRF score (rank 0 in both lists).
    let max_rrf = 2.0 / (k + 1.0);

    let mut merged: Vec<_> = scores.into_values().collect();
    merged.sort_by(|a, b| b.1.total_cmp(&a.1));

    merged
        .into_iter()
        .take(limit)
        .map(|(mut r, s)| {
            r.score = s / max_rrf;
            r
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: open an in-memory `KnowledgeDB` with the given dimensions.
    fn open_memory_db(dimensions: usize) -> KnowledgeDB {
        KnowledgeDB::open(Path::new(":memory:"), dimensions).expect("failed to open in-memory DB")
    }

    /// Helper: build a non-universal `Chunk` for test use.
    fn make_chunk(id: &str, title: &str, body: &str, source: &str) -> Chunk {
        Chunk {
            id: id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            tags: String::new(),
            source_file: source.to_string(),
            heading_path: String::new(),
            is_universal: false,
        }
    }

    /// Helper: build a universal `Chunk` for test use.
    fn make_universal_chunk(id: &str, title: &str, body: &str, source: &str) -> Chunk {
        Chunk {
            id: id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            tags: "universal".to_string(),
            source_file: source.to_string(),
            heading_path: String::new(),
            is_universal: true,
        }
    }

    /// Helper: insert a chunk AND the matching `patterns` row. Mirrors the
    /// 1:1 invariant that real ingest paths enforce — tests inserting only
    /// chunks directly would break listing queries that now read the
    /// `patterns` table.
    fn seed_pattern_and_chunk(db: &KnowledgeDB, chunk: &Chunk, embedding: Option<&[f32]>) {
        db.upsert_pattern(&PatternRow {
            source_file: chunk.source_file.clone(),
            title: chunk.title.clone(),
            tags: chunk.tags.clone(),
            is_universal: chunk.is_universal,
            raw_body: chunk.body.clone(),
            content_hash: "0000000000000000".to_string(),
        })
        .unwrap();
        db.insert_chunk(chunk, embedding).unwrap();
    }

    // -- FFI verification -------------------------------------------------

    #[test]
    fn sqlite_vec_ffi_creates_vec0_table() {
        let db = open_memory_db(4);
        db.conn
            .execute_batch(
                "CREATE VIRTUAL TABLE test_vec USING vec0(
                    id TEXT PRIMARY KEY,
                    embedding float[4]
                )",
            )
            .expect("vec0 table creation should succeed");

        // Insert a row and read it back to be sure.
        let blob = vec_to_blob(&[1.0, 2.0, 3.0, 4.0]);
        db.conn
            .execute(
                "INSERT INTO test_vec (id, embedding) VALUES (?1, ?2)",
                params!["v1", blob],
            )
            .expect("insert into vec0 should succeed");

        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM test_vec", [], |row| row.get(0))
            .expect("count query should succeed");
        assert_eq!(count, 1);
    }

    // -- Insert + FTS search ----------------------------------------------

    #[test]
    fn insert_and_fts_search() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let chunk = make_chunk(
            "c1",
            "Rust Ownership",
            "Ownership and borrowing",
            "guide.md",
        );
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        db.insert_chunk(&chunk, Some(&emb)).unwrap();

        let results = db.search_fts("ownership", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "c1");
        assert_eq!(results[0].title, "Rust Ownership");
    }

    // -- Insert + vector search -------------------------------------------

    #[test]
    fn insert_and_vector_search() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let chunk = make_chunk("c1", "Vector Test", "Some body", "test.md");
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        db.insert_chunk(&chunk, Some(&emb)).unwrap();

        let query_emb = vec![1.0_f32, 0.1, 0.0, 0.0];
        let results = db.search_vector(&query_emb, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "c1");
    }

    // -- Hybrid search with RRF -------------------------------------------

    #[test]
    fn hybrid_search_combines_fts_and_vector() {
        let db = open_memory_db(4);
        db.init().unwrap();

        // Insert two chunks with different embeddings.
        let c1 = Chunk {
            id: "c1".into(),
            title: "Alpha patterns".into(),
            body: "The alpha pattern is foundational".into(),
            tags: "design".into(),
            source_file: "patterns.md".into(),
            heading_path: "Alpha".into(),
            is_universal: false,
        };
        let c2 = Chunk {
            id: "c2".into(),
            title: "Beta patterns".into(),
            body: "The beta pattern extends alpha concepts".into(),
            tags: "design".into(),
            source_file: "patterns.md".into(),
            heading_path: "Beta".into(),
            is_universal: false,
        };

        db.insert_chunk(&c1, Some(&[1.0, 0.0, 0.0, 0.0])).unwrap();
        db.insert_chunk(&c2, Some(&[0.0, 1.0, 0.0, 0.0])).unwrap();

        // Query text matches both via FTS; vector is closer to c1.
        let results = db
            .search_hybrid("alpha", Some(&[0.9, 0.1, 0.0, 0.0]), 10)
            .unwrap();
        assert!(!results.is_empty());
        // c1 should rank higher (matches both FTS and vector).
        assert_eq!(results[0].id, "c1");
    }

    // -- Hybrid search with None embedding falls back to FTS-only ---------

    #[test]
    fn hybrid_search_none_embedding_falls_back_to_fts() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let chunk = make_chunk(
            "c1",
            "Rust Ownership",
            "Ownership and borrowing",
            "guide.md",
        );
        db.insert_chunk(&chunk, Some(&[1.0, 0.0, 0.0, 0.0]))
            .unwrap();

        let results = db.search_hybrid("ownership", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "c1");
    }

    // -- clear_all --------------------------------------------------------

    #[test]
    fn clear_all_removes_all_data() {
        let db = open_memory_db(4);
        db.init().unwrap();

        db.insert_chunk(
            &make_chunk("c1", "T1", "Body one", "a.md"),
            Some(&[1.0, 0.0, 0.0, 0.0]),
        )
        .unwrap();
        db.insert_chunk(
            &make_chunk("c2", "T2", "Body two", "b.md"),
            Some(&[0.0, 1.0, 0.0, 0.0]),
        )
        .unwrap();

        let stats = db.stats().unwrap();
        assert_eq!(stats.chunks, 2);

        db.clear_all().unwrap();

        let stats = db.stats().unwrap();
        assert_eq!(stats.chunks, 0);
        assert_eq!(stats.sources, 0);
    }

    // -- delete_by_source -------------------------------------------------

    #[test]
    fn delete_by_source_removes_only_that_file() {
        let db = open_memory_db(4);
        db.init().unwrap();

        seed_pattern_and_chunk(
            &db,
            &make_chunk("c1", "T1", "Body one content", "a.md"),
            Some(&[1.0, 0.0, 0.0, 0.0]),
        );
        seed_pattern_and_chunk(
            &db,
            &make_chunk("c2", "T2", "Body two content", "b.md"),
            Some(&[0.0, 1.0, 0.0, 0.0]),
        );

        db.delete_by_source("a.md").unwrap();

        let stats = db.stats().unwrap();
        assert_eq!(stats.chunks, 1);
        assert_eq!(stats.sources, 1);

        // FTS should not find the deleted chunk.
        let fts = db.search_fts("one", 10).unwrap();
        assert!(fts.is_empty());

        // Vector search should only find the remaining chunk.
        let vec_results = db.search_vector(&[0.0, 1.0, 0.0, 0.0], 10).unwrap();
        assert_eq!(vec_results.len(), 1);
        assert_eq!(vec_results[0].id, "c2");
    }

    // -- stats ------------------------------------------------------------

    #[test]
    fn stats_returns_correct_counts() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let stats = db.stats().unwrap();
        assert_eq!(stats.chunks, 0);
        assert_eq!(stats.sources, 0);

        seed_pattern_and_chunk(
            &db,
            &make_chunk("c1", "T1", "Body one content", "a.md"),
            None,
        );
        db.insert_chunk(&make_chunk("c2", "T2", "Body two content", "a.md"), None)
            .unwrap();
        seed_pattern_and_chunk(
            &db,
            &make_chunk("c3", "T3", "Body three content", "b.md"),
            None,
        );

        let stats = db.stats().unwrap();
        assert_eq!(stats.chunks, 3);
        assert_eq!(stats.sources, 2);
    }

    // -- source_files -----------------------------------------------------

    #[test]
    fn source_files_returns_distinct_paths_in_sorted_order() {
        let db = open_memory_db(4);
        db.init().unwrap();

        seed_pattern_and_chunk(
            &db,
            &make_chunk("c1", "T1", "Body one content", "rust/b.md"),
            None,
        );
        seed_pattern_and_chunk(
            &db,
            &make_chunk("c2", "T2", "Body two content", "rust/a.md"),
            None,
        );
        db.insert_chunk(
            &make_chunk("c3", "T3", "Body three content", "rust/a.md"),
            None,
        )
        .unwrap();

        let files = db.source_files().unwrap();
        assert_eq!(
            files,
            vec!["rust/a.md".to_string(), "rust/b.md".to_string()]
        );
    }

    #[test]
    fn source_files_returns_empty_vec_for_empty_database() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let files = db.source_files().unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn source_files_excludes_files_after_delete_by_source() {
        let db = open_memory_db(4);
        db.init().unwrap();

        seed_pattern_and_chunk(
            &db,
            &make_chunk("c1", "T1", "Body one content", "a.md"),
            None,
        );
        seed_pattern_and_chunk(
            &db,
            &make_chunk("c2", "T2", "Body two content", "b.md"),
            None,
        );

        db.delete_by_source("a.md").unwrap();

        let files = db.source_files().unwrap();
        assert_eq!(files, vec!["b.md".to_string()]);
    }

    // -- RRF unit test ----------------------------------------------------

    #[test]
    fn rrf_merges_two_ranked_lists() {
        let stub = |id: &str| SearchResult {
            id: id.to_string(),
            title: String::new(),
            body: String::new(),
            tags: String::new(),
            source_file: String::new(),
            heading_path: String::new(),
            score: 0.0,
            is_universal: false,
        };
        let a = vec![stub("x"), stub("y")];
        let b = vec![stub("y"), stub("z")];

        let merged = reciprocal_rank_fusion(&a, &b, 10);
        assert_eq!(merged.len(), 3);

        // "y" appears in both lists so should have the highest RRF score.
        assert_eq!(merged[0].id, "y");

        // "y" gets rank-1 from list_a (1/62) plus rank-0 from list_b (1/61),
        // normalized by max possible RRF (2/61).
        let raw = 1.0 / 62.0 + 1.0 / 61.0;
        let max_rrf = 2.0 / 61.0;
        let expected_y = raw / max_rrf;
        assert!((merged[0].score - expected_y).abs() < 1e-10);
    }

    // -- duplicate insert ---------------------------------------------------

    #[test]
    fn duplicate_insert_produces_single_fts_row() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let chunk = make_chunk(
            "dup1",
            "Duplicate Test",
            "This body is about duplicate insertion testing",
            "dup.md",
        );
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];

        // Insert the same chunk twice.
        db.insert_chunk(&chunk, Some(&emb)).unwrap();
        db.insert_chunk(&chunk, Some(&emb)).unwrap();

        // FTS should return exactly one result for this chunk.
        let results = db.search_fts("duplicate insertion", 10).unwrap();
        assert_eq!(
            results.len(),
            1,
            "expected 1 FTS result after duplicate insert, got {}",
            results.len()
        );
        assert_eq!(results[0].id, "dup1");

        // chunks table should have exactly 1 row.
        let stats = db.stats().unwrap();
        assert_eq!(stats.chunks, 1);
    }

    // -- vec_to_blob round-trip -------------------------------------------

    #[test]
    fn vec_to_blob_round_trip() {
        let original = vec![1.0_f32, -2.5, 0.0, std::f32::consts::PI];
        let blob = vec_to_blob(&original);
        let recovered = blob_to_vec(&blob);
        assert_eq!(original, recovered);
    }

    // -- FTS5 column weighting --------------------------------------------

    #[test]
    fn fts_title_match_ranks_above_body_match() {
        let db = open_memory_db(4);
        db.init().unwrap();

        // Chunk with "typescript" in title and tags.
        let tagged = Chunk {
            id: "tagged".to_string(),
            title: "TypeScript Conventions".to_string(),
            body: "Follow these coding conventions.".to_string(),
            tags: "typescript, conventions".to_string(),
            source_file: "ts.md".to_string(),
            heading_path: String::new(),
            is_universal: false,
        };
        db.insert_chunk(&tagged, None).unwrap();

        // Chunk with "typescript" only in body text.
        let body_only = Chunk {
            id: "body_only".to_string(),
            title: "Rust Interop".to_string(),
            body: "When calling Rust from typescript, use wasm-bindgen.".to_string(),
            tags: "rust, wasm".to_string(),
            source_file: "rust.md".to_string(),
            heading_path: String::new(),
            is_universal: false,
        };
        db.insert_chunk(&body_only, None).unwrap();

        let results = db.search_fts("typescript", 10).unwrap();
        assert!(
            results.len() >= 2,
            "should find both chunks, got {}",
            results.len()
        );
        assert_eq!(
            results[0].id, "tagged",
            "title/tag match should rank first, got: {}",
            results[0].id
        );
    }

    #[test]
    fn fts_empty_tags_still_works() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let chunk = make_chunk("c1", "Error Handling", "Use anyhow for errors", "errors.md");
        db.insert_chunk(&chunk, None).unwrap();

        let results = db.search_fts("error", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Error Handling");
    }

    // -- sanitize_fts_query ------------------------------------------------

    #[test]
    fn sanitize_strips_dots_and_slashes() {
        assert_eq!(sanitize_fts_query("path/to/file.ts"), "path to file ts");
    }

    #[test]
    fn sanitize_strips_special_chars() {
        assert_eq!(
            sanitize_fts_query("foo:bar {qux} [quux] \"quoted\" 'single'"),
            "foo bar qux quux quoted single"
        );
    }

    #[test]
    fn sanitize_preserves_parentheses() {
        assert_eq!(
            sanitize_fts_query("rust AND (error OR handling)"),
            "rust AND (error OR handling)"
        );
    }

    #[test]
    fn sanitize_strips_leading_minus() {
        assert_eq!(sanitize_fts_query("-NOT this -term"), "NOT this term");
    }

    #[test]
    fn sanitize_collapses_whitespace() {
        assert_eq!(sanitize_fts_query("  lots   of   space  "), "lots of space");
    }

    #[test]
    fn sanitize_pure_special_returns_empty() {
        assert_eq!(sanitize_fts_query("...///"), "");
    }

    #[test]
    fn sanitize_asterisk_and_caret() {
        assert_eq!(sanitize_fts_query("foo* ^bar"), "foo bar");
    }

    #[test]
    fn sanitize_strips_hyphens() {
        assert_eq!(sanitize_fts_query("pre-commit hook"), "pre commit hook");
    }

    #[test]
    fn sanitize_strips_hyphens_with_column_name() {
        // "pre-commit" must not produce bare "commit" which FTS5 could
        // interpret as a column filter prefix.
        let result = sanitize_fts_query("dprint formatting pre-commit hook");
        assert_eq!(result, "dprint formatting pre commit hook");
        assert!(!result.contains("pre-commit"));
    }

    #[test]
    fn fts_search_with_dots_does_not_crash() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let chunk = make_chunk("c1", "Dotted", "Some dotted content", "a.md");
        db.insert_chunk(&chunk, None).unwrap();

        // This would crash FTS5 without sanitization.
        let results = db.search_fts("file.with.dots", 10).unwrap();
        // May or may not find results, but must not crash.
        drop(results);
    }

    #[test]
    fn fts_search_with_path_does_not_crash() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let chunk = make_chunk("c1", "Path", "Some path content", "a.md");
        db.insert_chunk(&chunk, None).unwrap();

        let results = db.search_fts("path/to/file.ts", 10).unwrap();
        drop(results);
    }

    #[test]
    fn fts_search_with_hyphenated_term_does_not_crash() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let chunk = make_chunk(
            "c1",
            "Git Hooks",
            "Run dprint check as a pre-commit hook",
            "hooks.md",
        );
        db.insert_chunk(&chunk, None).unwrap();

        // This would crash FTS5 without hyphen sanitization: "commit" could be
        // interpreted as a column filter prefix.
        let results = db.search_fts("pre-commit hook", 10).unwrap();
        assert!(
            !results.is_empty(),
            "should find the hook pattern after hyphen sanitization"
        );
    }

    #[test]
    fn fts_search_empty_after_sanitize_returns_empty() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let chunk = make_chunk("c1", "Title", "Body content", "a.md");
        db.insert_chunk(&chunk, None).unwrap();

        let results = db.search_fts("...", 10).unwrap();
        assert!(results.is_empty());
    }

    // -- list_patterns -----------------------------------------------------

    #[test]
    fn list_patterns_returns_one_per_source() {
        let db = open_memory_db(4);
        db.init().unwrap();

        // Two chunks from same source, one from another. `list_patterns`
        // reads the `patterns` table directly so tests must seed both.
        seed_pattern_and_chunk(&db, &make_chunk("c1", "Alpha", "Body A1", "alpha.md"), None);
        // Second chunk for the same source_file — upsert is idempotent on
        // `patterns` and `insert_chunk` is additive on `chunks`.
        db.insert_chunk(&make_chunk("c2", "Alpha Sub", "Body A2", "alpha.md"), None)
            .unwrap();
        seed_pattern_and_chunk(&db, &make_chunk("c3", "Beta", "Body B1", "beta.md"), None);

        let patterns = db.list_patterns().unwrap();
        assert_eq!(patterns.len(), 2);
        assert_eq!(patterns[0].source_file, "alpha.md");
        assert_eq!(patterns[0].title, "Alpha");
        assert_eq!(patterns[1].source_file, "beta.md");
        assert_eq!(patterns[1].title, "Beta");
    }

    #[test]
    fn list_patterns_empty_db() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let patterns = db.list_patterns().unwrap();
        assert!(patterns.is_empty());
    }

    #[test]
    fn fts_and_or_query_non_matching_returns_empty() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let c1 = Chunk {
            id: "c1".into(),
            title: "Rust Conventions".into(),
            body: "Use anyhow for errors.".into(),
            tags: "rust, conventions".into(),
            source_file: "rust.md".into(),
            heading_path: String::new(),
            is_universal: false,
        };
        let c2 = Chunk {
            id: "c2".into(),
            title: "TypeScript Conventions".into(),
            body: "Prefer type over interface.".into(),
            tags: "typescript, conventions".into(),
            source_file: "ts.md".into(),
            heading_path: String::new(),
            is_universal: false,
        };
        db.insert_chunk(&c1, None).unwrap();
        db.insert_chunk(&c2, None).unwrap();

        // This query has no overlapping terms with the data.
        let results = db
            .search_fts("golang AND (quantum OR physics OR simulation)", 10)
            .unwrap();
        assert!(
            results.is_empty(),
            "expected no results for non-matching AND/OR query, got {}",
            results.len()
        );
    }

    #[test]
    fn fts_and_operator_requires_all_terms() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let chunk = make_chunk("c1", "Rust Guide", "Ownership and borrowing", "rust.md");
        db.insert_chunk(&chunk, None).unwrap();

        // "rust" matches, but "xyznotreal" does not.
        // FTS5 AND should require both.
        let results = db
            .search_fts("xyznotreal AND (rust OR ownership)", 10)
            .unwrap();
        assert!(
            results.is_empty(),
            "AND should require all operands to match, got {} results",
            results.len()
        );
    }

    #[test]
    fn list_patterns_includes_tags() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let chunk = Chunk {
            id: "c1".into(),
            title: "Rust Conventions".into(),
            body: "Use snake_case".into(),
            tags: "rust, style".into(),
            source_file: "rust.md".into(),
            heading_path: "Naming".into(),
            is_universal: false,
        };
        seed_pattern_and_chunk(&db, &chunk, None);

        let patterns = db.list_patterns().unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].tags, "rust, style");
    }

    // -- sanitize_fts_query: expanded coverage ---------------------------

    #[test]
    fn sanitize_strips_backslash() {
        assert_eq!(sanitize_fts_query("foo\\bar"), "foo bar");
    }

    #[test]
    fn sanitize_combined_multi_operator_sequence() {
        assert_eq!(sanitize_fts_query("foo/bar:baz\\qux"), "foo bar baz qux");
    }

    #[test]
    fn sanitize_all_operators_returns_empty() {
        assert_eq!(sanitize_fts_query("/:.\\*^-{}[]\"'"), "");
    }

    #[test]
    fn sanitize_operators_mixed_with_terms_and_leading_minus() {
        // "rust-lang/rust:main" should strip hyphens, slashes, and colons,
        // and the leading minus on "lang" (after hyphen replacement) is a no-op
        // since split_whitespace handles the space-separated tokens.
        assert_eq!(
            sanitize_fts_query("rust-lang/rust:main"),
            "rust lang rust main"
        );
    }

    // -- universal patterns + schema probe ----------------------------

    #[test]
    fn insert_and_select_chunk_round_trips_is_universal() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let universal =
            make_universal_chunk("u1", "Workflow", "Always git push origin HEAD.", "wf.md");
        db.insert_chunk(&universal, None).unwrap();

        let normal = make_chunk("n1", "Style", "Use anyhow for errors.", "style.md");
        db.insert_chunk(&normal, None).unwrap();

        let results = db.search_fts("workflow OR style", 10).unwrap();
        let by_id: std::collections::HashMap<_, _> = results
            .iter()
            .map(|r| (r.id.as_str(), r.is_universal))
            .collect();
        assert!(by_id["u1"], "u1 should be universal");
        assert!(!by_id["n1"], "n1 should not be universal");
    }

    #[test]
    fn chunk_check_constraint_rejects_invalid_is_universal_value() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let result = db.conn.execute(
            "INSERT INTO chunks (id, title, body, source_file, is_universal) \
             VALUES ('bad', 'Bad', 'Body', 'bad.md', 2)",
            [],
        );
        assert!(
            result.is_err(),
            "CHECK constraint should reject is_universal = 2"
        );
    }

    #[test]
    fn universal_patterns_returns_only_universal_tagged() {
        let db = open_memory_db(4);
        db.init().unwrap();

        seed_pattern_and_chunk(
            &db,
            &make_universal_chunk("u1", "Workflow", "Always git push origin HEAD.", "wf.md"),
            None,
        );
        seed_pattern_and_chunk(
            &db,
            &make_chunk("n1", "Style", "Use anyhow for errors.", "style.md"),
            None,
        );
        seed_pattern_and_chunk(
            &db,
            &make_chunk("n2", "SQLite", "Use bundled sqlite.", "sql.md"),
            None,
        );

        let universal = db.universal_patterns().unwrap();
        assert_eq!(universal.len(), 1);
        assert_eq!(universal[0].source_file, "wf.md");
        // `universal_patterns()` returns only universal rows by construction;
        // no `is_universal` field — the filter is in the WHERE clause.
        assert_eq!(
            universal[0].raw_body, "Always git push origin HEAD.",
            "raw_body should carry the authorial body for render"
        );
    }

    #[test]
    fn list_patterns_marks_universal_when_any_chunk_is_universal() {
        let db = open_memory_db(4);
        db.init().unwrap();

        seed_pattern_and_chunk(
            &db,
            &make_universal_chunk("u1", "Workflow", "Always git push origin HEAD.", "wf.md"),
            None,
        );
        seed_pattern_and_chunk(
            &db,
            &make_chunk("n1", "Style", "Use anyhow for errors.", "style.md"),
            None,
        );

        let patterns = db.list_patterns().unwrap();
        let by_source: std::collections::HashMap<_, _> = patterns
            .iter()
            .map(|p| (p.source_file.as_str(), p.is_universal))
            .collect();
        assert!(by_source["wf.md"], "wf.md should be universal");
        assert!(!by_source["style.md"], "style.md should not be universal");
    }

    #[test]
    fn knowledge_db_open_probe_detects_missing_is_universal_column() {
        // Build a database manually with the OLD schema (no is_universal column).
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("old.db");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE chunks (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    body TEXT NOT NULL,
                    tags TEXT DEFAULT '',
                    source_file TEXT NOT NULL,
                    heading_path TEXT DEFAULT '',
                    ingested_at TEXT DEFAULT (datetime('now'))
                )",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chunks (id, title, body, source_file) \
                 VALUES ('c1', 'T', 'B', 'f.md')",
                [],
            )
            .unwrap();
        }

        let err = match KnowledgeDB::open(&db_path, 4) {
            Ok(_) => panic!("expected open to fail on old schema"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("lore ingest --force"),
            "expected upgrade advisory, got: {err}"
        );
    }

    #[test]
    fn knowledge_db_open_does_not_error_on_fresh_database() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("fresh.db");
        // Open a brand-new file: no chunks table yet, probe is satisfied.
        let db = KnowledgeDB::open(&db_path, 4).expect("fresh DB should open cleanly");
        db.init().expect("init should succeed");
    }

    #[test]
    fn knowledge_db_open_skipping_schema_check_bypasses_probe_for_force_ingest() {
        // Build an old-schema database (no `is_universal` column). The
        // regular `open` path rejects this with the upgrade advisory —
        // `open_skipping_schema_check` must accept it so that
        // `lore ingest --force` can reach `clear_all` and rebuild.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("old.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE chunks (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    body TEXT NOT NULL,
                    source_file TEXT NOT NULL
                )",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chunks (id, title, body, source_file) \
                 VALUES ('c1', 'T', 'B', 'f.md')",
                [],
            )
            .unwrap();
        }

        assert!(KnowledgeDB::open(&db_path, 4).is_err());

        let db = KnowledgeDB::open_skipping_schema_check(&db_path, 4)
            .expect("skip-probe open must succeed on old schema");
        db.init().expect("init under skip-probe open must succeed");
        db.clear_all()
            .expect("clear_all must drop+recreate the old chunks table");

        // After clear_all the new schema is live — a re-open through the
        // probe path succeeds, confirming the advertised remedy works.
        drop(db);
        KnowledgeDB::open(&db_path, 4).expect("re-open after clear_all must pass the probe");
    }

    #[test]
    fn clear_all_drops_and_recreates_chunks_with_current_ddl() {
        // Open a DB, init, drop the is_universal column via raw SQL to
        // simulate an incomplete migration, then confirm `clear_all`
        // restores the expected column set.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("chunks_ddl.db");
        let db = KnowledgeDB::open(&db_path, 4).unwrap();
        db.init().unwrap();

        // Forcibly swap the chunks table to the old shape — no `is_universal`.
        db.conn
            .execute_batch(
                "DROP TABLE chunks;
                 CREATE TABLE chunks (
                     id TEXT PRIMARY KEY,
                     title TEXT NOT NULL,
                     body TEXT NOT NULL,
                     source_file TEXT NOT NULL
                 )",
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO chunks (id, title, body, source_file) \
                 VALUES ('c1', 'T', 'B', 'f.md')",
                [],
            )
            .unwrap();

        db.clear_all().unwrap();

        let columns: Vec<String> = db
            .conn
            .prepare("PRAGMA table_info(chunks)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            columns.iter().any(|c| c == "is_universal"),
            "chunks must carry is_universal after clear_all, got: {columns:?}"
        );

        // And the row seeded under the old schema is gone.
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "clear_all must leave chunks empty");
    }

    // -- ingest_metadata -----------------------------------------------

    #[test]
    fn metadata_round_trip() {
        let db = open_memory_db(4);
        db.init().unwrap();
        db.set_metadata("last_commit", "abc123").unwrap();
        assert_eq!(
            db.get_metadata("last_commit").unwrap(),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn metadata_overwrite() {
        let db = open_memory_db(4);
        db.init().unwrap();
        db.set_metadata("key", "first").unwrap();
        db.set_metadata("key", "second").unwrap();
        assert_eq!(db.get_metadata("key").unwrap(), Some("second".to_string()));
    }

    #[test]
    fn metadata_missing_key_returns_none() {
        let db = open_memory_db(4);
        db.init().unwrap();
        assert_eq!(db.get_metadata("nonexistent").unwrap(), None);
    }

    #[test]
    fn metadata_empty_value() {
        let db = open_memory_db(4);
        db.init().unwrap();
        db.set_metadata("key", "").unwrap();
        assert_eq!(db.get_metadata("key").unwrap(), Some(String::new()));
    }

    // -- FTS5 porter stemming -------------------------------------------------

    #[test]
    fn porter_stemming_matches_morphological_variants() {
        let db = open_memory_db(4);
        db.init().unwrap();

        let chunk = make_chunk(
            "c1",
            "Rust Testing",
            "Use deterministic fakes for testing",
            "guide.md",
        );
        db.insert_chunk(&chunk, Some(&[1.0, 0.0, 0.0, 0.0]))
            .unwrap();

        // "fakes" should match "fake" via porter stemming (both stem to "fake").
        let results = db.search_fts("fake", 10).unwrap();
        assert!(
            !results.is_empty(),
            "porter stemming should match 'fake' against 'fakes'"
        );

        // "testing" should match "test" via porter stemming.
        let results = db.search_fts("test", 10).unwrap();
        assert!(
            !results.is_empty(),
            "porter stemming should match 'test' against 'testing'"
        );
    }
}
