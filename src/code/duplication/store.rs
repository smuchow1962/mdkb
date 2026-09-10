//! Content-addressed cache for the artifacts derived from a symbol's body.
//!
//! Its own database file, deliberately. Two facts about the code index rule out
//! keeping this beside it:
//!
//! * A symbol's id does not survive a reparse — the indexing pipeline carries
//!   vectors across on the *text* they were computed from, precisely because the
//!   id it is keyed by is gone (see `split_by_reuse`). Anything keyed on a
//!   symbol id is thrown away every time its file is touched.
//! * `code.sqlite` is disposable: every symbol re-derives from source, so a
//!   corrupt index is quarantined and rebuilt without ceremony. A vector that
//!   cost an ONNX pass does not re-derive cheaply and must not be inside
//!   something designed to be thrown away.
//!
//! So the key is the hash of the body text. It survives a reparse, a rebuild,
//! and a symbol moving between files — and two identical bodies anywhere in the
//! repository share one row and one embedding.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

/// The artifacts derived from one body, all cheap to recompute except the last.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyArtifacts {
    /// Rename- and literal-invariant structural fingerprint.
    pub simhash: u64,
    /// Named AST nodes in the body — the complexity filter.
    pub nodes: u32,
    /// `None` until the embedding pass has run for this body.
    pub embedding: Option<Vec<f32>>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS body_fingerprints (
    body_hash TEXT PRIMARY KEY,
    simhash   INTEGER NOT NULL,
    nodes     INTEGER NOT NULL,
    embedding BLOB,
    last_seen INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// The `meta` key naming what produced the embeddings in this file.
const EMBEDDING_SIGNATURE: &str = "embedding_signature";

/// The duplication side database.
#[derive(Debug)]
pub struct DupDb {
    conn: Connection,
    path: PathBuf,
}

impl DupDb {
    /// Open (creating if absent) the duplication cache at `path`.
    ///
    /// Same pragmas as the index DB (`Context::configure_connection`) and for the
    /// same reason: the daemon and a one-shot CLI open this file as independent
    /// connections, so `busy_timeout` — set first, before the schema statement
    /// that follows — makes ordinary write contention wait instead of failing
    /// with `SQLITE_BUSY`. `temp_store = memory` keeps [`Self::gc`]'s temp table
    /// off disk. No `foreign_keys`: this schema deliberately has none.
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "
            PRAGMA busy_timeout = 5000;
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA temp_store = memory;
            ",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn, path })
    }

    /// An in-memory cache, for tests.
    #[cfg(test)]
    pub fn in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn,
            path: PathBuf::from(":memory:"),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Record the structural artifacts of a body, refreshing `last_seen`.
    ///
    /// The embedding is left alone: it is the expensive column, and a reparse
    /// that yields the same body must not discard it. `simhash` and `nodes` are
    /// rewritten because a fingerprinting change has to be able to correct them.
    pub fn upsert_structural(
        &self,
        body_hash: &str,
        simhash: u64,
        nodes: u32,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO body_fingerprints (body_hash, simhash, nodes, last_seen) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(body_hash) DO UPDATE SET \
                 simhash = excluded.simhash, \
                 nodes = excluded.nodes, \
                 last_seen = excluded.last_seen",
            params![body_hash, to_i64(simhash), nodes, now],
        )?;
        Ok(())
    }

    /// Attach an embedding to a body already recorded by [`Self::upsert_structural`].
    ///
    /// Returns the number of rows updated: 0 means the body is not in the cache,
    /// which is a caller bug rather than a silent no-op worth ignoring.
    pub fn set_embedding(&self, body_hash: &str, embedding: &[f32]) -> rusqlite::Result<usize> {
        self.conn.execute(
            "UPDATE body_fingerprints SET embedding = ?2 WHERE body_hash = ?1",
            params![body_hash, encode(embedding)],
        )
    }

    /// Everything known about a body, or `None` if it has never been seen.
    pub fn get(&self, body_hash: &str) -> rusqlite::Result<Option<BodyArtifacts>> {
        self.conn
            .query_row(
                "SELECT simhash, nodes, embedding FROM body_fingerprints WHERE body_hash = ?1",
                params![body_hash],
                |row| {
                    let raw: i64 = row.get(0)?;
                    let blob: Option<Vec<u8>> = row.get(2)?;
                    Ok(BodyArtifacts {
                        simhash: from_i64(raw),
                        nodes: row.get(1)?,
                        embedding: blob.as_deref().and_then(decode),
                    })
                },
            )
            .optional()
    }

    /// Drop every embedding that `signature` did not produce, and record it.
    ///
    /// Returns how many were dropped. A vector is only comparable with vectors
    /// from the same model, at the same stored dimensions, computed over the
    /// same input cut — change any of the three and the cached vectors score
    /// against the new ones as noise, which reads as a duplication finding
    /// nobody can explain. Nothing else in this file would ever notice: the key
    /// is the body hash, and the body did not change.
    ///
    /// The simhashes stay. They are structural, cost no model, and are what the
    /// pass falls back on — throwing them away would turn a model change into a
    /// full re-fingerprint of the repository for no reason.
    ///
    /// A cache written before this table existed has no signature, so the first
    /// run after the upgrade clears every embedding. That is the honest reading:
    /// what produced them was never recorded, so they cannot be vouched for.
    pub fn reconcile_embedding_signature(&self, signature: &str) -> rusqlite::Result<usize> {
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![EMBEDDING_SIGNATURE],
                |r| r.get(0),
            )
            .optional()?;
        if stored.as_deref() == Some(signature) {
            return Ok(0);
        }
        let cleared = self.conn.execute(
            "UPDATE body_fingerprints SET embedding = NULL WHERE embedding IS NOT NULL",
            [],
        )?;
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![EMBEDDING_SIGNATURE, signature],
        )?;
        Ok(cleared)
    }

    /// Number of cached bodies.
    pub fn count(&self) -> rusqlite::Result<usize> {
        self.conn
            .query_row("SELECT COUNT(*) FROM body_fingerprints", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|n| n as usize)
    }

    /// Drop every body not in `live`, returning how many were removed.
    ///
    /// Content addressing means rows accumulate for every version of every body
    /// the repository has ever held. Nothing else will ever delete them.
    pub fn gc(&self, live: &[String]) -> rusqlite::Result<usize> {
        if live.is_empty() {
            return self.conn.execute("DELETE FROM body_fingerprints", []);
        }
        // Chunked to stay under SQLITE_MAX_VARIABLE_NUMBER: the live set is one
        // entry per candidate symbol, which is unbounded in a large repository.
        let mut keep_stmt = self
            .conn
            .prepare("CREATE TEMP TABLE IF NOT EXISTS dup_live (body_hash TEXT PRIMARY KEY)")?;
        keep_stmt.execute([])?;
        self.conn.execute("DELETE FROM dup_live", [])?;
        {
            let mut insert = self
                .conn
                .prepare("INSERT OR IGNORE INTO dup_live (body_hash) VALUES (?1)")?;
            for hash in live {
                insert.execute(params![hash])?;
            }
        }
        let removed = self.conn.execute(
            "DELETE FROM body_fingerprints \
             WHERE body_hash NOT IN (SELECT body_hash FROM dup_live)",
            [],
        )?;
        self.conn.execute("DELETE FROM dup_live", [])?;
        Ok(removed)
    }
}

/// SQLite integers are signed; a simhash uses all 64 bits.
///
/// The round trip is a bit-cast in both directions, never a numeric conversion:
/// `rusqlite`'s `fallible_uint` feature is enabled, so reading a negative value
/// back as `u64` would fail rather than silently wrap — which is the right
/// behaviour, and the reason the cast has to be explicit here.
const fn to_i64(v: u64) -> i64 {
    v as i64
}

const fn from_i64(v: i64) -> u64 {
    v as u64
}

/// f32 vector as little-endian bytes.
fn encode(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

/// Little-endian bytes back to an f32 vector.
///
/// A blob whose length is not a multiple of four is not an embedding this code
/// wrote; it is decoded as absent rather than as a truncated vector, because a
/// silently short vector would score against every other body as garbage.
fn decode(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    Some(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_round_trips_its_structural_artifacts() {
        let db = DupDb::in_memory().unwrap();
        db.upsert_structural("abc", 42, 7, 1000).unwrap();

        let got = db.get("abc").unwrap().expect("recorded");
        assert_eq!(got.simhash, 42);
        assert_eq!(got.nodes, 7);
        assert_eq!(got.embedding, None, "no embedding written yet");
    }

    #[test]
    fn a_simhash_with_the_high_bit_set_survives_the_signed_column() {
        // SQLite has no unsigned integer. Without the deliberate bit-cast this
        // value comes back as an error (fallible_uint) or as a different number.
        let db = DupDb::in_memory().unwrap();
        let hash = u64::MAX - 1;
        db.upsert_structural("high", hash, 3, 1).unwrap();

        assert_eq!(db.get("high").unwrap().unwrap().simhash, hash);
    }

    #[test]
    fn upserting_the_same_body_twice_leaves_one_row() {
        let db = DupDb::in_memory().unwrap();
        db.upsert_structural("same", 1, 1, 10).unwrap();
        db.upsert_structural("same", 2, 2, 20).unwrap();

        assert_eq!(db.count().unwrap(), 1);
        let got = db.get("same").unwrap().unwrap();
        assert_eq!(got.simhash, 2, "the newer fingerprint wins");
        assert_eq!(got.nodes, 2);
    }

    #[test]
    fn an_upsert_does_not_discard_the_embedding_it_already_has() {
        // The whole point of the cache: a reparse yielding the same body must
        // not cost another ONNX pass.
        let db = DupDb::in_memory().unwrap();
        db.upsert_structural("keep", 1, 5, 10).unwrap();
        db.set_embedding("keep", &[0.25, 0.5]).unwrap();

        db.upsert_structural("keep", 1, 5, 99).unwrap();

        assert_eq!(
            db.get("keep").unwrap().unwrap().embedding,
            Some(vec![0.25, 0.5])
        );
    }

    #[test]
    fn an_embedding_round_trips_through_the_blob() {
        let db = DupDb::in_memory().unwrap();
        db.upsert_structural("vec", 1, 1, 1).unwrap();
        let v = vec![-1.5f32, 0.0, 2.25, 0.125];

        assert_eq!(db.set_embedding("vec", &v).unwrap(), 1);
        assert_eq!(db.get("vec").unwrap().unwrap().embedding, Some(v));
    }

    #[test]
    fn embedding_an_unknown_body_updates_nothing() {
        let db = DupDb::in_memory().unwrap();
        assert_eq!(db.set_embedding("ghost", &[1.0]).unwrap(), 0);
    }

    #[test]
    fn an_unknown_body_is_none_not_an_error() {
        let db = DupDb::in_memory().unwrap();
        assert_eq!(db.get("never-seen").unwrap(), None);
    }

    #[test]
    fn gc_removes_what_the_live_set_omits_and_keeps_the_rest() {
        let db = DupDb::in_memory().unwrap();
        db.upsert_structural("live1", 1, 1, 1).unwrap();
        db.upsert_structural("live2", 2, 2, 1).unwrap();
        db.upsert_structural("stale", 3, 3, 1).unwrap();

        let removed = db.gc(&["live1".into(), "live2".into()]).unwrap();

        assert_eq!(removed, 1);
        assert_eq!(db.count().unwrap(), 2);
        assert!(db.get("stale").unwrap().is_none());
        assert!(db.get("live1").unwrap().is_some());
    }

    #[test]
    fn gc_with_an_empty_live_set_clears_the_cache() {
        let db = DupDb::in_memory().unwrap();
        db.upsert_structural("a", 1, 1, 1).unwrap();
        db.upsert_structural("b", 2, 2, 1).unwrap();

        assert_eq!(db.gc(&[]).unwrap(), 2);
        assert_eq!(db.count().unwrap(), 0);
    }

    #[test]
    fn a_blob_that_is_not_a_vector_decodes_as_absent() {
        // Three bytes cannot be f32s. Decoding it as a one-element vector would
        // score as garbage against every other body.
        let db = DupDb::in_memory().unwrap();
        db.upsert_structural("bad", 1, 1, 1).unwrap();
        db.conn
            .execute(
                "UPDATE body_fingerprints SET embedding = ?1 WHERE body_hash = 'bad'",
                params![vec![1u8, 2, 3]],
            )
            .unwrap();

        assert_eq!(db.get("bad").unwrap().unwrap().embedding, None);
    }

    #[test]
    fn a_changed_signature_nulls_the_embeddings_and_keeps_the_simhashes() {
        // The order a real run uses: reconcile first, then embed. So the cache
        // below is one a previous run at 512 tokens legitimately vouched for.
        let db = DupDb::in_memory().unwrap();
        db.upsert_structural("a", 11, 5, 1).unwrap();
        db.upsert_structural("b", 22, 6, 1).unwrap();
        assert_eq!(db.reconcile_embedding_signature("Jina:256:512").unwrap(), 0);
        db.set_embedding("a", &[1.0, 0.0]).unwrap();
        db.set_embedding("b", &[0.0, 1.0]).unwrap();

        // Same bodies, same hashes — only the cut changed.
        let cleared = db.reconcile_embedding_signature("Jina:256:256").unwrap();

        assert_eq!(cleared, 2, "both vectors are stale");
        assert_eq!(db.get("a").unwrap().unwrap().embedding, None);
        assert_eq!(db.get("b").unwrap().unwrap().embedding, None);
        assert_eq!(
            db.get("a").unwrap().unwrap().simhash,
            11,
            "the structural half costs no model and must survive"
        );
        assert_eq!(db.get("b").unwrap().unwrap().nodes, 6);
        assert_eq!(db.count().unwrap(), 2, "no row was deleted");
    }

    #[test]
    fn the_same_signature_leaves_every_embedding_alone() {
        // The common case: the second run of an unchanged configuration must
        // not throw away the pass the first one paid for.
        let db = DupDb::in_memory().unwrap();
        db.upsert_structural("a", 1, 5, 1).unwrap();
        db.reconcile_embedding_signature("Jina:256:256").unwrap();
        db.set_embedding("a", &[0.5, 0.5]).unwrap();

        let cleared = db.reconcile_embedding_signature("Jina:256:256").unwrap();

        assert_eq!(cleared, 0);
        assert_eq!(
            db.get("a").unwrap().unwrap().embedding,
            Some(vec![0.5, 0.5])
        );
    }

    #[test]
    fn a_cache_written_before_the_signature_existed_is_cleared_once() {
        // An upgrade meets rows whose provenance was never recorded. They are
        // dropped on the first reconcile and the second one is a no-op, so the
        // upgrade costs one re-embed and not one per run.
        let db = DupDb::in_memory().unwrap();
        db.upsert_structural("legacy", 9, 5, 1).unwrap();
        db.set_embedding("legacy", &[1.0]).unwrap();

        assert_eq!(db.reconcile_embedding_signature("Jina:256:256").unwrap(), 1);
        assert_eq!(db.reconcile_embedding_signature("Jina:256:256").unwrap(), 0);
    }

    #[test]
    fn the_signature_survives_being_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dup.sqlite");
        {
            let db = DupDb::open(&path).unwrap();
            db.upsert_structural("keep", 1, 5, 1).unwrap();
            db.reconcile_embedding_signature("Jina:256:256").unwrap();
            db.set_embedding("keep", &[1.0]).unwrap();
        }

        let reopened = DupDb::open(&path).unwrap();

        assert_eq!(
            reopened
                .reconcile_embedding_signature("Jina:256:256")
                .unwrap(),
            0,
            "a reopened cache must not re-clear what it already vouched for"
        );
        assert_eq!(
            reopened.get("keep").unwrap().unwrap().embedding,
            Some(vec![1.0])
        );
    }

    #[test]
    fn a_file_backed_cache_waits_out_a_concurrent_writer() {
        // Two processes (daemon + one-shot CLI) open this file independently.
        // Without busy_timeout the second writer fails immediately on the first
        // write lock it meets, which would abort an indexing run.
        let dir = tempfile::tempdir().unwrap();
        let db = DupDb::open(dir.path().join("dup.sqlite")).unwrap();

        let timeout: i64 = db
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        let journal: String = db
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();

        assert_eq!(timeout, 5000);
        assert_eq!(journal, "wal");
    }

    #[test]
    fn a_file_backed_cache_survives_being_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dup.sqlite");
        {
            let db = DupDb::open(&path).unwrap();
            db.upsert_structural("persist", 7, 7, 7).unwrap();
            db.set_embedding("persist", &[1.0, 2.0]).unwrap();
        }

        let reopened = DupDb::open(&path).unwrap();
        let got = reopened.get("persist").unwrap().unwrap();
        assert_eq!(got.simhash, 7);
        assert_eq!(got.embedding, Some(vec![1.0, 2.0]));
    }
}
