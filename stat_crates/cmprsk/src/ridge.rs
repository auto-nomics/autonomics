//! Ridge-penalized Fine–Gray regression for prediction-oriented fits where
//! the covariate count rivals the event count (radiomics fusion models).
//!
//! The estimator is the Fine–Gray pseudo-likelihood with an L2 penalty on
//! standardized covariates,
//!
//! ```text
//!   minimize  −ℓ(β) + (λ/2) · Σ_{j ∈ P} β_j²
//! ```
//!
//! solved by the same Armijo-damped Newton iteration as [`crate::crr`] with
//! the penalty added to the gradient (`+λβ_j`) and Hessian (`+λ` on the
//! diagonal) of the penalized columns. Everything else — complete-case
//! filtering, status recoding, censoring KM weights, the Breslow-type
//! baseline — is shared with the unpenalized driver through
//! [`crate::crr::prepare`], so the two estimators differ only by the penalty.
//!
//! Two conventions matter for the SAP's fusion models:
//!
//! * columns listed in [`CrrRidgeOptions::unpenalized`] (the clinical
//!   covariates) are exempt from the penalty — they always enter at full
//!   strength, exactly as the pre-specified clinical model requires — while
//!   the high-dimensional biomarker columns shrink;
//! * `λ` is caller-supplied. It is selected upstream by cross-validation
//!   inside the leak-free pipeline (never on the evaluation fold), so this
//!   function itself never sees evaluation data.
//!
//! The reported `var` is the sandwich `(H+Λ)⁻¹ V2 (H+Λ)⁻¹` transformed to the
//! original covariate scale. It is conditional on `λ` and biased under
//! selection: use it for prediction calibration, not confirmatory inference —
//! hypothesis tests belong to the unpenalized clinical model.

use crate::crr::{CrrInput, Prepared, prepare};
use crate::error::{CmprskError, Result};
use crate::kernels::{CrrData, crrf, crrfit, crrfsv, crrvv};
use crate::linalg;
use crate::predict::CrrPrediction;

/// Options for [`crr_ridge`].
#[derive(Debug, Clone)]
pub struct CrrRidgeOptions {
    /// Value of `fstatus` denoting the failure type of interest.
    pub failcode: f64,
    /// Value of `fstatus` denoting a censored observation.
    pub cencode: f64,
    /// Ridge penalty `λ ≥ 0` on the standardized scale.
    pub lambda: f64,
    /// Indices into `cov1` exempt from the penalty (clinical covariates).
    pub unpenalized: Vec<usize>,
    /// Standardize covariate columns before penalizing. Recommended: it makes
    /// `λ` scale-free. When `false` the penalty acts on raw covariates.
    pub standardize: bool,
    /// Gradient tolerance for convergence.
    pub gtol: f64,
    /// Maximum number of Newton iterations.
    pub maxiter: usize,
}

impl Default for CrrRidgeOptions {
    fn default() -> Self {
        Self {
            failcode: 1.0,
            cencode: 0.0,
            lambda: 1.0,
            unpenalized: Vec::new(),
            standardize: true,
            gtol: 1e-6,
            maxiter: 50,
        }
    }
}

/// The fitted ridge Fine–Gray model.
#[derive(Debug, Clone)]
pub struct CrrRidgeFit {
    /// Coefficients on the **original** covariate scale.
    pub coef: Vec<f64>,
    /// Implicit intercept on the original scale, `-Σ b_j·center_j/scale_j`;
    /// the linear predictor is `x·coef + intercept`.
    pub intercept: f64,
    /// Coefficients on the standardized scale the penalty acted on.
    pub coef_standardized: Vec<f64>,
    /// Column means used by the standardization (zeros when disabled).
    pub center: Vec<f64>,
    /// Column scales used by the standardization (ones when disabled).
    pub scale: Vec<f64>,
    /// The penalty multiplier used.
    pub lambda: f64,
    /// Indices of unpenalized columns.
    pub unpenalized: Vec<usize>,
    /// Term labels, one per coefficient.
    pub terms: Vec<String>,
    /// Unpenalized log pseudo-likelihood at the solution (comparable with
    /// [`crate::CrrFit::loglik`]).
    pub loglik: f64,
    /// Penalized objective at the solution: `-loglik + (λ/2)Σβ_j²`.
    pub objective_penalized: f64,
    /// Sandwich variance on the original scale, conditional on `λ`.
    pub var: Vec<Vec<f64>>,
    /// Whether the Newton iteration converged.
    pub converged: bool,
    /// Newton updates performed.
    pub n_iter: usize,
    /// Unique failure times of the cause of interest, ascending.
    pub uftime: Vec<f64>,
    /// Jumps in the Breslow-type baseline cumulative subdistribution hazard
    /// at the standardized origin (covariates at their column means).
    pub bfitj: Vec<f64>,
    /// Number of observations used.
    pub n: usize,
    /// Number of observations dropped for missing values.
    pub n_missing: usize,
    /// Number of failures of the cause of interest.
    pub n_events: usize,
    /// Number of `cov1` covariates.
    pub ncov1: usize,
}

impl CrrRidgeFit {
    /// Linear predictor `x·coef + intercept` for one covariate row.
    pub fn linear_predictor(&self, row: &[f64]) -> Result<f64> {
        if row.len() != self.ncov1 {
            return Err(CmprskError::Predict(format!(
                "covariate row must have {} columns, got {}",
                self.ncov1,
                row.len()
            )));
        }
        Ok(self.intercept
            + row
                .iter()
                .zip(self.coef.iter())
                .map(|(x, b)| x * b)
                .sum::<f64>())
    }
}

/// Fit the ridge-penalized Fine–Gray model. `cov1` only — time-interacted
/// `cov2` terms are out of scope for the penalized estimator.
pub fn crr_ridge(input: &CrrInput, opts: &CrrRidgeOptions) -> Result<CrrRidgeFit> {
    if !input.cov2.is_empty() {
        return Err(CmprskError::Invalid(
            "crr_ridge does not support time-interacted cov2 terms".into(),
        ));
    }
    if !(opts.lambda.is_finite() && opts.lambda >= 0.0) {
        return Err(CmprskError::Invalid(
            "lambda must be finite and nonnegative".into(),
        ));
    }
    let nc1 = input.cov1.first().map_or(0, |r| r.len());
    if input.cov1.len() != input.ftime.len() {
        return Err(CmprskError::LengthMismatch {
            what: "cov1",
            got: input.cov1.len(),
            expected: input.ftime.len(),
        });
    }
    if let Some(&bad) = opts.unpenalized.iter().find(|&&j| j >= nc1) {
        return Err(CmprskError::Invalid(format!(
            "unpenalized index {bad} is outside cov1 (0..{nc1})"
        )));
    }

    let prepared = prepare(input, opts.failcode, opts.cencode)?;
    let Prepared {
        keep,
        ftime,
        ici,
        icg,
        ncg,
        uuu,
        uft,
        n,
        n_missing,
        n_events,
    } = prepared;
    let ndf = uft.len();
    let np = nc1;
    if np == 0 {
        return Err(CmprskError::NoCovariates);
    }

    // ── standardization: column means/scales over the analysis sample ───────
    let mut center = vec![0.0_f64; np];
    let mut scale = vec![1.0_f64; np];
    if opts.standardize {
        for j in 0..np {
            let mut mean = 0.0_f64;
            for &i in &keep {
                mean += input.cov1[i][j];
            }
            mean /= n as f64;
            let mut ss = 0.0_f64;
            for &i in &keep {
                let d = input.cov1[i][j] - mean;
                ss += d * d;
            }
            let sd = (ss / n as f64).sqrt();
            if !(sd > 0.0 && sd.is_finite()) {
                return Err(CmprskError::Invalid(format!(
                    "covariate column {j} is constant on the analysis sample; \
                     standardize=false penalizes raw columns instead"
                )));
            }
            center[j] = mean;
            scale[j] = sd;
        }
    }

    let cov1_std: Vec<Vec<f64>> = keep
        .iter()
        .map(|&i| {
            (0..np)
                .map(|j| (input.cov1[i][j] - center[j]) / scale[j])
                .collect()
        })
        .collect();

    let data = CrrData {
        t2: &ftime,
        ici: &ici,
        x: &cov1_std,
        ncov: np,
        np,
        x2: &[],
        ncov2: 0,
        tf: &[],
        ndf,
        wt: &uuu,
        ncg,
        icg: &icg,
    };

    // λ per column: the penalty only touches columns not exempted.
    let lambda_eff: Vec<f64> = (0..np)
        .map(|j| {
            if opts.unpenalized.contains(&j) {
                0.0
            } else {
                opts.lambda
            }
        })
        .collect();

    // ── Armijo-damped Newton on the penalized objective ─────────────────────
    let mut b = vec![0.0_f64; np];
    let mut converged = false;
    let mut n_iter = 0usize;

    let penalized_objective = |lik: f64, b: &[f64]| -> f64 {
        lik + 0.5
            * lambda_eff
                .iter()
                .zip(b.iter())
                .map(|(&l, &bj)| l * bj * bj)
                .sum::<f64>()
    };
    let penalized_gradient = |s: &[f64], b: &[f64]| -> Vec<f64> {
        s.iter()
            .zip(b.iter())
            .zip(lambda_eff.iter())
            .map(|((&sj, &bj), &l)| sj + l * bj)
            .collect()
    };

    for ll in 0..=opts.maxiter {
        let z = crrfsv(&data, &b);
        let g = penalized_gradient(&z.s, &b);
        let f_pen = penalized_objective(z.lik, &b);

        let gmax = g
            .iter()
            .zip(b.iter())
            .map(|(sj, bj)| sj.abs() * bj.abs().max(1.0))
            .fold(f64::NEG_INFINITY, f64::max);
        if gmax < f_pen.abs().max(1.0) * opts.gtol {
            converged = true;
            break;
        }
        if ll == opts.maxiter {
            converged = false;
            break;
        }

        let h_pen: Vec<Vec<f64>> =
            z.v.iter()
                .enumerate()
                .map(|(j, row)| {
                    row.iter()
                        .enumerate()
                        .map(|(k, &v)| if j == k { v + lambda_eff[j] } else { v })
                        .collect()
                })
                .collect();
        let step = linalg::solve(&h_pen, &g, "ridge Newton step")?;
        let mut sc: Vec<f64> = step.iter().map(|v| -v).collect();
        let mut bn: Vec<f64> = b.iter().zip(sc.iter()).map(|(x, y)| x + y).collect();
        let mut fbn = penalized_objective(crrf(&data, &bn), &bn);

        let mut i = 0usize;
        loop {
            let armijo = f_pen + 1e-4 * sc.iter().zip(g.iter()).map(|(a, c)| a * c).sum::<f64>();
            if !(fbn.is_nan() || fbn > armijo) {
                break;
            }
            i += 1;
            for v in sc.iter_mut() {
                *v *= 0.5;
            }
            bn = b.iter().zip(sc.iter()).map(|(x, y)| x + y).collect();
            fbn = penalized_objective(crrf(&data, &bn), &bn);
            if i > 20 {
                break;
            }
        }
        if i > 20 {
            converged = false;
            break;
        }
        b = bn;
        n_iter += 1;
    }

    // ── variance conditional on λ: (H+Λ)⁻¹ V2 (H+Λ)⁻¹, back-transformed ─────
    let vv = crrvv(&data, &b);
    let h_pen: Vec<Vec<f64>> =
        vv.v.iter()
            .enumerate()
            .map(|(j, row)| {
                row.iter()
                    .enumerate()
                    .map(|(k, &v)| if j == k { v + lambda_eff[j] } else { v })
                    .collect()
            })
            .collect();
    let bread = linalg::inverse(&h_pen, "ridge variance")?;
    let var_std = linalg::sandwich(&bread, &vv.v2);
    let var = var_std
        .iter()
        .enumerate()
        .map(|(j, row)| {
            row.iter()
                .enumerate()
                .map(|(k, &v)| v / (scale[j] * scale[k]))
                .collect()
        })
        .collect();

    // ── back-transform and baseline ─────────────────────────────────────────
    let coef: Vec<f64> = b
        .iter()
        .zip(scale.iter())
        .map(|(&bj, &sj)| bj / sj)
        .collect();
    let intercept = -b
        .iter()
        .zip(center.iter())
        .zip(scale.iter())
        .map(|((&bj, &mj), &sj)| bj * mj / sj)
        .sum::<f64>();
    let bfitj = crrfit(&data, &b);
    let loglik = -crrf(&data, &b);
    let objective_penalized = penalized_objective(-loglik, &b);

    let terms: Vec<String> = (0..np)
        .map(|j| {
            input
                .cov1_names
                .get(j)
                .cloned()
                .unwrap_or_else(|| format!("cov1{}", j + 1))
        })
        .collect();

    Ok(CrrRidgeFit {
        coef,
        intercept,
        coef_standardized: b,
        center,
        scale,
        lambda: opts.lambda,
        unpenalized: opts.unpenalized.clone(),
        terms,
        loglik,
        objective_penalized,
        var,
        converged,
        n_iter,
        uftime: uft,
        bfitj,
        n,
        n_missing,
        n_events,
        ncov1: np,
    })
}

/// Predicted cumulative incidence for each row of `cov1` (original scale).
///
/// `F(t; x) = 1 - exp(-Λ̂₀(t) · exp(x·coef + intercept))`, with `Λ̂₀` the
/// cumulative sum of [`CrrRidgeFit::bfitj`].
pub fn predict_crr_ridge(fit: &CrrRidgeFit, cov1: &[Vec<f64>]) -> Result<CrrPrediction> {
    if cov1.is_empty() {
        return Err(CmprskError::Predict("no covariate rows supplied".into()));
    }
    if cov1.iter().any(|r| r.len() != fit.ncov1) {
        return Err(CmprskError::Predict(format!(
            "each covariate row must have {} columns",
            fit.ncov1
        )));
    }
    let mut curves = Vec::with_capacity(cov1.len());
    for row in cov1 {
        let eta = fit.linear_predictor(row)?.exp();
        let mut acc = 0.0_f64;
        curves.push(
            fit.bfitj
                .iter()
                .map(|j| {
                    acc += j * eta;
                    1.0 - (-acc).exp()
                })
                .collect(),
        );
    }
    Ok(CrrPrediction {
        uftime: fit.uftime.clone(),
        curves,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crr::{CrrOptions, TimeFunctions, crr};
    use crate::predict::predict_crr;

    /// Deterministic xorshift64* — dependency-free pseudo-randomness for tests.
    struct XorShift(u64);
    impl XorShift {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn uniform(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
        fn exponential(&mut self, rate: f64) -> f64 {
            -(self.uniform().max(1e-12)).ln() / rate
        }
    }

    /// Competing-risk sample: latent exponential cause-1 (signal in `beta1`),
    /// cause-2, and independent censoring.
    fn simulate(n: usize, beta1: &[f64], seed: u64) -> (Vec<f64>, Vec<f64>, Vec<Vec<f64>>) {
        let p = beta1.len();
        let mut rng = XorShift(seed);
        let mut ftime = Vec::with_capacity(n);
        let mut fstatus = Vec::with_capacity(n);
        let mut cov1 = Vec::with_capacity(n);
        for _ in 0..n {
            let row: Vec<f64> = (0..p).map(|_| rng.uniform() * 2.0 - 1.0).collect();
            let eta: f64 = row.iter().zip(beta1.iter()).map(|(x, b)| x * b).sum();
            let t1 = rng.exponential(0.05 * eta.exp());
            let t2 = rng.exponential(0.06);
            let c = rng.exponential(0.04);
            let (t, s) = if t1 <= t2 && t1 <= c {
                (t1, 1.0)
            } else if t2 < t1 && t2 <= c {
                (t2, 2.0)
            } else {
                (c, 0.0)
            };
            ftime.push(t);
            fstatus.push(s);
            cov1.push(row);
        }
        (ftime, fstatus, cov1)
    }

    fn names(p: usize) -> Vec<String> {
        (0..p).map(|j| format!("x{j}")).collect()
    }

    #[test]
    fn lambda_zero_matches_unpenalized_crr() {
        let (ftime, fstatus, cov1) = simulate(220, &[0.7, -0.5], 11);
        let nm = names(2);
        let input = CrrInput {
            ftime: &ftime,
            fstatus: &fstatus,
            cov1: &cov1,
            cov1_names: &nm,
            cov2: &[],
            cov2_names: &[],
            tf: TimeFunctions::None,
            cengroup: None,
        };
        let plain = crr(
            &input,
            &CrrOptions {
                maxiter: 100,
                gtol: 1e-9,
                ..Default::default()
            },
        )
        .unwrap();
        let ridge = crr_ridge(
            &input,
            &CrrRidgeOptions {
                lambda: 0.0,
                maxiter: 200,
                gtol: 1e-9,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(plain.converged);
        assert!(ridge.converged);
        let rows = vec![vec![0.3, -0.8], vec![-1.1, 0.2], vec![0.0, 0.0]];
        let p_plain = predict_crr(&plain, &rows, &[]).unwrap();
        let p_ridge = predict_crr_ridge(&ridge, &rows).unwrap();
        assert_eq!(p_plain.uftime.len(), p_ridge.uftime.len());
        for (a, b) in p_plain.curves.iter().zip(p_ridge.curves.iter()) {
            for (ca, cb) in a.iter().zip(b.iter()) {
                assert!((ca - cb).abs() < 5e-4, "curves differ: {ca} vs {cb}");
            }
        }
    }

    #[test]
    fn large_lambda_shrinks_penalized_but_not_exempt_columns() {
        let (ftime, fstatus, cov1) = simulate(240, &[0.8, 0.6], 23);
        let nm = names(2);
        let input = CrrInput {
            ftime: &ftime,
            fstatus: &fstatus,
            cov1: &cov1,
            cov1_names: &nm,
            cov2: &[],
            cov2_names: &[],
            tf: TimeFunctions::None,
            cengroup: None,
        };
        let full = crr(
            &input,
            &CrrOptions {
                maxiter: 100,
                gtol: 1e-8,
                ..Default::default()
            },
        )
        .unwrap();
        // Clinical column 0 exempt, biomarker column 1 penalized.
        let shrunk = crr_ridge(
            &input,
            &CrrRidgeOptions {
                lambda: 400.0,
                unpenalized: vec![0],
                maxiter: 200,
                gtol: 1e-8,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(shrunk.converged);
        assert!(
            shrunk.coef[1].abs() < full.coef[1].abs() * 0.35,
            "penalized column must shrink: {} vs {}",
            shrunk.coef[1],
            full.coef[1]
        );
        // The exempt clinical coefficient stays near the unpenalized fit.
        assert!(
            (shrunk.coef[0] - full.coef[0]).abs() < 0.15 * full.coef[0].abs().max(0.15),
            "exempt column drifted: {} vs {}",
            shrunk.coef[0],
            full.coef[0]
        );
    }

    #[test]
    fn standardization_makes_penalty_scale_free() {
        let (ftime, fstatus, cov1) = simulate(200, &[0.9], 37);
        let doubled: Vec<Vec<f64>> = cov1.iter().map(|r| vec![r[0] * 2.0]).collect();
        let nm = names(1);
        let fit_a = crr_ridge(
            &CrrInput {
                ftime: &ftime,
                fstatus: &fstatus,
                cov1: &cov1,
                cov1_names: &nm,
                cov2: &[],
                cov2_names: &[],
                tf: TimeFunctions::None,
                cengroup: None,
            },
            &CrrRidgeOptions {
                lambda: 3.0,
                maxiter: 200,
                gtol: 1e-9,
                ..Default::default()
            },
        )
        .unwrap();
        let fit_b = crr_ridge(
            &CrrInput {
                ftime: &ftime,
                fstatus: &fstatus,
                cov1: &doubled,
                cov1_names: &nm,
                cov2: &[],
                cov2_names: &[],
                tf: TimeFunctions::None,
                cengroup: None,
            },
            &CrrRidgeOptions {
                lambda: 3.0,
                maxiter: 200,
                gtol: 1e-9,
                ..Default::default()
            },
        )
        .unwrap();
        let rows = vec![vec![0.4], vec![-0.9]];
        let rows_b = vec![vec![0.8], vec![-1.8]];
        let a = predict_crr_ridge(&fit_a, &rows).unwrap();
        let b = predict_crr_ridge(&fit_b, &rows_b).unwrap();
        for (ca, cb) in a.curves.iter().zip(b.curves.iter()) {
            for (x, y) in ca.iter().zip(cb.iter()) {
                assert!((x - y).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn high_dimensional_fit_converges_and_predicts() {
        // 80 subjects, 30 covariates, ~15% cause-1 events: the regime where
        // the unpenalized estimator is unstable and ridge is needed.
        let beta: Vec<f64> = (0..30).map(|j| if j < 3 { 0.6 } else { 0.0 }).collect();
        let (ftime, fstatus, cov1) = simulate(80, &beta, 5);
        let nm = names(30);
        let input = CrrInput {
            ftime: &ftime,
            fstatus: &fstatus,
            cov1: &cov1,
            cov1_names: &nm,
            cov2: &[],
            cov2_names: &[],
            tf: TimeFunctions::None,
            cengroup: None,
        };
        let fit = crr_ridge(
            &input,
            &CrrRidgeOptions {
                lambda: 5.0,
                maxiter: 300,
                gtol: 1e-7,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(fit.converged);
        assert!(fit.coef.iter().all(|c| c.is_finite()));
        let pred = predict_crr_ridge(&fit, &cov1[..5]).unwrap();
        for curve in &pred.curves {
            assert!(curve.iter().all(|v| (0.0..=1.0).contains(v)));
            assert!(curve.windows(2).all(|w| w[0] <= w[1] + 1e-12));
        }
    }

    #[test]
    fn rejects_bad_options() {
        let (ftime, fstatus, cov1) = simulate(50, &[0.5], 3);
        let nm = names(1);
        let input = CrrInput {
            ftime: &ftime,
            fstatus: &fstatus,
            cov1: &cov1,
            cov1_names: &nm,
            cov2: &[],
            cov2_names: &[],
            tf: TimeFunctions::None,
            cengroup: None,
        };
        assert!(
            crr_ridge(
                &input,
                &CrrRidgeOptions {
                    lambda: -1.0,
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(
            crr_ridge(
                &input,
                &CrrRidgeOptions {
                    unpenalized: vec![7],
                    ..Default::default()
                }
            )
            .is_err()
        );

        let constant: Vec<Vec<f64>> = vec![vec![1.0]; 50];
        let bad = CrrInput {
            ftime: &ftime,
            fstatus: &fstatus,
            cov1: &constant,
            cov1_names: &nm,
            cov2: &[],
            cov2_names: &[],
            tf: TimeFunctions::None,
            cengroup: None,
        };
        assert!(crr_ridge(&bad, &CrrRidgeOptions::default()).is_err());
        assert!(
            crr_ridge(
                &bad,
                &CrrRidgeOptions {
                    standardize: false,
                    ..Default::default()
                }
            )
            .is_ok()
        );

        let with_cov2 = CrrInput {
            ftime: &ftime,
            fstatus: &fstatus,
            cov1: &cov1,
            cov1_names: &nm,
            cov2: &cov1,
            cov2_names: &nm,
            tf: TimeFunctions::Fns(&[crate::TimeFn::Identity]),
            cengroup: None,
        };
        assert!(crr_ridge(&with_cov2, &CrrRidgeOptions::default()).is_err());
    }
}
