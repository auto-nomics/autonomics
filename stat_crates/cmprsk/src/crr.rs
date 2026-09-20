//! `crr()` — Fine & Gray (1999) proportional subdistribution hazards regression.
//!
//! Port of `reference/cmprsk/R/cmprsk.R:30-219`. The R function is a driver
//! around five Fortran kernels (see [`crate::kernels`]); everything else it does
//! — complete-case filtering, status recoding, the censoring-distribution KM,
//! the Newton–Raphson loop with backtracking, and the sandwich assembly — is
//! reproduced here.

use serde::{Deserialize, Serialize};

use crate::error::{CmprskError, Result};
use crate::kernels::{CrrData, crrf, crrfit, crrfsv, crrsr, crrvv};
use crate::km::censoring_weights;
use crate::linalg;

/// A function of time multiplying a `cov2` column.
///
/// R's `crr` takes an arbitrary closure for `tf`. That cannot round-trip
/// through a JSON node spec, so the DAG-facing surface uses this closed
/// vocabulary; [`TimeFunctions::Custom`] keeps arbitrary functions reachable
/// from Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimeFn {
    /// `t`
    Identity,
    /// `log(t)`
    Log,
    /// `sqrt(t)`
    Sqrt,
    /// `t^2`
    Square,
    /// `log(1 + t)`
    Log1p,
}

impl TimeFn {
    /// Evaluate at `t`.
    pub fn apply(self, t: f64) -> f64 {
        match self {
            TimeFn::Identity => t,
            TimeFn::Log => t.ln(),
            TimeFn::Sqrt => t.sqrt(),
            TimeFn::Square => t * t,
            TimeFn::Log1p => t.ln_1p(),
        }
    }

    /// Short label used to name the interaction term, mirroring R's
    /// `paste(cov2.vars, 'tf', 1:nc2, sep = '')` convention but readable.
    pub fn label(self) -> &'static str {
        match self {
            TimeFn::Identity => "t",
            TimeFn::Log => "log(t)",
            TimeFn::Sqrt => "sqrt(t)",
            TimeFn::Square => "t^2",
            TimeFn::Log1p => "log1p(t)",
        }
    }
}

/// How to build the `tfs` matrix from the unique failure times.
pub enum TimeFunctions<'a> {
    /// No `cov2` supplied.
    None,
    /// One [`TimeFn`] per `cov2` column.
    Fns(&'a [TimeFn]),
    /// Arbitrary function: takes the unique failure times and returns an
    /// `ndf × ncov2` row-major matrix. Equivalent to R's `tf` argument.
    Custom(&'a dyn Fn(&[f64]) -> Vec<Vec<f64>>),
}

/// Options mirroring `crr`'s scalar arguments.
#[derive(Debug, Clone)]
pub struct CrrOptions {
    /// Value of `fstatus` denoting the failure type of interest.
    pub failcode: f64,
    /// Value of `fstatus` denoting a censored observation.
    pub cencode: f64,
    /// Gradient tolerance for convergence.
    pub gtol: f64,
    /// Maximum number of Newton iterations (`0` scores at `init` only).
    pub maxiter: usize,
    /// Starting values; defaults to all zeros.
    pub init: Option<Vec<f64>>,
    /// Whether to compute the robust variance and score residuals.
    pub variance: bool,
}

impl Default for CrrOptions {
    fn default() -> Self {
        Self {
            failcode: 1.0,
            cencode: 0.0,
            gtol: 1e-6,
            maxiter: 10,
            init: None,
            variance: true,
        }
    }
}

/// The fitted model — the `crr` object.
#[derive(Debug, Clone)]
pub struct CrrFit {
    /// Estimated regression coefficients.
    pub coef: Vec<f64>,
    /// Term labels, one per coefficient.
    pub terms: Vec<String>,
    /// Log pseudo-likelihood at `coef`.
    pub loglik: f64,
    /// Derivatives of the log pseudo-likelihood at `coef`.
    pub score: Vec<f64>,
    /// `-` second derivatives of the log pseudo-likelihood (`$inf`).
    pub inf: Vec<Vec<f64>>,
    /// Robust (sandwich) variance–covariance matrix of `coef`.
    pub var: Vec<Vec<f64>>,
    /// Inverse of `inf` (`$invinf`).
    pub invinf: Vec<Vec<f64>>,
    /// Score residuals, `ndf × np`. `None` when `variance = false`.
    pub res: Option<Vec<Vec<f64>>>,
    /// Unique failure times of the cause of interest, ascending.
    pub uftime: Vec<f64>,
    /// Jumps in the Breslow-type baseline cumulative subdistribution hazard.
    pub bfitj: Vec<f64>,
    /// The `tfs` matrix (`ndf × ncov2`); empty when `cov2` is unused.
    pub tfs: Vec<Vec<f64>>,
    /// Whether the Newton iteration converged.
    pub converged: bool,
    /// Number of observations used.
    pub n: usize,
    /// Number of observations dropped for missing values.
    pub n_missing: usize,
    /// Log pseudo-likelihood at `β = 0`.
    pub loglik_null: f64,
    /// Number of failures of the cause of interest.
    pub n_events: usize,
    /// Number of fixed (`cov1`) covariates.
    pub ncov1: usize,
    /// Number of time-interacted (`cov2`) covariates.
    pub ncov2: usize,
}

/// Inputs to [`crr`]. Covariate matrices are row-major (`n` rows).
pub struct CrrInput<'a> {
    /// Failure / censoring times.
    pub ftime: &'a [f64],
    /// Failure-type codes.
    pub fstatus: &'a [f64],
    /// Fixed covariates, `n × nc1`. Empty when unused.
    pub cov1: &'a [Vec<f64>],
    /// Column names for `cov1`.
    pub cov1_names: &'a [String],
    /// Time-interacted covariates, `n × nc2`. Empty when unused.
    pub cov2: &'a [Vec<f64>],
    /// Column names for `cov2`.
    pub cov2_names: &'a [String],
    /// How to build `tfs`.
    pub tf: TimeFunctions<'a>,
    /// Censoring-distribution strata; a single group when `None`.
    pub cengroup: Option<&'a [f64]>,
}

/// Fit the Fine–Gray model.
pub fn crr(input: &CrrInput, opts: &CrrOptions) -> Result<CrrFit> {
    let n_in = input.ftime.len();
    if input.fstatus.len() != n_in {
        return Err(CmprskError::LengthMismatch {
            what: "fstatus",
            got: input.fstatus.len(),
            expected: n_in,
        });
    }
    let nc1 = input.cov1.first().map_or(0, |r| r.len());
    let nc2 = input.cov2.first().map_or(0, |r| r.len());
    if !input.cov1.is_empty() && input.cov1.len() != n_in {
        return Err(CmprskError::LengthMismatch {
            what: "cov1",
            got: input.cov1.len(),
            expected: n_in,
        });
    }
    if !input.cov2.is_empty() && input.cov2.len() != n_in {
        return Err(CmprskError::LengthMismatch {
            what: "cov2",
            got: input.cov2.len(),
            expected: n_in,
        });
    }
    if nc1 == 0 && nc2 == 0 {
        return Err(CmprskError::NoCovariates);
    }
    if let Some(cg) = input.cengroup
        && cg.len() != n_in
    {
        return Err(CmprskError::LengthMismatch {
            what: "cengroup",
            got: cg.len(),
            expected: n_in,
        });
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

    // ── covariate matrices ──────────────────────────────────────────────────
    let cov1: Vec<Vec<f64>> = if nc1 > 0 {
        keep.iter().map(|&i| input.cov1[i].clone()).collect()
    } else {
        Vec::new()
    };
    let cov2: Vec<Vec<f64>> = if nc2 > 0 {
        keep.iter().map(|&i| input.cov2[i].clone()).collect()
    } else {
        Vec::new()
    };

    let tfs: Vec<Vec<f64>> = if nc2 == 0 {
        Vec::new()
    } else {
        match &input.tf {
            TimeFunctions::None => {
                return Err(CmprskError::Invalid(
                    "cov2 supplied but tf is TimeFunctions::None".into(),
                ));
            }
            TimeFunctions::Fns(fns) => {
                if fns.len() != nc2 {
                    return Err(CmprskError::TfShapeMismatch {
                        ncov2: nc2,
                        ntf: fns.len(),
                    });
                }
                uft.iter()
                    .map(|&t| fns.iter().map(|f| f.apply(t)).collect())
                    .collect()
            }
            TimeFunctions::Custom(f) => {
                let m = f(&uft);
                if m.len() != ndf || m.iter().any(|r| r.len() != nc2) {
                    return Err(CmprskError::TfShapeMismatch {
                        ncov2: nc2,
                        ntf: m.first().map_or(0, |r| r.len()),
                    });
                }
                m
            }
        }
    };

    let np = nc1 + nc2;
    let data = CrrData {
        t2: &ftime,
        ici: &ici,
        x: &cov1,
        ncov: nc1,
        np,
        x2: &cov2,
        ncov2: nc2,
        tf: &tfs,
        ndf,
        wt: &uuu,
        ncg,
        icg: &icg,
    };

    // ── Newton–Raphson with Armijo backtracking (cmprsk.R:104-154) ──────────
    let mut b = match &opts.init {
        None => vec![0.0_f64; np],
        Some(v) => {
            if v.len() != np {
                return Err(CmprskError::BadInit {
                    got: v.len(),
                    expected: np,
                });
            }
            v.clone()
        }
    };
    let stepf = 0.5_f64;
    let mut converged = false;
    let mut z = crrfsv(&data, &b);

    for ll in 0..=opts.maxiter {
        z = crrfsv(&data, &b);

        // max(abs(score) * pmax(abs(b), 1)) < max(abs(lik), 1) * gtol
        let gmax =
            z.s.iter()
                .zip(b.iter())
                .map(|(s, bi)| s.abs() * bi.abs().max(1.0))
                .fold(f64::NEG_INFINITY, f64::max);
        if gmax < z.lik.abs().max(1.0) * opts.gtol {
            converged = true;
            break;
        }
        if ll == opts.maxiter {
            converged = false;
            break;
        }

        let step = linalg::solve(&z.v, &z.s, "Newton step")?;
        let mut sc: Vec<f64> = step.iter().map(|v| -v).collect();
        let mut bn: Vec<f64> = b.iter().zip(sc.iter()).map(|(x, y)| x + y).collect();
        let mut fbn = crrf(&data, &bn);

        let mut i = 0usize;
        loop {
            let armijo = z.lik + 1e-4 * sc.iter().zip(z.s.iter()).map(|(a, c)| a * c).sum::<f64>();
            if !(fbn.is_nan() || fbn > armijo) {
                break;
            }
            i += 1;
            for v in sc.iter_mut() {
                *v *= stepf;
            }
            bn = b.iter().zip(sc.iter()).map(|(x, y)| x + y).collect();
            fbn = crrf(&data, &bn);
            if i > 20 {
                break;
            }
        }
        if i > 20 {
            converged = false;
            break;
        }
        b = bn;
    }

    // ── variance (cmprsk.R:155-176) ─────────────────────────────────────────
    let nan_mat = || vec![vec![f64::NAN; np]; np];
    let (inf, var, invinf, res) = if opts.variance {
        let vv = crrvv(&data, &b);
        let h = linalg::inverse(&vv.v, "variance")?;
        let var = linalg::sandwich(&h, &vv.v2);
        let r = crrsr(&data, &b);
        (vv.v, var, h, Some(r))
    } else {
        (nan_mat(), nan_mat(), nan_mat(), None)
    };

    // ── null log-likelihood and baseline hazard jumps ───────────────────────
    let b0 = vec![0.0_f64; np];
    let fb0 = crrf(&data, &b0);
    let bfitj = crrfit(&data, &b);

    let terms = build_terms(input.cov1_names, input.cov2_names, &input.tf, nc1, nc2);

    Ok(CrrFit {
        coef: b,
        terms,
        loglik: -z.lik,
        score: z.s.iter().map(|v| -v).collect(),
        inf,
        var,
        invinf,
        res,
        uftime: uft,
        bfitj,
        tfs,
        converged,
        n,
        n_missing,
        loglik_null: -fb0,
        n_events,
        ncov1: nc1,
        ncov2: nc2,
    })
}

/// Shared preprocessing shared by [`crr`] and the penalized variant in
/// [`crate::ridge`]: complete-case filtering, stable time ordering, status
/// recoding, censoring groups, the censoring KM weights, and the unique
/// failure times of the cause of interest.
pub(crate) struct Prepared {
    pub keep: Vec<usize>,
    pub ftime: Vec<f64>,
    pub ici: Vec<u8>,
    pub icg: Vec<usize>,
    pub ncg: usize,
    pub uuu: Vec<Vec<f64>>,
    pub uft: Vec<f64>,
    pub n: usize,
    pub n_missing: usize,
    pub n_events: usize,
}

pub(crate) fn prepare(input: &CrrInput, failcode: f64, cencode: f64) -> Result<Prepared> {
    let n_in = input.ftime.len();
    let nc1 = input.cov1.first().map_or(0, |r| r.len());
    let nc2 = input.cov2.first().map_or(0, |r| r.len());

    // ── na.action = na.omit: drop rows with any missing value ───────────────
    let mut keep: Vec<usize> = Vec::with_capacity(n_in);
    for i in 0..n_in {
        let mut ok = input.ftime[i].is_finite() && input.fstatus[i].is_finite();
        if let Some(cg) = input.cengroup {
            ok &= cg[i].is_finite();
        }
        if ok && nc1 > 0 {
            ok &= input.cov1[i].iter().all(|v| v.is_finite());
        }
        if ok && nc2 > 0 {
            ok &= input.cov2[i].iter().all(|v| v.is_finite());
        }
        if ok {
            keep.push(i);
        }
    }
    let n_missing = n_in - keep.len();
    if keep.is_empty() {
        return Err(CmprskError::NoObservations);
    }

    // ── d <- d[order(d$ftime), ] — R's order() is stable ────────────────────
    keep.sort_by(|&a, &b| {
        input.ftime[a]
            .partial_cmp(&input.ftime[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let n = keep.len();

    let ftime: Vec<f64> = keep.iter().map(|&i| input.ftime[i]).collect();

    // ── status recoding (cmprsk.R:64-65) ────────────────────────────────────
    // cenind = 1 when censored; ici = 1 for the cause of interest, 2 for any
    // competing failure, 0 for censored.
    let mut cenind = vec![0u8; n];
    let mut ici = vec![0u8; n];
    for (k, &i) in keep.iter().enumerate() {
        let st = input.fstatus[i];
        cenind[k] = u8::from(st == cencode);
        ici[k] = if st == failcode {
            1
        } else {
            2 * (1 - cenind[k])
        };
    }

    // ── censoring groups: match(cengroup, sort(unique(cengroup))) ───────────
    let (icg, ncg) = match input.cengroup {
        None => (vec![0usize; n], 1usize),
        Some(cg) => {
            let mut ucg: Vec<f64> = keep.iter().map(|&i| cg[i]).collect();
            ucg.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            ucg.dedup();
            let idx: Vec<usize> = keep
                .iter()
                .map(|&i| {
                    ucg.iter()
                        .position(|&u| u == cg[i])
                        .expect("cengroup level present")
                })
                .collect();
            let ncg = ucg.len();
            (idx, ncg)
        }
    };

    let uuu = censoring_weights(&ftime, &cenind, &icg, ncg);

    // ── unique failure times of the cause of interest ───────────────────────
    let mut uft: Vec<f64> = ftime
        .iter()
        .zip(ici.iter())
        .filter(|&(_, &c)| c == 1)
        .map(|(&t, _)| t)
        .collect();
    let n_events = uft.len();
    uft.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    uft.dedup();
    if uft.is_empty() {
        return Err(CmprskError::NoEvents);
    }

    Ok(Prepared {
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
    })
}

/// Build term labels the way `cmprsk.R:197-211` does: `cov1` names verbatim,
/// `cov2` names suffixed with `*<time function>`.
fn build_terms(
    cov1_names: &[String],
    cov2_names: &[String],
    tf: &TimeFunctions<'_>,
    nc1: usize,
    nc2: usize,
) -> Vec<String> {
    let mut out = Vec::with_capacity(nc1 + nc2);
    for j in 0..nc1 {
        out.push(
            cov1_names
                .get(j)
                .cloned()
                .unwrap_or_else(|| format!("cov1{}", j + 1)),
        );
    }
    for j in 0..nc2 {
        let base = cov2_names
            .get(j)
            .cloned()
            .unwrap_or_else(|| format!("cov2{}", j + 1));
        let suffix = match tf {
            TimeFunctions::Fns(fns) => fns
                .get(j)
                .map(|f| f.label().to_string())
                .unwrap_or_else(|| format!("tf{}", j + 1)),
            _ => format!("tf{}", j + 1),
        };
        out.push(format!("{base}*{suffix}"));
    }
    out
}
