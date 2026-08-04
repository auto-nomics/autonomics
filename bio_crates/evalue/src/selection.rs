//! Selection bias E-values.
//!
//! Port of `R/EValue_selection.R`.

use crate::error::{EvalueError, Result};
use crate::math_utils::threshold;
use crate::measure::Estimate;

/// Result of a selection bias E-value computation.
#[derive(Clone, Debug)]
pub struct SelectionEvalueResult {
    pub rr_values: [Option<f64>; 3],
    pub evalues: [Option<f64>; 3],
    pub measure: &'static str,
    pub message: String,
}

impl SelectionEvalueResult {
    pub fn point_evalue(&self) -> Option<f64> {
        self.evalues[0]
    }
}

/// Compute selection bias E-value for RR.
///
/// Port of `svalues.RR()` in `EValue_selection.R:186`.
pub fn svalues_rr(
    est: f64,
    lo: Option<f64>,
    hi: Option<f64>,
    true_val: f64,
    sel_pop: bool,
    s_eq_u: bool,
    risk_inc: bool,
    risk_dec: bool,
) -> Result<SelectionEvalueResult> {
    if est < 0.0 {
        return Err(EvalueError::Invalid("RR cannot be negative".into()));
    }
    if true_val < 0.0 {
        return Err(EvalueError::Invalid("True value is impossible".into()));
    }
    if risk_inc && risk_dec {
        return Err(EvalueError::Invalid(
            "You have made incompatible assumptions about the association between selection and risk.".into(),
        ));
    }

    let values = [Some(est), lo, hi];

    // Compute selection E-values
    let thresh_results: Vec<(String, Option<f64>)> = values
        .iter()
        .map(|v| {
            v.and_then(|x| threshold_selection(x, true_val, sel_pop, s_eq_u, risk_inc, risk_dec))
                .map(|(m, e)| (m, Some(e)))
                .unwrap_or((String::new(), None))
        })
        .collect();

    let message = thresh_results[0].0.clone();
    let mut e: [Option<f64>; 3] = [
        thresh_results[0].1,
        thresh_results[1].1,
        thresh_results[2].1,
    ];

    // CI crossing null
    let null_ci = if est > true_val {
        lo.map(|l| l < true_val)
    } else if est < true_val {
        hi.map(|h| h > true_val)
    } else {
        None
    };

    if let Some(true_v) = null_ci {
        if true_v {
            e[1] = Some(1.0);
            e[2] = Some(1.0);
        }
    }

    if lo.is_some() || hi.is_some() {
        if est > true_val {
            e[2] = None;
        } else if est < true_val {
            e[1] = None;
        } else {
            e[1] = Some(1.0);
            e[2] = None;
        }
    }

    Ok(SelectionEvalueResult {
        rr_values: values,
        evalues: e,
        measure: "RR",
        message,
    })
}

/// Compute selection bias E-value for OR.
pub fn svalues_or(
    est: f64,
    lo: Option<f64>,
    hi: Option<f64>,
    rare: bool,
    true_val: f64,
    sel_pop: bool,
    s_eq_u: bool,
    risk_inc: bool,
    risk_dec: bool,
) -> Result<SelectionEvalueResult> {
    if est < 0.0 {
        return Err(EvalueError::Invalid("OR cannot be negative".into()));
    }
    let est_rr = Estimate::or(est, rare).to_rr()?.est;
    let lo_rr = lo.map(|l| Estimate::or(l, rare).to_rr().map(|r| r.est)).transpose()?;
    let hi_rr = hi.map(|h| Estimate::or(h, rare).to_rr().map(|r| r.est)).transpose()?;
    let true_rr = Estimate::or(true_val, rare).to_rr()?.est;

    let mut res = svalues_rr(est_rr, lo_rr, hi_rr, true_rr, sel_pop, s_eq_u, risk_inc, risk_dec)?;
    res.measure = "OR";
    Ok(res)
}

/// Compute selection bias E-value for HR.
pub fn svalues_hr(
    est: f64,
    lo: Option<f64>,
    hi: Option<f64>,
    rare: bool,
    true_val: f64,
    sel_pop: bool,
    s_eq_u: bool,
    risk_inc: bool,
    risk_dec: bool,
) -> Result<SelectionEvalueResult> {
    if est < 0.0 {
        return Err(EvalueError::Invalid("HR cannot be negative".into()));
    }
    let est_rr = Estimate::hr(est, rare).to_rr()?.est;
    let lo_rr = lo.map(|l| Estimate::hr(l, rare).to_rr().map(|r| r.est)).transpose()?;
    let hi_rr = hi.map(|h| Estimate::hr(h, rare).to_rr().map(|r| r.est)).transpose()?;
    let true_rr = Estimate::hr(true_val, rare).to_rr()?.est;

    let mut res = svalues_rr(est_rr, lo_rr, hi_rr, true_rr, sel_pop, s_eq_u, risk_inc, risk_dec)?;
    res.measure = "HR";
    Ok(res)
}

/// Compute selection bias E-value for a single RR value.
///
/// Port of `threshold_selection()` in `EValue_selection.R:283`.
///
/// Returns (message, evalue).
fn threshold_selection(
    x: f64,
    true_val: f64,
    sel_pop: bool,
    s_eq_u: bool,
    risk_inc: bool,
    risk_dec: bool,
) -> Option<(String, f64)> {
    if x.is_nan() {
        return None;
    }

    let (x, true_val) = if x <= 1.0 {
        (1.0 / x, 1.0 / true_val)
    } else {
        (x, true_val)
    };

    let rat = if true_val <= x {
        x / true_val
    } else {
        true_val / x
    };

    if sel_pop {
        let m = "This selection bias E-value refers to RR_UY|S=1 and RR_AU|S=1 (see documentation)";
        return Some((m.into(), rat + (rat * (rat - 1.0)).sqrt()));
    }

    if !s_eq_u && !risk_inc && !risk_dec {
        let m = "This selection bias E-value refers to RR_UY|A=0, RR_UY|A=1, RR_SU|A=0, and RR_SU|A=1 (see documentation)";
        let sqrt_rat = rat.sqrt();
        return Some((m.into(), sqrt_rat + (sqrt_rat * (sqrt_rat - 1.0)).sqrt()));
    }

    if s_eq_u && !risk_inc && !risk_dec {
        let m = "This selection bias E-value refers to RR_UY|A=0 and RR_UY|A=1 (see documentation)";
        return Some((m.into(), rat.sqrt()));
    }

    if s_eq_u && risk_inc {
        let m = "This selection bias E-value refers to RR_UY|A = 1 (see documentation)";
        return Some((m.into(), rat));
    }

    if s_eq_u && risk_dec {
        let m = "This selection bias E-value refers to RR_UY|A = 0 (see documentation)";
        return Some((m.into(), rat));
    }

    if risk_inc {
        let m = "This selection bias E-value refers to RR_UY|A=1 and RR_SU|A=1 (see documentation)";
        return Some((m.into(), rat + (rat * (rat - 1.0)).sqrt()));
    }

    if risk_dec {
        let m = "This selection bias E-value refers to RR_UY|A=0 and RR_SU|A=0 (see documentation)";
        return Some((m.into(), rat + (rat * (rat - 1.0)).sqrt()));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_svalues_rr_zika() {
        // Zika virus: RR = 73.1, lo = 13.0
        let res = svalues_rr(73.1, Some(13.0), None, 1.0, false, false, false, false).unwrap();
        let e = res.point_evalue().unwrap();
        assert!(e > 1.0);
    }

    #[test]
    fn test_svalues_rr_sel_pop() {
        // Obesity paradox: RR = 1.50, lo = 1.22, sel_pop = TRUE
        let res = svalues_rr(1.50, Some(1.22), None, 1.0, true, false, false, false).unwrap();
        let e = res.point_evalue().unwrap();
        assert!(e > 1.0);
    }

    #[test]
    fn test_svalues_rr_endometrial() {
        // Endometrial cancer: RR = 2.30, true = 11.98, S_eq_U = TRUE, risk_inc = TRUE
        let res = svalues_rr(2.30, None, None, 11.98, false, true, true, false).unwrap();
        let e = res.point_evalue().unwrap();
        // rat = 11.98/2.30 ≈ 5.21, e = rat = 5.21 (S=U and risk_inc → just rat)
        assert!((e - 11.98 / 2.30).abs() < 1e-6, "got {e}");
    }
}
