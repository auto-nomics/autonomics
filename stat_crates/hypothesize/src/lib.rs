//! `hypothesize` — composable hypothesis-testing algebra.
//!
//! Pure-Rust port of the algebraic core of R's
//! [`hypothesize`](https://github.com/queelius/hypothesize) package, built on
//! three SICP principles:
//!
//! 1. **Data abstraction** — every test is a [`HypothesisTest`] value with a
//!    uniform accessor surface (`p_value`, `stat`, `dof`, `is_significant_at`).
//! 2. **Closure** — combining tests yields tests
//!    ([`fisher_combine`](combine::fisher_combine),
//!    [`intersection_test`](combine::intersection_test), …).
//! 3. **Higher-order functions** — [`adjust_pvals`](adjust::adjust_pvals)
//!    transforms tests; [`invert_test`](invert::invert_test) inverts a test
//!    into a confidence set.
//!
//! Distribution maths wrap [`statrs`](https://docs.rs/statrs); matrix algebra
//! uses [`faer`](https://docs.rs/faer). Phase 1 covers the likelihood trinity
//! (Wald / LRT / Score) plus z-test, the Boolean and p-value combinators,
//! the full `p.adjust` family, and test↔confidence-set duality. Sample-data
//! tests (t / Wilcoxon / chi-squared / KS / …) land in Phase 2–3.
//!
//! Cross-validated against R `hypothesize::` and `stats::p.adjust` — see
//! `tests/xval_hypothesize.rs` and `tests/xval_padjust.rs`.

pub mod adjust;
pub mod combine;
pub mod dist;
pub mod invert;
pub mod primitives;

pub use adjust::{AdjustMethod, adjust_pvals, p_adjust_raw};
pub use combine::{
    boolean::{complement_test, intersection_test, union_test},
    fisher::{fisher_combine, fisher_combine_pvals},
    stouffer::{stouffer_combine, stouffer_combine_pvals},
    tippett::{TippettVariant, tippett_combine, tippett_combine_pvals},
    wilkinson::{wilkinson_combine, wilkinson_combine_pvals},
};
pub use dist::{chisq_sf, f_sf, normal_cdf, normal_inv, normal_sf, t_sf};
pub use invert::{ConfidenceSet, confint_wald, confint_z, invert_test};
pub use primitives::{
    HypothesisTest,
    anova::oneway_anova,
    cor::{CorMethod, cor_test},
    gof::{AdDist, anderson_darling, chisq_gof, ks_one_sample, ks_two_sample, shapiro_wilk},
    lrt::{LogLik, lrt},
    prop::{prop_test_one, prop_test_two},
    ranks::{ZeroMethod, friedman_test, kruskal_wallis, mann_whitney, wilcoxon_signed_rank},
    score::{score_multi, score_uni},
    t::{t_test_one, t_test_paired, t_test_two},
    table::fisher_exact,
    var::{Center, bartlett_test, fligner_test, levene_test, var_test},
    wald::{wald_multi, wald_uni},
    z::z_test,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Alternative hypothesis, matching R's `alternative` argument.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Alternative {
    /// `H₁: θ ≠ θ₀` — two-sided (default).
    #[default]
    TwoSided,
    /// `H₁: θ < θ₀`.
    Less,
    /// `H₁: θ > θ₀`.
    Greater,
}

impl Alternative {
    /// R-facing string (`"two.sided"`, `"less"`, `"greater"`).
    pub fn as_r_str(self) -> &'static str {
        match self {
            Alternative::TwoSided => "two.sided",
            Alternative::Less => "less",
            Alternative::Greater => "greater",
        }
    }
    pub fn parse_r(s: &str) -> std::result::Result<Self, HypoError> {
        let lower = s.to_ascii_lowercase().replace('.', "_");
        match lower.as_str() {
            "two_sided" | "twosided" | "two" => Ok(Alternative::TwoSided),
            "less" | "l" => Ok(Alternative::Less),
            "greater" | "g" => Ok(Alternative::Greater),
            other => Err(HypoError::InvalidInput(format!(
                "unknown alternative '{other}' (expected two.sided|less|greater)"
            ))),
        }
    }
}


/// Errors returned by `hypothesize`.
#[derive(Debug, Clone, thiserror::Error)]
pub enum HypoError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("length mismatch: {a} vs {b}")]
    LengthMismatch { a: usize, b: usize },
    #[error("p-value out of range (0, 1]: {0}")]
    PvalOutOfRange(f64),
    #[error("negative or NaN weights")]
    InvalidWeights,
    #[error("singular matrix: {0}")]
    SingularMatrix(String),
    #[error("numerical error: {0}")]
    Numerical(String),
}

pub type Result<T> = std::result::Result<T, HypoError>;

/// Tag stored in [`HypothesisTest::extras`] under [`KEY_KIND`] mirroring the
/// R S3 class vector (e.g. `"wald_test"`, `"likelihood_ratio_test"`).
pub const KEY_KIND: &str = "kind";

/// Tag for the original (pre-adjustment / pre-complement) p-value.
pub const KEY_ORIGINAL_PVAL: &str = "original_pval";

/// Tag for the family of component p-values of a combined test.
pub const KEY_COMPONENT_PVALS: &str = "component_pvals";

/// Tag for the number of component tests in a combined test.
pub const KEY_N_TESTS: &str = "n_tests";

/// Helper: build a `serde_json::Map` from key/value pairs of `Into<Value>`.
pub(crate) fn extras(
    pairs: impl IntoIterator<Item = (&'static str, serde_json::Value)>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    m
}

/// Sample mean. Returns 0 for empty slices (caller should validate length).
pub(crate) fn extras_mean(x: &[f64]) -> f64 {
    x.iter().sum::<f64>() / x.len() as f64
}

/// Sample variance (n−1 denominator). Caller must ensure `len ≥ 2`.
pub(crate) fn extras_var(x: &[f64]) -> f64 {
    let n = x.len() as f64;
    let m = extras_mean(x);
    x.iter().map(|&xi| (xi - m).powi(2)).sum::<f64>() / (n - 1.0)
}

/// Average ranks (R `rank(ties.method = "average")`).
pub(crate) fn extras_rank_average(v: &[f64]) -> Vec<f64> {
    let n = v.len();
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| v[a].partial_cmp(&v[b]).unwrap_or(std::cmp::Ordering::Equal));
    let mut ranks = vec![0.0_f64; n];
    let mut i = 0;
    while i < n {
        let mut j = i + 1;
        while j < n && v[idx[j]] == v[idx[i]] {
            j += 1;
        }
        let avg = ((i + 1 + j) as f64) / 2.0;
        for k in i..j {
            ranks[idx[k]] = avg;
        }
        i = j;
    }
    ranks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternative_roundtrip() {
        for a in [
            Alternative::TwoSided,
            Alternative::Less,
            Alternative::Greater,
        ] {
            let s = a.as_r_str();
            assert_eq!(Alternative::parse_r(s).unwrap(), a);
        }
        assert_eq!(
            Alternative::parse_r("TWO_SIDED").unwrap(),
            Alternative::TwoSided
        );
        assert!(Alternative::parse_r("sideways").is_err());
    }
}
