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
        assert_eq!(report.gap, 0.0);
        assert_eq!(report.high_mean, 0.0);
        assert_eq!(report.low_mean, 0.0);
    }

    #[test]
    fn a_one_sided_case_set_still_reports_a_finite_gap() {
        // No low pairs at all: the low mean has no population to average.
        let report = run_gap(&[GapCase::high("alpha beta", "alpha beta")], &bag_of_words);

        assert!(report.gap.is_finite(), "an empty side must not poison the gap");
        assert_eq!(report.low_mean, 0.0);
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

        assert_eq!(report.high_mean, 0.0);
    }
}
