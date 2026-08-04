//! `cuminc()` — nonparametric cumulative incidence functions and Gray's
//! stratified k-sample test.
//!
//! Port of `reference/cmprsk/R/cmprsk.R:352-486`, over the Fortran in
//! [`cinc`] and [`crst`].

pub mod cinc;
pub mod crst;

use crate::error::{CmprskError, Result};
use crate::summary::chisq_upper_tail;

pub use cinc::{CincOut, tpoi};
pub use crst::{CrstmOut, crstm};

/// One estimated cumulative incidence curve (`group` × `cause`).
#[derive(Debug, Clone)]
pub struct CumincCurve {
    /// Display label for the group level.
    pub group: String,
    /// Display label for the cause.
    pub cause: String,
    /// 0-based group level index.
    pub group_index: usize,
    /// Numeric cause code.
    pub cause_value: f64,
    /// Step-function times (corners).
    pub time: Vec<f64>,
    /// Cumulative incidence estimates.
    pub est: Vec<f64>,
    /// Variance estimates.
    pub var: Vec<f64>,
}

/// One row of the `$Tests` component.
#[derive(Debug, Clone)]
pub struct GrayTest {
    /// Display label for the cause.
    pub cause: String,
    /// Numeric cause code.
    pub cause_value: f64,
    /// Test statistic `s' V⁻¹ s`; `-1` when `V` is rank-deficient (R's
    /// sentinel).
    pub stat: f64,
    /// `1 - pchisq(stat, ng - 1)`.
    pub p_value: f64,
    /// Degrees of freedom, `ng - 1`.
    pub df: usize,
}

/// Result of [`cuminc`].
#[derive(Debug, Clone)]
pub struct CumincResult {
    /// Curves, ordered cause-major then group (matching R's list order).
    pub curves: Vec<CumincCurve>,
    /// Gray's tests, one per cause. Empty when there is a single group.
    pub tests: Vec<GrayTest>,
    /// Observations used.
    pub n: usize,
    /// Observations dropped for missing values.
    pub n_missing: usize,
    /// Number of groups.
    pub n_groups: usize,
}

/// Options mirroring `cuminc`'s scalar arguments.
#[derive(Debug, Clone)]
pub struct CumincOptions {
    /// Power of the weight function in Gray's test.
    pub rho: f64,
    /// Value of `fstatus` denoting a censored observation.
    pub cencode: f64,
}

impl Default for CumincOptions {
    fn default() -> Self {
        Self {
            rho: 0.0,
            cencode: 0.0,
        }
    }
}

/// Inputs to [`cuminc`].
///
/// `group` and `strata` are numeric; their levels are the sorted unique values,
/// which is exactly what R's `as.factor` produces for a numeric vector. Callers
/// with string groups map them to ordered codes themselves and pass the labels
/// through `group_labels`.
pub struct CumincInput<'a> {
    /// Failure / censoring times.
    pub ftime: &'a [f64],
    /// Failure-type codes.
    pub fstatus: &'a [f64],
    /// Group membership; a single group when `None`.
    pub group: Option<&'a [f64]>,
    /// Display labels indexed by group level.
    pub group_labels: Option<&'a [String]>,
    /// Strata for the test; a single stratum when `None`.
    pub strata: Option<&'a [f64]>,
}

/// Format a numeric code the way R's `paste()` would, so curve names match.
fn fmt_code(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Map values to 0-based level indices using sorted-unique ordering
/// (`as.factor` semantics for numeric input).
fn factorise(values: &[f64]) -> (Vec<usize>, Vec<f64>) {
    let mut levels: Vec<f64> = values.to_vec();
    levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    levels.dedup();
    let codes = values
        .iter()
        .map(|v| levels.iter().position(|u| u == v).expect("level present"))
        .collect();
    (codes, levels)
}

/// Estimate cumulative incidence functions and, when there is more than one
/// group, Gray's stratified k-sample test for each cause.
pub fn cuminc(input: &CumincInput, opts: &CumincOptions) -> Result<CumincResult> {
    let n_in = input.ftime.len();
    if input.fstatus.len() != n_in {
        return Err(CmprskError::LengthMismatch {
            what: "fstatus",
            got: input.fstatus.len(),
            expected: n_in,
        });
    }
    for (what, v) in [("group", input.group), ("strata", input.strata)] {
        if let Some(v) = v
            && v.len() != n_in
        {
            return Err(CmprskError::LengthMismatch {
                what,
                got: v.len(),
                expected: n_in,
            });
        }
    }

    // ── na.omit ─────────────────────────────────────────────────────────────
    let mut keep: Vec<usize> = Vec::with_capacity(n_in);
    for i in 0..n_in {
        let mut ok = input.ftime[i].is_finite() && input.fstatus[i].is_finite();
        if let Some(g) = input.group {
            ok &= g[i].is_finite();
        }
        if let Some(s) = input.strata {
            ok &= s[i].is_finite();
        }
        if ok {
            keep.push(i);
        }
    }
    let n_missing = n_in - keep.len();
    if keep.is_empty() {
        return Err(CmprskError::NoObservations);
    }

    // ── d <- d[order(d$time), ] — stable ────────────────────────────────────
    keep.sort_by(|&a, &b| {
        input.ftime[a]
            .partial_cmp(&input.ftime[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let no = keep.len();

    let time: Vec<f64> = keep.iter().map(|&i| input.ftime[i]).collect();
    let cause: Vec<f64> = keep.iter().map(|&i| input.fstatus[i]).collect();

    let group_raw: Vec<f64> = match input.group {
        Some(g) => keep.iter().map(|&i| g[i]).collect(),
        None => vec![1.0; no],
    };
    let strata_raw: Vec<f64> = match input.strata {
        Some(s) => keep.iter().map(|&i| s[i]).collect(),
        None => vec![1.0; no],
    };
    let (gcode, glevels) = factorise(&group_raw);
    let (scode, slevels) = factorise(&strata_raw);
    let ng = glevels.len();
    let nst = slevels.len();

    let group_names: Vec<String> = (0..ng)
        .map(|k| match input.group_labels {
            Some(l) if k < l.len() => l[k].clone(),
            _ => fmt_code(glevels[k]),
        })
        .collect();

    // censind = 0 when censored, 1 otherwise
    let censind: Vec<u8> = cause.iter().map(|&c| u8::from(c != opts.cencode)).collect();

    // uclab = sort(unique(cause[censind == 1]))
    let mut uclab: Vec<f64> = cause
        .iter()
        .zip(censind.iter())
        .filter(|&(_, &ci)| ci == 1)
        .map(|(&c, _)| c)
        .collect();
    uclab.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    uclab.dedup();
    let nc = uclab.len();

    let mut curves = Vec::with_capacity(ng * nc);
    let mut tests = Vec::with_capacity(nc);

    for &uc in uclab.iter().take(nc) {
        let causeind: Vec<u8> = cause.iter().map(|&c| u8::from(c == uc)).collect();
        let cause_label = fmt_code(uc);

        for (jj, grp_name) in group_names.iter().enumerate() {
            let idx: Vec<usize> = (0..no).filter(|&i| gcode[i] == jj).collect();
            let y: Vec<f64> = idx.iter().map(|&i| time[i]).collect();
            let ic: Vec<u8> = idx.iter().map(|&i| censind[i]).collect();
            let icc: Vec<u8> = idx.iter().map(|&i| causeind[i]).collect();
            let z = cinc::cinc(&y, &ic, &icc);
            curves.push(CumincCurve {
                group: grp_name.clone(),
                cause: cause_label.clone(),
                group_index: jj,
                cause_value: uc,
                time: z.x,
                est: z.f,
                var: z.v,
            });
        }

        if ng > 1 {
            // 0 censored, 1 cause of interest, 2 any other failure
            let m: Vec<u8> = (0..no).map(|i| 2 * censind[i] - causeind[i]).collect();
            let ig: Vec<usize> = gcode.iter().map(|&g| g + 1).collect();
            let ist: Vec<usize> = scode.iter().map(|&s| s + 1).collect();
            let out = crstm(&time, &m, &ig, &ist, nst, ng, opts.rho);
            let stat = quadratic_form(&out.s, &out.vs);
            let df = ng - 1;
            tests.push(GrayTest {
                cause: cause_label.clone(),
                cause_value: uc,
                stat,
                p_value: chisq_upper_tail(stat, df as f64),
                df,
            });
        }
    }

    Ok(CumincResult {
        curves,
        tests,
        n: no,
        n_missing,
        n_groups: ng,
    })
}

/// `s' V⁻¹ s`, or `-1` when `V` is rank deficient.
///
/// R does this with `qr()` and compares `a$rank` to `ncol(a$qr)`, defaulting the
/// statistic to `-1` on rank deficiency. The rank test here is the SVD
/// criterion `σ_min > σ_max · 1e-7`, matching `qr`'s default relative
/// tolerance.
fn quadratic_form(s: &[f64], vs: &[Vec<f64>]) -> f64 {
    use faer::prelude::*;
    let p = s.len();
    if p == 0 {
        return -1.0;
    }
    if vs.iter().flatten().any(|v| !v.is_finite()) {
        return -1.0;
    }
    let m = crate::linalg::to_mat(vs);
    match m.as_ref().singular_values() {
        Ok(sv) if !sv.is_empty() => {
            let smax = sv[0];
            let smin = *sv.last().unwrap();
            if smax <= 0.0 || smin <= smax * 1e-7 {
                return -1.0;
            }
        }
        _ => return -1.0,
    }
    match crate::linalg::solve(vs, s, "Gray test") {
        Ok(x) => s.iter().zip(x.iter()).map(|(a, b)| a * b).sum(),
        Err(_) => -1.0,
    }
}

/// `timepoints()` — read estimates and variances off the curves at the
/// requested times.
///
/// Returns `(est, var)`, each `curves.len() × times.len()`, with `NaN` where
/// the requested time lies beyond the end of a curve (R's `NA`).
pub fn timepoints(result: &CumincResult, times: &[f64]) -> (Vec<Vec<f64>>, Vec<Vec<f64>>) {
    let mut t: Vec<f64> = times.to_vec();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    t.dedup();

    let mut est = Vec::with_capacity(result.curves.len());
    let mut var = Vec::with_capacity(result.curves.len());
    for c in &result.curves {
        let ind = tpoi(&c.time, &t);
        est.push(
            ind.iter()
                .map(|&k| if k == 0 { f64::NAN } else { c.est[k - 1] })
                .collect(),
        );
        var.push(
            ind.iter()
                .map(|&k| if k == 0 { f64::NAN } else { c.var[k - 1] })
                .collect(),
        );
    }
    (est, var)
}
