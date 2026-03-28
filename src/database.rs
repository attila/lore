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

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl KnowledgeDB {
    /// Open (or create) a database at `db_path` configured for `dimensions`-wide
    /// embeddings.
    pub fn open(db_path: &Path, dimensions: usize) -> anyhow::Result<Self> {
        register_sqlite_vec();

        let conn = Connection::open(db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

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
            )",
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
        self.conn.execute_batch(
            "DELETE FROM chunks; DELETE FROM patterns_fts; DELETE FROM patterns_vec;",
        )?;
        Ok(())
    }

    /// Delete all chunks belonging to a specific source file.
    ///
    /// Used for single-file re-indexing after writes.
    pub fn delete_by_source(&self, source_file: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "DELETE FROM patterns_fts WHERE chunk_id IN \
             (SELECT id FROM chunks WHERE source_file = ?1)",
            params![source_file],
        )?;
        self.conn.execute(
            "DELETE FROM patterns_vec WHERE id IN \
             (SELECT id FROM chunks WHERE source_file = ?1)",
            params![source_file],
        )?;
        self.conn.execute(
            "DELETE FROM chunks WHERE source_file = ?1",
            params![source_file],
        )?;
        Ok(())
    }

    /// Insert a chunk (with optional embedding) into all three tables.
    pub fn insert_chunk(&self, chunk: &Chunk, embedding: Option<&[f32]>) -> anyhow::Result<()> {
        self.conn.execute(
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

        self.conn.execute(
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
            self.conn.execute(
                "INSERT INTO patterns_vec (id, embedding) VALUES (?1, ?2)",
                params![chunk.id, blob],
            )?;
        }

        Ok(())
    }

    /// Full-text search via FTS5. Returns results ordered by BM25 rank.
    pub fn search_fts(&self, query: &str, limit: usize) -> anyhow::Result<Vec<SearchResult>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.title, c.body, c.tags, c.source_file, c.heading_path,
                    rank AS score
             FROM patterns_fts f
             JOIN chunks c ON c.id = f.chunk_id
             WHERE patterns_fts MATCH ?1
             ORDER BY rank
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

    let mut merged: Vec<_> = scores.into_values().collect();
    merged.sort_by(|a, b| b.1.total_cmp(&a.1));

    merged
        .into_iter()
        .take(limit)
        .map(|(mut r, s)| {
            r.score = s;
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

        // "y" gets rank-1 from list_a (1/(60+1+1) = 1/62) plus
        // rank-0 from list_b (1/(60+0+1) = 1/61).
        let expected_y = 1.0 / 62.0 + 1.0 / 61.0;
        assert!((merged[0].score - expected_y).abs() < 1e-10);
    }

    // -- vec_to_blob round-trip -------------------------------------------

    #[test]
    fn vec_to_blob_round_trip() {
        let original = vec![1.0_f32, -2.5, 0.0, std::f32::consts::PI];
        let blob = vec_to_blob(&original);
        let recovered = blob_to_vec(&blob);
        assert_eq!(original, recovered);
    }
}
