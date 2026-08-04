//! Stouffer's Z-score method for combining independent p-values.
//!
//! Converts each p-value to a one-sided z-score `zᵢ = Φ⁻¹(1 − pᵢ)` and
//! combines them as
//!
//! ```text
//! Z = Σᵢ wᵢ zᵢ / √(Σᵢ wᵢ²)   →   p = 2·Φ(−|Z|)
//! ```
//!
//! Unweighted (all `wᵢ = 1`) when `weights` is `None`. The two-sided
//! p-value matches `poolr::stouffer`'s default.

use serde_json::json;

use super::{PvalSource, extract_pvals, validate_strict};
use crate::{Alternative, HypoError, HypothesisTest, KEY_COMPONENT_PVALS, KEY_KIND, KEY_N_TESTS, Result, dist::normal_inv, dist::normal_two_sided_p, extras};

/// Combine via Stouffer's Z (optionally weighted).
pub fn stouffer_combine<S: PvalSource>(srcs: &[S], weights: Option<&[f64]>) -> Result<HypothesisTest> {
    stouffer_combine_pvals(&extract_pvals(srcs)?, weights)
}

/// Same as [`stouffer_combine`] but takes raw p-values directly.
pub fn stouffer_combine_pvals(pvals: &[f64], weights: Option<&[f64]>) -> Result<HypothesisTest> {
    validate_strict(pvals)?;
    let k = pvals.len();
    if k == 0 {
        return Err(HypoError::InvalidInput("no p-values supplied".into()));
    }
    let w: Vec<f64> = match weights {
        Some(ws) => {
            if ws.len() != k {
                return Err(HypoError::LengthMismatch { a: k, b: ws.len() });
            }
            for &x in ws {
                if !x.is_finite() || x < 0.0 {
                    return Err(HypoError::InvalidWeights);
                }
            }
            ws.to_vec()
        }
        None => vec![1.0; k],
    };

    let mut numer = 0.0;
    let mut denom = 0.0;
    for (p, w_i) in pvals.iter().zip(w.iter()) {
        // poolr uses qnorm(1 - p_lower) — same as -qnorm(p).
        let z = normal_inv(1.0 - p);
        numer += w_i * z;
        denom += w_i * w_i;
    }
    if denom <= 0.0 {
        return Err(HypoError::InvalidWeights);
    }
    let z_combined = numer / denom.sqrt();
    let p_value = normal_two_sided_p(z_combined);

    let mut e = extras([
        (KEY_KIND, json!("stouffer_combined_test")),
        (KEY_N_TESTS, json!(k as u64)),
        (KEY_COMPONENT_PVALS, json!(pvals)),
        ("z", json!(z_combined)),
    ]);
    if let Some(ws) = weights {
        e.insert("weights".into(), json!(ws));
    }
    Ok(HypothesisTest::new(
        z_combined,
        p_value,
        f64::INFINITY,
        Alternative::TwoSided,
        "Stouffer's Combined Z-Score Test",
        e,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unweighted_stouffer() {
        // z_i = Φ⁻¹(1 - p_i); for equal p = 0.05, z = 1.6449
        let pvals = vec![0.05_f64; 3];
        let t = stouffer_combine_pvals(&pvals, None).unwrap();
        let z = normal_inv(0.95) * 3.0 / (3.0_f64).sqrt();
        assert!((t.extra_f64("z").unwrap() - z).abs() < 1e-9);
        assert!((t.p_value - normal_two_sided_p(z)).abs() < 1e-12);
    }

    #[test]
    fn weighted_stouffer() {
        let pvals = vec![0.1_f64, 0.02];
        let w = vec![1.0, 2.0];
        let t = stouffer_combine_pvals(&pvals, Some(&w)).unwrap();
        let z1 = normal_inv(0.9);
        let z2 = normal_inv(0.98);
        let z_expect = (1.0 * z1 + 2.0 * z2) / (5.0_f64).sqrt();
        assert!((t.extra_f64("z").unwrap() - z_expect).abs() < 1e-9);
    }

    #[test]
    fn rejects_mismatched_weights() {
        assert!(stouffer_combine_pvals(&[0.1, 0.2], Some(&[1.0])).is_err());
        assert!(stouffer_combine_pvals(&[0.1, 0.2], Some(&[1.0, -1.0])).is_err());
    }
}
