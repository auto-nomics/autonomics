//! OLS / WLS LD-score regression for MLE starting values — 1:1 port of
//! `HDL.L.R` lines 462-500 (Bulik-Sullivan-style).
//!
//! These regressions run on the **per-SNP** quantities (before the eigen
//! transform): `a11 = bhat1²`, `a22 = bhat2²`, `a12 = bhat1·bhat2` each
//! regressed on the per-SNP LD score `LDsc`. The resulting `(h², intercept)`
//! seeds the L-BFGS-B multi-start.
//!
//! R's `lm(y ~ x)` coefficients are recovered as:
//! - slope     = Σ(x−x̄)(y−ȳ) / Σ(x−x̄)²
//! - intercept = ȳ − slope·x̄
//!
//! and R's `summary(reg)$coef[1:2,1:2] * c(Nfac, M)` keeps only the estimate
//! column: `h² = slope·M` (or `slope·Nfac` for the gcov N-factor), `int =
//! intercept·Nfac`.

/// Starting values from LD-score regression: `(h², intercept)` for h11 / h22 /
/// h12 (OLS and WLS), plus the sample z-z correlation `rho12`.
#[derive(Debug, Clone, Default)]
pub struct StartValues {
    pub h11_ols: [f64; 2],
    pub h22_ols: [f64; 2],
    pub h12_ols: [f64; 2],
    pub h11_wls: [f64; 2],
    pub h22_wls: [f64; 2],
    pub h12_wls: [f64; 2],
    pub rho12: f64,
}

/// Weighted (or unweighted) linear regression `y ~ x`.
/// Returns `(intercept, slope)`. Weights `w = None` ⇒ OLS (w_i = 1).
fn lm_wls(y: &[f64], x: &[f64], w: Option<&[f64]>) -> (f64, f64) {
    let n = y.len() as f64;
    let (xbar, ybar) = match w {
        None => (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n),
        Some(w) => {
            let sw: f64 = w.iter().sum();
            let swx: f64 = w.iter().zip(x).map(|(a, b)| a * b).sum::<f64>() / sw;
            let swy: f64 = w.iter().zip(y).map(|(a, b)| a * b).sum::<f64>() / sw;
            (swx, swy)
        }
    };
    let (sxx, sxy) = match w {
        None => {
            let sxx: f64 = x.iter().map(|xi| (xi - xbar).powi(2)).sum();
            let sxy: f64 = x
                .iter()
                .zip(y)
                .map(|(xi, yi)| (xi - xbar) * (yi - ybar))
                .sum();
            (sxx, sxy)
        }
        Some(w) => {
            let sxx: f64 = w
                .iter()
                .zip(x)
                .map(|(wi, xi)| wi * (xi - xbar).powi(2))
                .sum::<f64>();
            let sxy: f64 = w
                .iter()
                .zip(x)
                .zip(y)
                .map(|((wi, xi), yi)| wi * (xi - xbar) * (yi - ybar))
                .sum::<f64>();
            (sxx, sxy)
        }
    };
    let slope = if sxx.abs() < 1e-300 { 0.0 } else { sxy / sxx };
    let intercept = ybar - slope * xbar;
    (intercept, slope)
}

/// Compute the HDL-L LD-score starting values.
///
/// - `bhat1`, `bhat2`, `ldsc`: per-SNP quantities (length `M`)
/// - `n1`, `n2`: per-trait sample sizes (R uses `median(N)`)
/// - `n0`: sample overlap (`0` for independent cohorts, the HDL-L default)
/// - `rho12`: Z-Z correlation on shared SNPs (only used when `n0 > 0`)
///
/// Returns `(h², intercept)` pairs for h11/h22/h12 under both OLS and WLS.
pub fn start_values(
    bhat1: &[f64],
    bhat2: &[f64],
    ldsc: &[f64],
    n1: f64,
    n2: f64,
    n0: f64,
    rho12: f64,
) -> StartValues {
    let m = ldsc.len() as f64;
    let a11: Vec<f64> = bhat1.iter().map(|b| b * b).collect();
    let a22: Vec<f64> = bhat2.iter().map(|b| b * b).collect();
    let a12: Vec<f64> = bhat1.iter().zip(bhat2).map(|(a, b)| a * b).collect();

    // gcov N-factor: N0>0 → N1·N2/N0 ; N0==0 → √(N1·N2) (= R's `N`).
    let nfac12 = if n0 > 0.0 {
        let p1 = n0 / n1;
        let p2 = n0 / n2;
        n0 / p1 / p2
    } else {
        n1.sqrt() * n2.sqrt()
    };
    let p1 = n0 / n1;
    let p2 = n0 / n2;

    // ---- OLS ----
    let (i11, s11) = lm_wls(&a11, ldsc, None);
    let (i22, s22) = lm_wls(&a22, ldsc, None);
    let (i12, s12) = lm_wls(&a12, ldsc, None);
    let h11_ols = [i11 * n1, s11 * m];
    let h22_ols = [i22 * n2, s22 * m];
    let h12_ols = [i12 * nfac12, s12 * m];

    // ---- WLS (weights from Bulik-Sullivan variance estimates) ----
    let h11v: Vec<f64> = ldsc
        .iter()
        .map(|l| (h11_ols[1] * l / m + 1.0 / n1).powi(2))
        .collect();
    let h22v: Vec<f64> = ldsc
        .iter()
        .map(|l| (h22_ols[1] * l / m + 1.0 / n2).powi(2))
        .collect();
    let w11: Vec<f64> = h11v.iter().map(|v| 1.0 / v).collect();
    let w22: Vec<f64> = h22v.iter().map(|v| 1.0 / v).collect();
    let (wi11, ws11) = lm_wls(&a11, ldsc, Some(&w11));
    let (wi22, ws22) = lm_wls(&a22, ldsc, Some(&w22));
    let h11_wls = [wi11 * n1, ws11 * m];
    let h22_wls = [wi22 * n2, ws22 * m];

    let h12v: Vec<f64> = ldsc
        .iter()
        .enumerate()
        .map(|(k, l)| {
            let base = (h12_ols[1] * l / m).powi(2);
            if n0 > 0.0 {
                (h11v[k] * h22v[k]).sqrt() + (h12_ols[1] * l / m + p1 * p2 * rho12 / n0).powi(2)
            } else {
                (h11v[k] * h22v[k]).sqrt() + base
            }
        })
        .collect();
    let w12: Vec<f64> = h12v.iter().map(|v| 1.0 / v).collect();
    let (wi12, ws12) = lm_wls(&a12, ldsc, Some(&w12));
    let h12_wls = [wi12 * nfac12, ws12 * m];

    StartValues {
        h11_ols,
        h22_ols,
        h12_ols,
        h11_wls,
        h22_wls,
        h12_wls,
        rho12,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ols_recovers_line() {
        // y = 2 + 3x exactly
        let x = vec![0.0_f64, 1.0, 2.0, 3.0, 4.0];
        let y: Vec<f64> = x.iter().map(|v| 2.0 + 3.0 * v).collect();
        let (i, s) = lm_wls(&y, &x, None);
        assert!((i - 2.0).abs() < 1e-9 && (s - 3.0).abs() < 1e-9);
    }

    #[test]
    fn start_values_smoke() {
        let ldsc = vec![1.0_f64, 2.0, 3.0, 1.5, 2.5];
        let b1 = vec![0.01_f64, 0.02, -0.01, 0.03, 0.015];
        let b2 = vec![0.02_f64, -0.01, 0.03, 0.01, 0.025];
        let sv = start_values(&b1, &b2, &ldsc, 1000.0, 2000.0, 0.0, 0.0);
        // h² estimates should be finite and in a sane range.
        for v in sv
            .h11_wls
            .iter()
            .chain(sv.h22_wls.iter())
            .chain(sv.h12_wls.iter())
        {
            assert!(v.is_finite(), "non-finite start value");
        }
    }
}
