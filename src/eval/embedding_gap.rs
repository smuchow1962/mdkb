//! Deterministic separation-quality evaluation for code embedding models.
//!
//! Answers one question: can a model tell duplicated code from code that merely
//! looks alike? The metric is the *gap* — mean cosine over the pairs that ought
//! to score high, minus mean cosine over the pairs that ought to score low. A
//! model with a wide gap leaves room for a threshold; a model with a narrow one
//! has no threshold that separates the two, whatever its absolute scores are.
//!
//! This is not recall@k. Duplication detection never ranks a result list against
//! a query — it asks of one pair at a time whether the two are the same logic,
//! so what matters is the distance between the two populations, not the order
//! within either.
//!
//! `embed` is injected, so the metric is testable without a model or a network.

use serde::Serialize;

/// One evaluation pair and the answer a model is expected to give.
///
/// `high` is the ground truth: true when the two texts are the same logic and a
/// model *should* score them close, false when they are not and it should not.
#[derive(Debug, Clone)]
pub struct GapCase {
    pub a: String,
    pub b: String,
    pub high: bool,
}

impl GapCase {
    /// A pair that ought to score high — the two texts are the same logic.
    pub fn high(a: impl Into<String>, b: impl Into<String>) -> Self {
        Self {
            a: a.into(),
            b: b.into(),
            high: true,
        }
    }

    /// A pair that ought to score low — the two texts are not the same logic.
    pub fn low(a: impl Into<String>, b: impl Into<String>) -> Self {
        Self {
            a: a.into(),
            b: b.into(),
            high: false,
        }
    }
}

/// Aggregate separation over a case set.
///
/// `gap` is the headline number; the two means are kept because they say *how*
/// a model failed. A model that scores everything at 0.9 and one that scores
/// everything at 0.1 both report a gap near zero, and they are not the same
/// problem.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GapReport {
    pub gap: f64,
    pub high_mean: f64,
    pub low_mean: f64,
    pub n: usize,
}

/// Compute the high/low cosine gap over `cases`.
///
/// An empty side contributes a mean of 0.0 rather than NaN: a case set with no
/// low pairs is a badly built fixture, and a report that quietly poisons every
/// later arithmetic operation hides that instead of showing it.
pub fn run_gap(cases: &[GapCase], embed: &dyn Fn(&str) -> Vec<f32>) -> GapReport {
    let mut high_sum = 0.0f64;
    let mut high_n = 0usize;
    let mut low_sum = 0.0f64;
    let mut low_n = 0usize;

    for case in cases {
        let score = f64::from(cosine(&embed(&case.a), &embed(&case.b)));
        if case.high {
            high_sum += score;
            high_n += 1;
        } else {
            low_sum += score;
            low_n += 1;
        }
    }

    let high_mean = mean(high_sum, high_n);
    let low_mean = mean(low_sum, low_n);
    GapReport {
        gap: high_mean - low_mean,
        high_mean,
        low_mean,
        n: cases.len(),
    }
}

/// Sum over count, with an empty population reported as 0.0 rather than NaN.
fn mean(sum: f64, n: usize) -> f64 {
    if n == 0 { 0.0 } else { sum / n as f64 }
}

/// The case set the duplication model is chosen on.
///
/// Each case isolates one axis of similarity and holds the others still, which
/// is what makes a per-case failure readable. The four `low` cases are the
/// adversarial half: code that looks alike and is not the same logic. A model
/// that scores those high is worse than useless here, because the tool's job is
/// to tell them apart.
pub fn duplication_cases() -> Vec<GapCase> {
    vec![
        // Same logic, every identifier renamed. The axis the benchmark reports
        // most models are weakest on.
        GapCase::high(
            "fn total(items: &[Item]) -> u64 { let mut sum = 0; for it in items { sum += it.price; } sum }",
            "fn aggregate(rows: &[Row]) -> u64 { let mut acc = 0; for r in rows { acc += r.cost; } acc }",
        ),
        // Same problem, different algorithm, almost no shared vocabulary.
        GapCase::high(
            "fn contains(xs: &[i32], k: i32) -> bool { xs.iter().any(|x| *x == k) }",
            "fn contains(xs: &[i32], k: i32) -> bool { let (mut lo, mut hi) = (0, xs.len()); while lo < hi { let mid = (lo + hi) / 2; if xs[mid] == k { return true } else if xs[mid] < k { lo = mid + 1 } else { hi = mid } } false }",
        ),
        // Same logic, one written as a loop and one as an iterator chain.
        GapCase::high(
            "fn evens(v: &[i32]) -> Vec<i32> { let mut out = Vec::new(); for x in v { if x % 2 == 0 { out.push(*x); } } out }",
            "fn evens(v: &[i32]) -> Vec<i32> { v.iter().copied().filter(|x| x % 2 == 0).collect() }",
        ),
        // Code paired with the English description of what it does.
        GapCase::high(
            "fn retry<T>(f: impl Fn() -> Option<T>, n: usize) -> Option<T> { for _ in 0..n { if let Some(v) = f() { return Some(v) } } None }",
            "Call the closure up to n times and return the first successful result, or nothing if every attempt failed.",
        ),
        // Same logic across a rename AND a reordering of independent statements.
        GapCase::high(
            "fn init(cfg: &Cfg) -> App { let db = open(cfg.db); let log = logger(cfg.level); App { db, log } }",
            "fn build(conf: &Conf) -> Service { let writer = logger(conf.verbosity); let store = open(conf.store); Service { store, log: writer } }",
        ),
        // ---- adversarial: looks alike, is not the same logic ----
        // Identical framework boilerplate, unrelated bodies. The benchmark's
        // headline false-positive source.
        GapCase::low(
            "#[tokio::main] async fn main() -> anyhow::Result<()> { let app = Router::new(); serve(app).await?; Ok(()) }",
            "#[tokio::main] async fn main() -> anyhow::Result<()> { let cfg = load()?; migrate(&cfg).await?; Ok(()) }",
        ),
        // Same identifiers, opposite behaviour.
        GapCase::low(
            "fn apply(state: &mut State, delta: i64) { state.value += delta; }",
            "fn apply(state: &mut State, delta: i64) { state.value = delta; }",
        ),
        // Same signature and shape, different domain entirely.
        GapCase::low(
            "fn parse(input: &str) -> Result<Date> { Date::from_iso(input) }",
            "fn parse(input: &str) -> Result<Color> { Color::from_hex(input) }",
        ),
        // Same control-flow skeleton, unrelated purpose.
        GapCase::low(
            "fn sum_file(p: &Path) -> u64 { let mut t = 0; for line in read(p) { t += line.len() as u64; } t }",
            "fn hash_dir(p: &Path) -> u64 { let mut h = 0; for e in walk(p) { h ^= fnv(e.name()); } h }",
        ),
    ]
}

/// Cosine similarity, with a zero-magnitude vector scored as 0.0.
///
/// Vectors of differing length score 0.0 rather than panicking: an `embed` that
/// returns ragged output is a broken model, and this harness exists to report
/// that a model is bad, not to abort on it.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < f32::EPSILON { 0.0 } else { dot / denom }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for a model: each distinct word becomes one dimension, so two
    /// texts sharing vocabulary score high and two sharing none score zero.
    /// Deterministic, offline, and enough to prove the arithmetic.
    fn bag_of_words(text: &str) -> Vec<f32> {
        const DIM: usize = 64;
        let mut v = vec![0.0f32; DIM];
        for word in text.split_whitespace() {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for byte in word.as_bytes() {
                h ^= u64::from(*byte);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            v[(h % DIM as u64) as usize] += 1.0;
        }
        v
    }

    #[test]
    fn empty_cases_yield_a_zero_gap_not_nan() {
        let report = run_gap(&[], &bag_of_words);

        assert_eq!(report.n, 0);
        assert!(report.gap.is_finite(), "gap must never be NaN");
        assert!(report.gap.abs() < f64::EPSILON);
        assert!(report.high_mean.abs() < f64::EPSILON);
        assert!(report.low_mean.abs() < f64::EPSILON);
    }

    #[test]
    fn a_one_sided_case_set_still_reports_a_finite_gap() {
        // No low pairs at all: the low mean has no population to average.
        let report = run_gap(&[GapCase::high("alpha beta", "alpha beta")], &bag_of_words);

        assert!(report.gap.is_finite(), "an empty side must not poison the gap");
        assert!(report.low_mean.abs() < f64::EPSILON);
    }

    #[test]
    fn the_gap_separates_pairs_that_share_meaning_from_pairs_that_do_not() {
        let cases = vec![
            GapCase::high("parse retry backoff", "parse retry backoff"),
            GapCase::high("open socket listen", "open socket listen"),
            GapCase::low("parse retry backoff", "render template html"),
            GapCase::low("open socket listen", "compress archive zip"),
        ];

        let report = run_gap(&cases, &bag_of_words);

        assert_eq!(report.n, 4);
        // Identical texts are cosine 1.0; texts sharing no vocabulary are 0.0.
        assert!(
            (report.high_mean - 1.0).abs() < 1e-6,
            "identical pairs must score 1.0, got {}",
            report.high_mean
        );
        assert!(
            report.low_mean.abs() < 1e-6,
            "disjoint pairs must score 0.0, got {}",
            report.low_mean
        );
        assert!(
            (report.gap - 1.0).abs() < 1e-6,
            "gap must be the distance between the two populations, got {}",
            report.gap
        );
    }

    #[test]
    fn a_model_that_scores_everything_alike_reports_no_gap() {
        // The failure the headline number exists to catch: high absolute scores
        // that carry no information, because nothing separates the populations.
        let flat = |_: &str| vec![1.0f32; 8];
        let cases = vec![
            GapCase::high("a", "b"),
            GapCase::low("c", "d"),
        ];

        let report = run_gap(&cases, &flat);

        assert!(report.gap.abs() < 1e-6, "a flat model must show a zero gap");
        assert!((report.high_mean - 1.0).abs() < 1e-6);
        assert!((report.low_mean - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_ragged_embedding_scores_zero_instead_of_panicking() {
        let ragged = |text: &str| vec![1.0f32; text.len()];

        let report = run_gap(&[GapCase::high("ab", "abcd")], &ragged);

        assert!(report.high_mean.abs() < f64::EPSILON);
    }

    #[test]
    fn the_case_set_has_both_populations() {
        let cases = duplication_cases();
        assert!(cases.iter().any(|c| c.high), "no high cases");
        assert!(cases.iter().any(|c| !c.high), "no low cases");
    }

    /// Measure the real models. `#[ignore]`d on purpose: it downloads ONNX
    /// weights (~500 MB for Jina) and takes minutes, which CI must never do.
    ///
    /// Run by hand:
    ///   cargo test --lib eval::embedding_gap -- --ignored --nocapture
    #[test]
    #[ignore = "downloads ONNX weights; run by hand to choose the duplication model"]
    fn gap_models() {
        use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

        let cache = std::env::var("FASTEMBED_CACHE_DIR").unwrap_or_else(|_| {
            format!("{}/.cache/fastembed", std::env::var("HOME").unwrap())
        });
        let cases = duplication_cases();

        // (label, model, truncate-to-dims). A truncated variant answers whether
        // this model tolerates the benchmark's 256-dim reduction, which is only
        // established for MRL-trained models and NOT for v2-base-code.
        let variants: Vec<(&str, EmbeddingModel, Option<usize>)> = vec![
            ("AllMiniLML6V2-384", EmbeddingModel::AllMiniLML6V2, None),
            (
                "JinaEmbeddingsV2BaseCode-768",
                EmbeddingModel::JinaEmbeddingsV2BaseCode,
                None,
            ),
            (
                "JinaEmbeddingsV2BaseCode-256",
                EmbeddingModel::JinaEmbeddingsV2BaseCode,
                Some(256),
            ),
        ];

        println!("\n{:<32} {:>8} {:>10} {:>10}", "model", "gap", "high", "low");
        for (label, model, truncate) in variants {
            let embedder = TextEmbedding::try_new(
                InitOptions::new(model)
                    .with_cache_dir(cache.clone().into())
                    .with_show_download_progress(true),
            )
            .expect("model init");

            let embed = |text: &str| {
                let mut v = embedder
                    .embed(vec![text], Some(1))
                    .expect("embed")
                    .pop()
                    .expect("one vector");
                if let Some(d) = truncate {
                    v.truncate(d);
                }
                v
            };

            let r = run_gap(&cases, &embed);
            println!(
                "{label:<32} {:>8.4} {:>10.4} {:>10.4}",
                r.gap, r.high_mean, r.low_mean
            );
        }
        println!();
    }
}
