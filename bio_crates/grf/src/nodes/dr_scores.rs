//! Shared helper: doubly-robust (AIPW) score computation.
//!
//! Used by:
//! - [`grf_average_treatment_effect`](crate::nodes::average_treatment_effect)
//! - [`grf_best_linear_projection`](crate::nodes::best_linear_projection)
//! - (future) [`grf_rank_average_treatment_effect`](crate::nodes::rank_average_treatment_effect)
//!
//! Mirrors `get_scores.causal_forest` in R:
//! ```text
//!   DR_score[i] = τ̂[i] + γ[i] · (Y[i] − Ŷ[i] − τ̂[i] · (W[i] − Ŵ[i]))
//!   γ[i]       = (W[i] − Ŵ[i]) / (Ŵ[i] · (1 − Ŵ[i]))  (binary treatment)
//!              = NaN                                    (continuous treatment)
//! ```

/// Compute AIPW DR scores for a binary treatment. Returns a vector of
/// length `n`. Entries where `Ŵ ∈ {0, 1}` are `NaN` so the caller can
/// filter / exclude them.
pub fn dr_scores_binary(
    y_orig: &[f64],
    w_orig: &[f64],
    y_hat: &[f64],
    w_hat: &[f64],
    tau_hat: &[f64],
) -> Vec<f64> {
    let n = y_orig.len();
    debug_assert_eq!(w_orig.len(), n);
    debug_assert_eq!(y_hat.len(), n);
    debug_assert_eq!(w_hat.len(), n);
    debug_assert_eq!(tau_hat.len(), n);

    (0..n).map(|i| {
        let gamma = if w_hat[i] <= 0.0 || w_hat[i] >= 1.0 {
            f64::NAN
        } else {
            (w_orig[i] - w_hat[i]) / (w_hat[i] * (1.0 - w_hat[i]))
        };
        let y_resid = y_orig[i] - (y_hat[i] + tau_hat[i] * (w_orig[i] - w_hat[i]));
        tau_hat[i] + gamma * y_resid
    }).collect()
}