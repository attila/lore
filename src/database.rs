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

use rusqlite::{Connection, params};

use crate::chunking::Chunk;

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
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: String,
    pub source_file: String,
    pub heading_path: String,
    pub score: f64,
}

/// Aggregate statistics about the database contents.
pub struct DBStats {
    pub chunks: usize,
    pub sources: usize,
}

/// One entry per source document, used by `lore list`.
#[derive(Debug, Clone)]
pub struct PatternSummary {
    pub title: String,
    pub source_file: String,
    pub tags: String,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl KnowledgeDB {
    /// Open (or create) a database at `db_path` configured for `dimensions`-wide
    /// embeddings.
    pub fn open(db_path: &Path, dimensions: usize) -> anyhow::Result<Self> {
        register_sqlite_vec();

        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;",
        )?;

        Ok(Self { conn, dimensions })
    }

    /// Create all tables if they don't already exist.
    pub fn init(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS patterns_fts USING fts5(
                title, body, tags, source_file, chunk_id UNINDEXED
            )",
        )?;

        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chunks (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                tags TEXT DEFAULT '',
                source_file TEXT NOT NULL,
                heading_path TEXT DEFAULT '',
                ingested_at TEXT DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_chunks_source_file ON chunks(source_file)",
        )?;

        self.conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS patterns_vec USING vec0(
                id TEXT PRIMARY KEY,
                embedding float[{}]
            )",
            self.dimensions
        ))?;

        Ok(())
    }

    /// Delete every row from all three tables.
    pub fn clear_all(&self) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(
            "DELETE FROM chunks; DELETE FROM patterns_fts; DELETE FROM patterns_vec;",
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Delete all chunks belonging to a specific source file.
    ///
    /// Used for single-file re-indexing after writes.
    pub fn delete_by_source(&self, source_file: &str) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
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
        tx.commit()?;
        Ok(())
    }

    /// Insert a chunk (with optional embedding) into all three tables.
    ///
    /// Uses a transaction to ensure atomicity. Deletes any existing FTS5 and
    /// vec0 rows for this chunk ID first to prevent ghost rows on duplicate
    /// inserts.
    pub fn insert_chunk(&self, chunk: &Chunk, embedding: Option<&[f32]>) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        // Delete old FTS and vec rows if they exist to prevent ghost rows.
        tx.execute(
            "DELETE FROM patterns_fts WHERE chunk_id = ?1",
            params![chunk.id],
        )?;
        tx.execute("DELETE FROM patterns_vec WHERE id = ?1", params![chunk.id])?;

        tx.execute(
            "INSERT OR REPLACE INTO chunks (id, title, body, tags, source_file, heading_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                chunk.id,
                chunk.title,
                chunk.body,
                chunk.tags,
                chunk.source_file,
                chunk.heading_path,
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
                    bm25(patterns_fts, 10.0, 1.0, 5.0, 0.0) AS score
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
                    v.distance AS score
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

    /// Return one entry per source document (the shallowest chunk per file).
    ///
    /// Used by `lore list` to show a compact pattern index.
    pub fn list_patterns(&self) -> anyhow::Result<Vec<PatternSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_file, title, tags FROM chunks
             WHERE id IN (SELECT MIN(id) FROM chunks GROUP BY source_file)
             ORDER BY source_file",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(PatternSummary {
                source_file: row.get(0)?,
                title: row.get(1)?,
                tags: row.get(2)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
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
            "SELECT id, title, body, tags, source_file, heading_path, 0.0 AS score \
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
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Return aggregate statistics about the database.
    #[allow(clippy::cast_sign_loss)]
    pub fn stats(&self) -> anyhow::Result<DBStats> {
        let chunks: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        let sources: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT source_file) FROM chunks",
            [],
            |row| row.get(0),
        )?;
        Ok(DBStats {
            // COUNT(*) is always non-negative, so sign loss is not a concern.
            #[allow(clippy::cast_possible_truncation)]
            chunks: chunks as usize,
            #[allow(clippy::cast_possible_truncation)]
            sources: sources as usize,
        })
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
            '.' | '/' | '\\' | ':' | '{' | '}' | '[' | ']' | '"' | '\'' | '*' | '^' => ' ',
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

    /// Helper: build a `Chunk` for test use.
    fn make_chunk(id: &str, title: &str, body: &str, source: &str) -> Chunk {
        Chunk {
            id: id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            tags: String::new(),
            source_file: source.to_string(),
            heading_path: String::new(),
        }
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
        };
        let c2 = Chunk {
            id: "c2".into(),
            title: "Beta patterns".into(),
            body: "The beta pattern extends alpha concepts".into(),
            tags: "design".into(),
            source_file: "patterns.md".into(),
            heading_path: "Beta".into(),
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

        db.insert_chunk(
            &make_chunk("c1", "T1", "Body one content", "a.md"),
            Some(&[1.0, 0.0, 0.0, 0.0]),
        )
        .unwrap();
        db.insert_chunk(
            &make_chunk("c2", "T2", "Body two content", "b.md"),
            Some(&[0.0, 1.0, 0.0, 0.0]),
        )
        .unwrap();

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

        db.insert_chunk(&make_chunk("c1", "T1", "Body one content", "a.md"), None)
            .unwrap();
        db.insert_chunk(&make_chunk("c2", "T2", "Body two content", "a.md"), None)
            .unwrap();
        db.insert_chunk(&make_chunk("c3", "T3", "Body three content", "b.md"), None)
            .unwrap();

        let stats = db.stats().unwrap();
        assert_eq!(stats.chunks, 3);
        assert_eq!(stats.sources, 2);
    }

    // -- RRF unit test ----------------------------------------------------

    #[test]
    fn rrf_merges_two_ranked_lists() {
        let a = vec![
            SearchResult {
                id: "x".into(),
                title: String::new(),
                body: String::new(),
                tags: String::new(),
                source_file: String::new(),
                heading_path: String::new(),
                score: 0.0,
            },
            SearchResult {
                id: "y".into(),
                title: String::new(),
                body: String::new(),
                tags: String::new(),
                source_file: String::new(),
                heading_path: String::new(),
                score: 0.0,
            },
        ];

        let b = vec![
            SearchResult {
                id: "y".into(),
                title: String::new(),
                body: String::new(),
                tags: String::new(),
                source_file: String::new(),
                heading_path: String::new(),
                score: 0.0,
            },
            SearchResult {
                id: "z".into(),
                title: String::new(),
                body: String::new(),
                tags: String::new(),
                source_file: String::new(),
                heading_path: String::new(),
                score: 0.0,
            },
        ];

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

        // Two chunks from same source, one from another.
        db.insert_chunk(&make_chunk("c1", "Alpha", "Body A1", "alpha.md"), None)
            .unwrap();
        db.insert_chunk(&make_chunk("c2", "Alpha Sub", "Body A2", "alpha.md"), None)
            .unwrap();
        db.insert_chunk(&make_chunk("c3", "Beta", "Body B1", "beta.md"), None)
            .unwrap();

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
        };
        let c2 = Chunk {
            id: "c2".into(),
            title: "TypeScript Conventions".into(),
            body: "Prefer type over interface.".into(),
            tags: "typescript, conventions".into(),
            source_file: "ts.md".into(),
            heading_path: String::new(),
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
        };
        db.insert_chunk(&chunk, None).unwrap();

        let patterns = db.list_patterns().unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].tags, "rust, style");
    }
}
