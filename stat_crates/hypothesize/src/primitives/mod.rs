//! Primitives — likelihood trinity (Wald / LRT / Score) plus z-test.
//!
//! Every constructor returns a [`HypothesisTest`], the uniform data
//! abstraction. Phase 2–3 adds sample-data tests (t / Wilcoxon / …).

pub mod anova;
pub mod cor;
pub mod gof;
pub mod lrt;
pub mod prop;
pub mod ranks;
pub mod score;
pub mod t;
pub mod table;
pub mod var;
pub mod wald;
pub mod z;

pub use anova::oneway_anova;
pub use cor::{CorMethod, cor_test};
pub use gof::{AdDist, anderson_darling, chisq_gof, ks_one_sample, ks_two_sample, shapiro_wilk};
pub use lrt::{LogLik, lrt};
pub use prop::{prop_test_one, prop_test_two};
pub use ranks::{ZeroMethod, friedman_test, kruskal_wallis, mann_whitney, wilcoxon_signed_rank};
pub use score::{score_multi, score_uni};
pub use t::{t_test_one, t_test_paired, t_test_two};
pub use table::fisher_exact;
pub use var::{Center, bartlett_test, fligner_test, levene_test, var_test};
pub use wald::{wald_multi, wald_uni};
pub use z::z_test;

use crate::Alternative;

/// The fundamental data abstraction (SICP): the result of a hypothesis test.
///
/// Direct port of `hypothesize::hypothesis_test`'s S3 object. `extras` holds
/// everything R stored as named list elements — `estimate`, `se`, `vcov`,
/// `null_value`, `null_loglik`, `alt_loglik`, `component_pvals`, `score`,
/// `fisher_info`, `z`, `sigma`, `n`, `n_tests`, `adjustment_method`,
/// `original_pval`, … — keyed by their R-side names.
#[derive(Clone, Debug)]
pub struct HypothesisTest {
    /// Test statistic.
    pub stat: f64,
    /// P-value under H₀.
    pub p_value: f64,
    /// Degrees of freedom (`f64::INFINITY` for Normal-based tests).
    pub dof: f64,
    /// Alternative hypothesis stored on the test. For combinators and tests
    /// without a natural direction this is [`Alternative::TwoSided`].
    pub alternative: Alternative,
    /// Short human-readable method name (e.g. `"One Sample z-test"`).
    pub method: &'static str,
    /// Free-form metadata mirroring the R object's named elements.
    pub extras: serde_json::Map<String, serde_json::Value>,
}

impl HypothesisTest {
    /// Build a fresh test, allocating `extras` from key/value pairs.
    pub(crate) fn new(
        stat: f64,
        p_value: f64,
        dof: f64,
        alternative: Alternative,
        method: &'static str,
        extras: serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        Self {
            stat,
            p_value,
            dof,
            alternative,
            method,
            extras,
        }
    }

    /// Reject at level `alpha`?
    pub fn is_significant_at(&self, alpha: f64) -> bool {
        self.p_value < alpha
    }

    /// Borrowed view of an extra as `f64`.
    pub fn extra_f64(&self, key: &str) -> Option<f64> {
        self.extras.get(key).and_then(|v| v.as_f64())
    }

    /// Borrowed view of an extra as a `&[f64]` (flattened from a JSON array).
    pub fn extra_f64_vec(&self, key: &str) -> Option<Vec<f64>> {
        self.extras
            .get(key)?
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect())
    }
}
