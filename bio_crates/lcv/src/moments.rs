//! Moment-estimation primitives ported from `LCV/R/MomentFunctions.R`:
//! [`wls_slope`] (`WeightedRegression`, single-column), [`wls_intercept`]
//! (`WeightedRegression` with intercept), [`weighted_mean`] (`WeightedMean`),
//! and [`estimate_k4`] (`EstimateK4`).
//!
//! [`estimate_k4`] is the heart of LCV: it runs univariate LD-score
//! regression on each trait, cross-trait LDSC, normalises the Z-scores, and
//! computes the **mixed 4th moments** k₄₁ = E(nz₁³·nz₂) and k₄₂ = E(nz₁·nz₂³)
//! whose asymmetry encodes genetic causality.

use crate::error::{LcvError, Result};

// ─────────────────────────── weighted helpers ───────────────────────────

/// Weighted regression of `y` on a single regressor `x` (no intercept):
/// β = Σ(w·x·y) / Σ(w·x²).  Mirrors R `WeightedRegression(x, y, w)` when `x`
/// is a bare vector (1-column design matrix).
#[inline]
pub fn wls_slope(x: &[f64], y: &[f64], w: &[f64]) -> f64 {
    let mut swxy = 0.0;
    let mut swxx = 0.0;
    for i in 0..x.len() {
        swxy += w[i] * x[i] * y[i];
        swxx += w[i] * x[i] * x[i];
    }
    swxy / swxx
}

/// Weighted regression of `y` on `[x, 1]` (with intercept):
/// returns `(slope, intercept)` = (XᵀWX)⁻¹ XᵀWy with W = diag(w).
/// Mirrors R `WeightedRegression(cbind(x, 1), y, w)`.
#[inline]
pub fn wls_intercept(x: &[f64], y: &[f64], w: &[f64]) -> (f64, f64) {
    let mut swxx = 0.0;
    let mut swx = 0.0;
    let mut sw = 0.0;
    let mut swxy = 0.0;
    let mut swy = 0.0;
    for i in 0..x.len() {
        let xi = x[i];
        let yi = y[i];
        let wi = w[i];
        swxx += wi * xi * xi;
        swx += wi * xi;
        sw += wi;
        swxy += wi * xi * yi;
        swy += wi * yi;
    }
    let det = swxx * sw - swx * swx;
    let slope = (sw * swxy - swx * swy) / det;
    let intercept = (swxx * swy - swx * swxy) / det;
    (slope, intercept)
}

/// Weighted mean: Σ(y·w) / Σw. Mirrors R `WeightedMean(y, w)`.
#[inline]
pub fn weighted_mean(y: &[f64], w: &[f64]) -> f64 {
    let mut swy = 0.0;
    let mut sw = 0.0;
    for i in 0..y.len() {
        swy += y[i] * w[i];
        sw += w[i];
    }
    swy / sw
}

/// Plain arithmetic mean (R `mean(x)`).
#[inline]
fn mean(x: &[f64]) -> f64 {
    x.iter().sum::<f64>() / x.len() as f64
}

// ─────────────────────────── EstimateK4 ───────────────────────────

/// Per-SNP moment estimates returned by [`estimate_k4`] — the 8 outputs of
/// R's `EstimateK4(..., nargout = 8)`.
#[derive(Debug, Clone, Copy, Default)]
pub struct MomentEstimates {
    /// Estimated genetic correlation (cross-trait LDSC slope / √(h²₁·h²₂)).
    pub rho: f64,
    /// E(nz₁³·nz₂) estimate (mixed 4th moment, *before* subtracting 3·ρ).
    pub k41: f64,
    /// E(nz₁·nz₂³) estimate.
    pub k42: f64,
    /// Cross-trait LDSC intercept.
    pub intercept12: f64,
    /// Normalisation √(weighted_mean(z₁²) − intercept₁), ∝ √h²₁.
    pub s1: f64,
    /// Normalisation √(weighted_mean(z₂²) − intercept₂), ∝ √h²₂.
    pub s2: f64,
    /// LDSC intercept for trait 1.
    pub intercept1: f64,
    /// LDSC intercept for trait 2.
    pub intercept2: f64,
}

/// Configuration for [`estimate_k4`] / [`crate::model::run_lcv`], matching
/// the intercept and filtering switches of R's `EstimateK4` / `RunLCV`.
#[derive(Debug, Clone)]
pub struct K4Config {
    /// If `true`, estimate the cross-trait LDSC intercept from significant-SNP
    /// exclusion (R `crosstrait.intercept = 1`); if `false`, fix it at
    /// `intercept12` (R `crosstrait.intercept = 0`).
    pub crosstrait_intercept: bool,
    /// If `true`, estimate univariate LDSC intercepts (R `ldsc.intercept = 1`);
    /// if `false`, fix them at `1/n₁`, `1/n₂` (R `ldsc.intercept = 0`).
    pub ldsc_intercept: bool,
    /// Chisq significance threshold for excluding GWS SNPs when computing
    /// intercepts (R `sig.threshold`; default `∞`).
    pub sig_threshold: f64,
    /// Trait 1 sample size (only used when `ldsc_intercept = false`).
    pub n1: f64,
    /// Trait 2 sample size.
    pub n2: f64,
    /// Initial cross-trait intercept (used when `crosstrait_intercept = false`).
    pub intercept12: f64,
}

impl Default for K4Config {
    fn default() -> Self {
        Self {
            crosstrait_intercept: true,
            ldsc_intercept: true,
            sig_threshold: f64::INFINITY,
            n1: 1.0,
            n2: 1.0,
            intercept12: 0.0,
        }
    }
}

/// Estimate mixed 4th moments and LDSC parameters — R `EstimateK4`.
///
/// Runs univariate LDSC on each trait, cross-trait LDSC, normalises Z-scores,
/// and returns the full [`MomentEstimates`] struct.
///
/// All input slices must have the same length.
pub fn estimate_k4(
    ell: &[f64],
    z1: &[f64],
    z2: &[f64],
    weights: &[f64],
    cfg: &K4Config,
) -> Result<MomentEstimates> {
    let m = ell.len();
    if z1.len() != m || z2.len() != m || weights.len() != m {
        return Err(LcvError::Input(
            "ell, z1, z2, weights must have the same length".into(),
        ));
    }
    if m == 0 {
        return Err(LcvError::Input("empty input".into()));
    }

    // Precompute z² arrays (reused multiple times).
    let z1sq: Vec<f64> = z1.iter().map(|&z| z * z).collect();
    let z2sq: Vec<f64> = z2.iter().map(|&z| z * z).collect();
    let z12: Vec<f64> = (0..m).map(|i| z1[i] * z2[i]).collect();

    // ── Univariate LDSC regression on each trait ──────────────────
    let (intercept1, h2g1, intercept2, h2g2);

    if !cfg.ldsc_intercept {
        intercept1 = 1.0 / cfg.n1;
        intercept2 = 1.0 / cfg.n2;
        let y1: Vec<f64> = z1sq.iter().map(|&v| v - intercept1).collect();
        let y2: Vec<f64> = z2sq.iter().map(|&v| v - intercept2).collect();
        h2g1 = wls_slope(ell, &y1, weights);
        h2g2 = wls_slope(ell, &y2, weights);
    } else {
        let mean_z1sq = mean(&z1sq);
        let mean_z2sq = mean(&z2sq);
        let thr1 = cfg.sig_threshold * mean_z1sq;
        let thr2 = cfg.sig_threshold * mean_z2sq;

        // Trait 1: estimate intercept from non-significant SNPs
        let (xs1, ys1, ws1): (Vec<f64>, Vec<f64>, Vec<f64>) = {
            let mut xs = Vec::new();
            let mut ys = Vec::new();
            let mut ws = Vec::new();
            for i in 0..m {
                if z1sq[i] <= thr1 {
                    xs.push(ell[i]);
                    ys.push(z1sq[i]);
                    ws.push(weights[i]);
                }
            }
            (xs, ys, ws)
        };
        if xs1.is_empty() {
            return Err(LcvError::Numeric(
                "all SNPs exceed significance threshold for trait 1".into(),
            ));
        }
        let (_, int1) = wls_intercept(&xs1, &ys1, &ws1);
        intercept1 = int1;
        let y1: Vec<f64> = z1sq.iter().map(|&v| v - intercept1).collect();
        h2g1 = wls_slope(ell, &y1, weights);

        // Trait 2
        let (xs2, ys2, ws2): (Vec<f64>, Vec<f64>, Vec<f64>) = {
            let mut xs = Vec::new();
            let mut ys = Vec::new();
            let mut ws = Vec::new();
            for i in 0..m {
                if z2sq[i] <= thr2 {
                    xs.push(ell[i]);
                    ys.push(z2sq[i]);
                    ws.push(weights[i]);
                }
            }
            (xs, ys, ws)
        };
        if xs2.is_empty() {
            return Err(LcvError::Numeric(
                "all SNPs exceed significance threshold for trait 2".into(),
            ));
        }
        let (_, int2) = wls_intercept(&xs2, &ys2, &ws2);
        intercept2 = int2;
        let y2: Vec<f64> = z2sq.iter().map(|&v| v - intercept2).collect();
        h2g2 = wls_slope(ell, &y2, weights);
    }

    // ── Cross-trait LDSC regression ───────────────────────────────
    let (rho, intercept12);
    if !cfg.crosstrait_intercept {
        let y12: Vec<f64> = z12.iter().map(|&v| v - cfg.intercept12).collect();
        let slope = wls_slope(ell, &y12, weights);
        rho = slope / (h2g1 * h2g2).sqrt();
        intercept12 = cfg.intercept12;
    } else {
        let mean_z1sq = mean(&z1sq);
        let mean_z2sq = mean(&z2sq);
        let thr1 = cfg.sig_threshold * mean_z1sq;
        let thr2 = cfg.sig_threshold * mean_z2sq;
        let (xs, ys, ws): (Vec<f64>, Vec<f64>, Vec<f64>) = {
            let mut xs = Vec::new();
            let mut ys = Vec::new();
            let mut ws = Vec::new();
            for i in 0..m {
                if z1sq[i] < thr1 && z2sq[i] < thr2 {
                    xs.push(ell[i]);
                    ys.push(z12[i]);
                    ws.push(weights[i]);
                }
            }
            (xs, ys, ws)
        };
        if xs.is_empty() {
            return Err(LcvError::Numeric(
                "all SNPs exceed significance threshold for cross-trait".into(),
            ));
        }
        let (_, int12) = wls_intercept(&xs, &ys, &ws);
        intercept12 = int12;
        let y12: Vec<f64> = z12.iter().map(|&v| v - intercept12).collect();
        let slope = wls_slope(ell, &y12, weights);
        rho = slope / (h2g1 * h2g2).sqrt();
    }

    // ── Normalise effect sizes ────────────────────────────────────
    let s1 = (weighted_mean(&z1sq, weights) - intercept1).sqrt();
    let s2 = (weighted_mean(&z2sq, weights) - intercept2).sqrt();

    if !s1.is_finite() || !s2.is_finite() {
        return Err(LcvError::Numeric(
            "negative heritability estimate (s² < 0); check LD-score / sumstat ordering".into(),
        ));
    }

    let nz1: Vec<f64> = z1.iter().map(|&z| z / s1).collect();
    let nz2: Vec<f64> = z2.iter().map(|&z| z / s2).collect();

    // ── Mixed 4th moments ─────────────────────────────────────────
    // k41 = weighted_mean(nz1³·nz2 − 3·nz1·nz2·(int1/s1²) − 3·(nz1²−int1/s1²)·int12/(s1·s2))
    let i1_s1sq = intercept1 / (s1 * s1);
    let i2_s2sq = intercept2 / (s2 * s2);
    let int12_s1s2 = intercept12 / (s1 * s2);

    let mut k41_vec = vec![0.0f64; m];
    let mut k42_vec = vec![0.0f64; m];
    for i in 0..m {
        k41_vec[i] = nz1[i].powi(3) * nz2[i]
            - 3.0 * nz1[i] * nz2[i] * i1_s1sq
            - 3.0 * (nz1[i] * nz1[i] - i1_s1sq) * int12_s1s2;
        k42_vec[i] = nz1[i] * nz2[i].powi(3)
            - 3.0 * nz1[i] * nz2[i] * i2_s2sq
            - 3.0 * (nz2[i] * nz2[i] - i2_s2sq) * int12_s1s2;
    }
    let k41 = weighted_mean(&k41_vec, weights);
    let k42 = weighted_mean(&k42_vec, weights);

    Ok(MomentEstimates {
        rho,
        k41,
        k42,
        intercept12,
        s1,
        s2,
        intercept1,
        intercept2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wls_slope_simple() {
        // y = 2x; β should be 2
        let x = vec![1.0, 2.0, 3.0, 4.0];
        let y = vec![2.0, 4.0, 6.0, 8.0];
        let w = vec![1.0; 4];
        assert!((wls_slope(&x, &y, &w) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn wls_intercept_simple() {
        // y = 3x + 1
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![4.0, 7.0, 10.0, 13.0, 16.0];
        let w = vec![1.0; 5];
        let (slope, int) = wls_intercept(&x, &y, &w);
        assert!((slope - 3.0).abs() < 1e-12, "slope {slope}");
        assert!((int - 1.0).abs() < 1e-12, "intercept {int}");
    }

    #[test]
    fn weighted_mean_uniform() {
        let y = vec![1.0, 2.0, 3.0, 4.0];
        let w = vec![1.0; 4];
        assert!((weighted_mean(&y, &w) - 2.5).abs() < 1e-12);
    }

    #[test]
    fn weighted_mean_weighted() {
        let y = vec![1.0, 2.0, 3.0, 4.0];
        let w = vec![0.0, 0.0, 1.0, 1.0];
        assert!((weighted_mean(&y, &w) - 3.5).abs() < 1e-12);
    }
}
