//! The semantic pass: a second model, run lazily, over what the structural
//! pass could not settle.
//!
//! A second [`TextEmbedding`], never `llm::get_cached_service()`. That service
//! is one global instance shared by documents, memory and code search, and the
//! vectors it produced are on disk in `vectors.bin`, `vec_documents` and
//! `vec_memory`. Repointing it at a code model would invalidate all three and
//! force a re-embed of everything ever indexed — to serve a command that runs
//! on demand.
//!
//! It also has to be a different model. Measured on the case set in
//! `src/eval/embedding_gap.rs`, `AllMiniLML6V2` scores duplication pairs
//! *lower* than the adversarial pairs built to fool it (gap -0.0376): on this
//! task it is not weak, it is inverted. `JinaEmbeddingsV2BaseCode` truncated to
//! 256 dimensions scores +0.1503 — better than its own 768 dimensions, and a
//! third of the storage.

use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use super::store::DupDb;
use crate::error::{Error, Result};

/// Models the duplication pass accepts.
///
/// A short list on purpose: an embedding model that has not been measured on
/// the gap metric is a guess, and a guess here silently produces a report of
/// the wrong pairs rather than an error anyone would notice.
pub const SUPPORTED_DUP_MODELS: &[&str] = &["JinaEmbeddingsV2BaseCode", "AllMiniLML6V2"];

/// The model the gate chose.
pub const DEFAULT_DUP_MODEL: &str = "JinaEmbeddingsV2BaseCode";

/// Stored dimensions per body.
///
/// 256, not the model's native 768, because 256 measured *better*: gap 0.1503
/// against 0.1349. Matryoshka training front-loads the signal, and the tail
/// dimensions carry more noise than meaning for this comparison. Storage is a
/// third, which for a per-body cache over a large repository is the difference
/// between a megabyte and three.
pub const DUP_EMBEDDING_DIM: usize = 256;

/// Batch size for ONNX inference, matching `llm::embeddings`: larger batches
/// let rayon run several inferences at once, each allocating an ORT arena that
/// is never freed.
const EMBED_BATCH_SIZE: usize = 32;

/// Anything that can turn body texts into vectors.
///
/// A trait so the cache logic — which bodies to embed, which to leave alone —
/// is testable without downloading 160M parameters. Everything below the model
/// boundary is exercised in CI; only the model itself is `#[ignore]`d.
pub trait BodyEmbedder: Send + Sync {
    /// One vector per text, in order, already truncated and L2-normalised.
    fn embed_bodies(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

/// Scale `v` to unit length in place.
///
/// Done once when a vector is produced, so scoring a pair is a dot product
/// rather than a dot product and two square roots. Over n² pairs that is the
/// difference between the scoring pass mattering and not.
///
/// A zero vector is left alone: there is no direction to normalise it to, and
/// dividing would give NaN, which compares false against every threshold and
/// would drop the pair silently instead of scoring it 0.
pub fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return;
    }
    for x in v.iter_mut() {
        *x /= norm;
    }
}

/// Cosine similarity of two vectors already normalised by [`normalize`].
///
/// Length mismatch scores 0 rather than panicking: a cache written by an
/// earlier `DUP_EMBEDDING_DIM` is stale data, not a reason to abort a scan.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Fill the embedding of every body that does not have one yet.
///
/// Returns how many were embedded. A body already in the cache is skipped: that
/// is the entire reason the cache is keyed on body content — the same function
/// copied into three files is one ONNX pass, and an unrelated edit elsewhere in
/// the file costs nothing.
///
/// `bodies` is `(body_hash, body_text)`.
pub fn embed_missing(
    dup: &DupDb,
    bodies: &[(String, String)],
    embedder: &dyn BodyEmbedder,
) -> Result<usize> {
    let mut pending: Vec<(&str, &str)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (hash, text) in bodies {
        // Two candidates can share a body hash — that is the point of content
        // addressing — and embedding it twice in one batch would be waste.
        if !seen.insert(hash.as_str()) {
            continue;
        }
        let cached = dup
            .get(hash)
            .map_err(|e| Error::other(format!("duplication cache read failed: {e}")))?;
        if cached.is_some_and(|a| a.embedding.is_some()) {
            continue;
        }
        pending.push((hash.as_str(), text.as_str()));
    }
    if pending.is_empty() {
        return Ok(0);
    }

    let texts: Vec<&str> = pending.iter().map(|(_, t)| *t).collect();
    let vectors = embedder.embed_bodies(&texts)?;
    if vectors.len() != pending.len() {
        return Err(Error::other(format!(
            "embedder returned {} vectors for {} bodies",
            vectors.len(),
            pending.len()
        )));
    }
    for ((hash, _), vector) in pending.iter().zip(&vectors) {
        dup.set_embedding(hash, vector)
            .map_err(|e| Error::other(format!("duplication cache write failed: {e}")))?;
    }
    Ok(pending.len())
}

/// Index pairs the structural pass left unresolved.
///
/// `clusters` holds the groups it did settle; anything already grouped together
/// is a finding, and re-scoring it would only risk a model overruling a
/// certainty. Pairs are emitted with the lower index first.
pub fn unclustered_pairs(n: usize, clusters: &[Vec<usize>]) -> Vec<(usize, usize)> {
    let mut group_of = vec![usize::MAX; n];
    for (id, group) in clusters.iter().enumerate() {
        for &i in group {
            if i < n {
                group_of[i] = id;
            }
        }
    }
    let mut pairs = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            // usize::MAX means ungrouped, and two ungrouped candidates are not
            // in the same group — so the equality test has to exclude it.
            if group_of[i] != usize::MAX && group_of[i] == group_of[j] {
                continue;
            }
            pairs.push((i, j));
        }
    }
    pairs
}

/// Score pairs and keep those at or above `threshold`.
///
/// Under rayon because it is the one part of the pass that is embarrassingly
/// parallel and n² — the embedding itself is n and deliberately serial, since
/// nesting rayon inside ONNX Runtime's own pool burns every core without making
/// progress (see `llm::embeddings::cap_rayon_global_pool`).
///
/// A pair whose vector is missing scores nothing rather than 0: absent is not
/// "dissimilar", and recording it as such would tell the reader the model
/// judged something it never saw.
pub fn score_pairs(
    vectors: &[Option<Vec<f32>>],
    pairs: &[(usize, usize)],
    threshold: f32,
) -> Vec<(usize, usize, f32)> {
    let mut scored: Vec<(usize, usize, f32)> = pairs
        .par_iter()
        .filter_map(|&(i, j)| {
            let a = vectors.get(i)?.as_ref()?;
            let b = vectors.get(j)?.as_ref()?;
            let score = dot(a, b);
            (score >= threshold).then_some((i, j, score))
        })
        .collect();
    // Deterministic order: rayon's collect is order-preserving over the input,
    // but the report is read by a human and the strongest pair belongs first.
    scored.sort_unstable_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    scored
}

/// Load one vector per body hash, in the caller's order.
///
/// Normalised here as well as at write time, because that is what makes the
/// dot product in [`score_pairs`] a cosine. Doing it once per body at load is n
/// square roots; doing it inside the scorer would be n² of them.
///
/// A hash with no embedding yields `None`, which [`score_pairs`] skips.
pub fn load_vectors(dup: &DupDb, body_hashes: &[String]) -> Result<Vec<Option<Vec<f32>>>> {
    let mut out = Vec::with_capacity(body_hashes.len());
    for hash in body_hashes {
        let vector = dup
            .get(hash)
            .map_err(|e| Error::other(format!("duplication cache read failed: {e}")))?
            .and_then(|a| a.embedding)
            .map(|mut v| {
                normalize(&mut v);
                v
            });
        out.push(vector);
    }
    Ok(out)
}

/// The whole semantic pass: embed what is missing, then score what the
/// structural pass left open.
///
/// `bodies` is `(body_hash, body_text)` in candidate order, so an index in the
/// returned triples is an index into the caller's candidate list.
pub fn score_semantic_tail(
    dup: &DupDb,
    bodies: &[(String, String)],
    clusters: &[Vec<usize>],
    embedder: &dyn BodyEmbedder,
    threshold: f32,
) -> Result<Vec<(usize, usize, f32)>> {
    let pairs = unclustered_pairs(bodies.len(), clusters);
    if pairs.is_empty() {
        // Nothing to score means nothing to embed: the model is not loaded at
        // all when the structural pass already settled everything.
        return Ok(Vec::new());
    }
    embed_missing(dup, bodies, embedder)?;
    let hashes: Vec<String> = bodies.iter().map(|(h, _)| h.clone()).collect();
    let vectors = load_vectors(dup, &hashes)?;
    Ok(score_pairs(&vectors, &pairs, threshold))
}

// --- the model itself ---

/// A `fastembed` model dedicated to duplication, truncated to
/// [`DUP_EMBEDDING_DIM`].
pub struct DupEmbedder {
    model: fastembed::TextEmbedding,
    name: String,
}

impl std::fmt::Debug for DupEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DupEmbedder")
            .field("model", &self.name)
            .field("dimension", &DUP_EMBEDDING_DIM)
            .finish()
    }
}

impl DupEmbedder {
    /// Build the model named by `[code.duplication] model`.
    pub fn new(name: &str) -> Result<Self> {
        let variant = fastembed_model(name)?;
        let model = fastembed::TextEmbedding::try_new(
            fastembed::InitOptions::new(variant)
                .with_cache_dir(dup_cache_dir())
                .with_show_download_progress(true),
        )
        .map_err(|e| Error::other(format!("failed to initialise duplication model {name}: {e}")))?;
        Ok(Self {
            model,
            name: name.to_string(),
        })
    }

    pub fn model_name(&self) -> &str {
        &self.name
    }
}

impl BodyEmbedder for DupEmbedder {
    fn embed_bodies(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let raw = self
            .model
            .embed(texts.to_vec(), Some(EMBED_BATCH_SIZE))
            .map_err(|e| Error::other(format!("failed to embed bodies: {e}")))?;
        Ok(raw
            .into_iter()
            .map(|mut v| {
                // Truncate BEFORE normalising: a Matryoshka prefix is only a
                // unit vector after it is rescaled, and scoring an unnormalised
                // prefix with a dot product would read as a lower similarity
                // the shorter the vector.
                v.truncate(DUP_EMBEDDING_DIM);
                normalize(&mut v);
                v
            })
            .collect())
    }
}

fn fastembed_model(name: &str) -> Result<fastembed::EmbeddingModel> {
    match name {
        "JinaEmbeddingsV2BaseCode" => Ok(fastembed::EmbeddingModel::JinaEmbeddingsV2BaseCode),
        "AllMiniLML6V2" => Ok(fastembed::EmbeddingModel::AllMiniLML6V2),
        other => Err(Error::other(format!(
            "unsupported duplication model {other}; expected one of: {}",
            SUPPORTED_DUP_MODELS.join(", ")
        ))),
    }
}

/// Same on-disk model cache as `llm::embeddings` — the weights are shared even
/// though the services are not.
fn dup_cache_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("FASTEMBED_CACHE_DIR") {
        return std::path::PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("HOME") {
        return std::path::PathBuf::from(home).join(".cache/fastembed");
    }
    std::path::PathBuf::from(".fastembed_cache")
}

/// The duplication model, built once per process.
///
/// A second cache beside `llm::CACHED_SERVICE`, never the same one: releasing
/// the document service to reclaim ORT arena memory must not take the
/// duplication model with it, and vice versa.
static DUP_SERVICE: Mutex<Option<(String, Arc<DupEmbedder>)>> = Mutex::new(None);

/// Get or build the duplication embedder for `name`.
///
/// Rebuilds if the configured model changed, which only happens when a caller
/// edits the config between runs in one process — in practice, tests.
pub fn get_dup_embedder(name: &str) -> Result<Arc<DupEmbedder>> {
    {
        let guard = DUP_SERVICE
            .lock()
            .map_err(|_| Error::other("duplication embedder cache lock poisoned"))?;
        if let Some((cached, service)) = guard.as_ref() {
            if cached == name {
                return Ok(Arc::clone(service));
            }
        }
    }
    // Built outside the lock: the first call may download the weights.
    let service = Arc::new(DupEmbedder::new(name)?);
    let mut guard = DUP_SERVICE
        .lock()
        .map_err(|_| Error::other("duplication embedder cache lock poisoned"))?;
    *guard = Some((name.to_string(), Arc::clone(&service)));
    Ok(service)
}

/// Drop the duplication model, freeing its ONNX arena.
pub fn release_dup_embedder() {
    if let Ok(mut guard) = DUP_SERVICE.lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts what it was asked to embed, so a test can prove the cache spared
    /// the model rather than merely producing the same answer.
    struct Counting {
        calls: AtomicUsize,
        texts: AtomicUsize,
    }

    impl Counting {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                texts: AtomicUsize::new(0),
            }
        }
    }

    impl BodyEmbedder for Counting {
        fn embed_bodies(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.texts.fetch_add(texts.len(), Ordering::SeqCst);
            Ok(texts
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let mut v = vec![0.0f32; 4];
                    v[i % 4] = 1.0;
                    v
                })
                .collect())
        }
    }

    fn seeded(hashes: &[&str]) -> DupDb {
        let db = DupDb::in_memory().unwrap();
        for (i, h) in hashes.iter().enumerate() {
            db.upsert_structural(h, i as u64, 50, 1).unwrap();
        }
        db
    }

    #[test]
    fn a_body_with_no_embedding_is_embedded() {
        let db = seeded(&["h1", "h2"]);
        let embedder = Counting::new();

        let n = embed_missing(
            &db,
            &[
                ("h1".into(), "fn a() {}".into()),
                ("h2".into(), "fn b() {}".into()),
            ],
            &embedder,
        )
        .unwrap();

        assert_eq!(n, 2);
        assert_eq!(embedder.texts.load(Ordering::SeqCst), 2);
        assert!(db.get("h1").unwrap().unwrap().embedding.is_some());
    }

    #[test]
    fn a_body_already_embedded_is_not_embedded_again() {
        let db = seeded(&["h1", "h2"]);
        db.set_embedding("h1", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        let embedder = Counting::new();

        let n = embed_missing(
            &db,
            &[
                ("h1".into(), "fn a() {}".into()),
                ("h2".into(), "fn b() {}".into()),
            ],
            &embedder,
        )
        .unwrap();

        assert_eq!(n, 1, "only the one that was missing");
        assert_eq!(embedder.texts.load(Ordering::SeqCst), 1);
        assert_eq!(
            db.get("h1").unwrap().unwrap().embedding,
            Some(vec![1.0, 0.0, 0.0, 0.0]),
            "the existing vector is untouched"
        );
    }

    #[test]
    fn one_body_shared_by_two_candidates_is_embedded_once() {
        // The whole point of content addressing: the same function copied into
        // three files costs one ONNX pass.
        let db = seeded(&["same"]);
        let embedder = Counting::new();

        let n = embed_missing(
            &db,
            &[
                ("same".into(), "fn a() {}".into()),
                ("same".into(), "fn a() {}".into()),
                ("same".into(), "fn a() {}".into()),
            ],
            &embedder,
        )
        .unwrap();

        assert_eq!(n, 1);
        assert_eq!(embedder.texts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn nothing_to_embed_does_not_touch_the_model() {
        let db = seeded(&["h1"]);
        db.set_embedding("h1", &[1.0]).unwrap();
        let embedder = Counting::new();

        assert_eq!(
            embed_missing(&db, &[("h1".into(), "fn a() {}".into())], &embedder).unwrap(),
            0
        );
        assert_eq!(
            embedder.calls.load(Ordering::SeqCst),
            0,
            "the model is not even called"
        );
    }

    /// An embedder returning the wrong count would otherwise pair vectors with
    /// the wrong bodies — every subsequent score would be about the wrong code.
    #[test]
    fn a_short_batch_from_the_embedder_is_an_error_not_a_misalignment() {
        struct Short;
        impl BodyEmbedder for Short {
            fn embed_bodies(&self, _: &[&str]) -> Result<Vec<Vec<f32>>> {
                Ok(vec![vec![1.0]])
            }
        }
        let db = seeded(&["h1", "h2"]);

        let err = embed_missing(
            &db,
            &[("h1".into(), "a".into()), ("h2".into(), "b".into())],
            &Short,
        )
        .unwrap_err();

        assert!(format!("{err}").contains("1 vectors for 2 bodies"), "{err}");
    }

    #[test]
    fn normalising_gives_a_unit_vector() {
        let mut v = vec![3.0f32, 4.0];
        normalize(&mut v);

        assert!((v[0] - 0.6).abs() < 1e-6, "{v:?}");
        assert!((v[1] - 0.8).abs() < 1e-6, "{v:?}");
        assert!((dot(&v, &v) - 1.0).abs() < 1e-6, "unit length");
    }

    #[test]
    fn a_zero_vector_is_left_alone_rather_than_made_nan() {
        let mut v = vec![0.0f32; 4];
        normalize(&mut v);

        assert_eq!(v, vec![0.0; 4]);
        assert!(dot(&v, &v).is_finite(), "a NaN would drop the pair silently");
    }

    #[test]
    fn vectors_of_different_lengths_score_zero_not_a_panic() {
        // A cache written by an earlier DUP_EMBEDDING_DIM is stale data, not a
        // reason to abort a scan.
        assert!(dot(&[1.0, 0.0], &[1.0, 0.0, 0.0]).abs() < f32::EPSILON);
    }

    #[test]
    fn only_pairs_the_structural_pass_left_alone_are_scored() {
        // 0 and 1 already cluster; every other pair is still open.
        let pairs = unclustered_pairs(4, &[vec![0, 1]]);

        assert_eq!(pairs, [(0, 2), (0, 3), (1, 2), (1, 3), (2, 3)]);
    }

    #[test]
    fn two_candidates_in_different_clusters_are_still_scored() {
        let pairs = unclustered_pairs(4, &[vec![0, 1], vec![2, 3]]);

        assert_eq!(pairs, [(0, 2), (0, 3), (1, 2), (1, 3)]);
    }

    #[test]
    fn two_ungrouped_candidates_are_not_treated_as_one_group() {
        // Both carry the "no group" marker. Comparing markers for equality
        // without excluding it would skip every pair the structural pass left,
        // which is exactly the set this pass exists to score.
        assert_eq!(unclustered_pairs(2, &[]), [(0, 1)]);
    }

    #[test]
    fn a_cluster_holding_everything_leaves_nothing_to_score() {
        assert!(unclustered_pairs(3, &[vec![0, 1, 2]]).is_empty());
    }

    #[test]
    fn scoring_keeps_pairs_at_or_above_the_threshold_strongest_first() {
        let a = vec![1.0f32, 0.0];
        let close = vec![0.9487f32, 0.3162]; // ~0.9487 against `a`
        let far = vec![0.0f32, 1.0];
        let vectors = vec![Some(a), Some(close), Some(far)];

        let scored = score_pairs(&vectors, &[(0, 1), (0, 2), (1, 2)], 0.3);

        assert_eq!(scored.len(), 2, "the orthogonal pair is below 0.3");
        assert_eq!((scored[0].0, scored[0].1), (0, 1));
        assert!(scored[0].2 > scored[1].2, "strongest first");
    }

    #[test]
    fn a_pair_exactly_at_the_threshold_is_kept() {
        let vectors = vec![Some(vec![1.0f32, 0.0]), Some(vec![1.0f32, 0.0])];

        assert_eq!(score_pairs(&vectors, &[(0, 1)], 1.0).len(), 1);
    }

    #[test]
    fn a_pair_whose_vector_is_missing_is_not_scored_as_dissimilar() {
        // Absent is not "dissimilar". Recording it as 0 would tell the reader
        // the model judged code it never saw.
        let vectors = vec![Some(vec![1.0f32, 0.0]), None];

        assert!(score_pairs(&vectors, &[(0, 1)], -1.0).is_empty());
    }

    #[test]
    fn loading_normalises_so_the_dot_product_is_a_cosine() {
        let db = seeded(&["h1"]);
        // Written unnormalised on purpose: a cache from an older writer, or a
        // model that did not normalise, must still score as a cosine.
        db.set_embedding("h1", &[3.0, 4.0]).unwrap();

        let loaded = load_vectors(&db, &["h1".to_string()]).unwrap();

        let v = loaded[0].as_ref().unwrap();
        assert!((dot(v, v) - 1.0).abs() < 1e-6, "{v:?}");
    }

    #[test]
    fn loading_a_body_with_no_embedding_gives_none_in_place() {
        // Position matters: the index into the returned vec is the candidate
        // index. A missing body must leave a hole, not shorten the list.
        let db = seeded(&["h1", "h2"]);
        db.set_embedding("h2", &[1.0]).unwrap();

        let loaded = load_vectors(&db, &["h1".to_string(), "h2".to_string()]).unwrap();

        assert_eq!(loaded.len(), 2);
        assert!(loaded[0].is_none());
        assert!(loaded[1].is_some());
    }

    #[test]
    fn the_semantic_pass_scores_the_pairs_the_structural_pass_left() {
        let db = seeded(&["h0", "h1", "h2"]);
        let bodies: Vec<(String, String)> = ["h0", "h1", "h2"]
            .iter()
            .map(|h| ((*h).to_string(), format!("fn {h}() {{}}")))
            .collect();
        // Written by hand, all identical: what is under test is which pairs
        // reach the scorer, not what the model thinks of them.
        db.set_embedding("h0", &[1.0, 0.0]).unwrap();
        db.set_embedding("h1", &[1.0, 0.0]).unwrap();
        db.set_embedding("h2", &[1.0, 0.0]).unwrap();
        let embedder = Counting::new();

        // 0 and 1 already cluster structurally, so only pairs touching 2 remain.
        let scored = score_semantic_tail(&db, &bodies, &[vec![0, 1]], &embedder, 0.5).unwrap();

        assert_eq!(embedder.calls.load(Ordering::SeqCst), 0, "all cached");
        assert_eq!(
            scored.iter().map(|&(i, j, _)| (i, j)).collect::<Vec<_>>(),
            [(0, 2), (1, 2)],
            "the 0-1 pair is a structural finding already"
        );
    }

    #[test]
    fn a_fully_clustered_candidate_set_never_loads_the_model() {
        let db = seeded(&["h0", "h1"]);
        let bodies = vec![
            ("h0".to_string(), "fn a() {}".to_string()),
            ("h1".to_string(), "fn b() {}".to_string()),
        ];
        let embedder = Counting::new();

        let scored = score_semantic_tail(&db, &bodies, &[vec![0, 1]], &embedder, 0.0).unwrap();

        assert!(scored.is_empty());
        assert_eq!(
            embedder.calls.load(Ordering::SeqCst),
            0,
            "no open pair means no reason to embed anything"
        );
        assert!(
            db.get("h0").unwrap().unwrap().embedding.is_none(),
            "and nothing was written"
        );
    }

    #[test]
    fn the_duplication_model_is_not_the_document_model() {
        // Repointing the shared service would invalidate vectors.bin,
        // vec_documents and vec_memory and force a full re-embed of everything.
        assert_eq!(crate::llm::embeddings::MODEL_NAME, "AllMiniLML6V2");
        assert_eq!(crate::llm::embeddings::EMBEDDING_DIM, 384);
        assert_ne!(DEFAULT_DUP_MODEL, crate::llm::embeddings::MODEL_NAME);
        assert_ne!(DUP_EMBEDDING_DIM, crate::llm::embeddings::EMBEDDING_DIM);
    }

    /// The duplication pass writes to `dup.sqlite` and nowhere else.
    ///
    /// Asserted at the source level because the damage is silent: a call to
    /// `get_cached_service()` here would embed code bodies with the document
    /// model, and a write to the shared `VectorStore` would put 256-dim vectors
    /// into a 384-dim file. Neither fails loudly — the first returns worse
    /// answers, the second corrupts search for documents and memory both.
    #[test]
    fn the_duplication_pass_never_touches_the_shared_vector_store() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/code/duplication");
        let forbidden = [
            "get_cached_service",
            "VectorStore",
            "vec_documents",
            "vec_memory",
            "vectors.bin",
        ];
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("duplication module exists") {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            // Test modules and comment lines are dropped: this module's own doc
            // names all five to explain why it avoids them, and the list above
            // names them to forbid them. Only shipping code is evidence.
            let code = source.split("#[cfg(test)]").next().unwrap();
            for line in code.lines().filter(|l| !l.trim_start().starts_with("//")) {
                for name in forbidden {
                    assert!(
                        !line.contains(name),
                        "{} reaches for {name}; the duplication pass owns dup.sqlite only",
                        path.display()
                    );
                }
            }
            checked += 1;
        }
        assert!(checked >= 5, "only checked {checked} files");
    }

    /// The index path must never embed a body — a 160M-parameter model on
    /// every `mdkb index` is exactly the cost this design exists to avoid.
    ///
    /// The check is a path reference (`duplication::`), not the bare word:
    /// calling into the module requires naming it that way, while a doc comment
    /// that merely mentions the duplication pass is not a call.
    #[test]
    fn the_indexing_pipeline_does_not_reach_the_duplication_pass() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut sources: Vec<std::path::PathBuf> = std::fs::read_dir(root.join("src/code/indexing"))
            .expect("indexing module exists")
            .filter_map(|e| {
                let p = e.ok()?.path();
                (p.extension()? == "rs").then_some(p)
            })
            .collect();
        sources.push(root.join("src/core/indexing.rs"));
        assert!(sources.len() > 1, "found no indexing sources to check");

        for path in sources {
            let source = std::fs::read_to_string(&path).unwrap();
            assert!(
                !source.contains("duplication::"),
                "{} calls into the duplication pass; it must stay on-demand",
                path.display()
            );
        }
    }

    #[test]
    fn an_unsupported_model_name_is_refused() {
        let err = fastembed_model("BgeSmallEnV15").unwrap_err();
        assert!(format!("{err}").contains("unsupported duplication model"), "{err}");
    }

    #[test]
    fn every_supported_name_resolves_to_a_model() {
        for name in SUPPORTED_DUP_MODELS {
            assert!(fastembed_model(name).is_ok(), "{name}");
        }
        assert!(SUPPORTED_DUP_MODELS.contains(&DEFAULT_DUP_MODEL));
    }

    #[test]
    #[ignore = "downloads ONNX weights"]
    fn the_model_yields_normalised_vectors_of_the_stored_width() {
        let embedder = DupEmbedder::new(DEFAULT_DUP_MODEL).unwrap();

        let vectors = embedder
            .embed_bodies(&["fn a(x: u32) -> u32 { x + 1 }", "fn b(y: u32) -> u32 { y + 1 }"])
            .unwrap();

        assert_eq!(vectors.len(), 2);
        for v in &vectors {
            assert_eq!(v.len(), DUP_EMBEDDING_DIM);
            assert!((dot(v, v) - 1.0).abs() < 1e-5, "not unit length");
        }
        // Against the shipped threshold, not an invented one: what has to hold
        // is that a renamed copy clears the floor `[code.duplication]` uses.
        let score = dot(&vectors[0], &vectors[1]);
        println!("renamed copies scored {score}");
        assert!(score >= 0.70, "renamed copies scored {score}");
    }

    #[test]
    #[ignore = "downloads ONNX weights"]
    fn the_duplication_embedder_is_cached_per_process() {
        release_dup_embedder();
        let a = get_dup_embedder(DEFAULT_DUP_MODEL).unwrap();
        let b = get_dup_embedder(DEFAULT_DUP_MODEL).unwrap();

        assert!(Arc::ptr_eq(&a, &b));
        release_dup_embedder();
    }
}
