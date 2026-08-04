//! Multi-bias E-value computation.
//!
//! Port of `R/multi_evalue.R`. Computes the E-value considering multiple
//! biases simultaneously (confounding + selection + misclassification).

use crate::bias_spec::MultiBias;
use crate::error::{EvalueError, Result};
use crate::math_utils::{brent_root, deg_func};
use crate::measure::Estimate;

/// Result of a multi-bias E-value computation.
#[derive(Clone, Debug)]
pub struct MultiEvalueResult {
    /// Values on the RR scale: (point, lower, upper).
    pub rr_values: [Option<f64>; 3],
    /// Multi-bias E-values: (point, lower, upper).
    pub evalues: [Option<f64>; 3],
    /// Measure label.
    pub measure: &'static str,
    /// Parameter names the E-value refers to.
    pub parameters: Vec<String>,
}

impl MultiEvalueResult {
    pub fn point_evalue(&self) -> Option<f64> {
        self.evalues[0]
    }
}

/// Compute multi-bias E-value for RR.
///
/// Port of `multi_evalues.RR()` in `multi_evalue.R:117`.
pub fn multi_evalues_rr(
    biases: &MultiBias,
    est: f64,
    lo: Option<f64>,
    hi: Option<f64>,
    true_val: f64,
) -> Result<MultiEvalueResult> {
    if est < 0.0 {
        return Err(EvalueError::Invalid("Estimate cannot be negative".into()));
    }
    if true_val < 0.0 {
        return Err(EvalueError::Invalid("True value is impossible".into()));
    }
    if let (Some(lo_v), Some(hi_v)) = (lo, hi) {
        if lo_v > hi_v {
            return Err(EvalueError::Invalid(
                "Lower confidence limit should be less than upper confidence limit".into(),
            ));
        }
    }

    let values = [Some(est), lo, hi];

    // Compute E-values
    let mut e: [Option<f64>; 3] = values.map(|v| v.and_then(|x| multi_threshold(biases, x, true_val)));

    // Check if CI crosses null
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

    // Only report E-value for CI limit closer to null
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

    let params: Vec<String> = biases
        .parameters
        .iter()
        .map(|p| p.output.clone())
        .collect();

    Ok(MultiEvalueResult {
        rr_values: values,
        evalues: e,
        measure: "RR",
        parameters: params,
    })
}

/// Compute multi-bias E-value for OR.
pub fn multi_evalues_or(
    biases: &MultiBias,
    est: f64,
    lo: Option<f64>,
    hi: Option<f64>,
    rare: bool,
    true_val: f64,
) -> Result<MultiEvalueResult> {
    if est < 0.0 {
        return Err(EvalueError::Invalid("OR cannot be negative".into()));
    }
    let est_rr = Estimate::or(est, rare).to_rr()?.est;
    let lo_rr = lo.map(|l| Estimate::or(l, rare).to_rr().map(|r| r.est)).transpose()?;
    let hi_rr = hi.map(|h| Estimate::or(h, rare).to_rr().map(|r| r.est)).transpose()?;
    let true_rr = Estimate::or(true_val, rare).to_rr()?.est;

    let mut res = multi_evalues_rr(biases, est_rr, lo_rr, hi_rr, true_rr)?;
    res.measure = "OR";
    Ok(res)
}

/// Compute multi-bias E-value for HR.
pub fn multi_evalues_hr(
    biases: &MultiBias,
    est: f64,
    lo: Option<f64>,
    hi: Option<f64>,
    rare: bool,
    true_val: f64,
) -> Result<MultiEvalueResult> {
    if est < 0.0 {
        return Err(EvalueError::Invalid("HR cannot be negative".into()));
    }
    let est_rr = Estimate::hr(est, rare).to_rr()?.est;
    let lo_rr = lo.map(|l| Estimate::hr(l, rare).to_rr().map(|r| r.est)).transpose()?;
    let hi_rr = hi.map(|h| Estimate::hr(h, rare).to_rr().map(|r| r.est)).transpose()?;
    let true_rr = Estimate::hr(true_val, rare).to_rr()?.est;

    let mut res = multi_evalues_rr(biases, est_rr, lo_rr, hi_rr, true_rr)?;
    res.measure = "HR";
    Ok(res)
}

/// Solve for the multi-bias E-value threshold.
///
/// Port of `multi_threshold()` in `multi_evalue.R:191`.
/// Finds x where `deg_func(x, rat, n, d) = 0`.
fn multi_threshold(biases: &MultiBias, x: f64, true_val: f64) -> Option<f64> {
    if x.is_nan() {
        return None;
    }

    let rat = (true_val / x).max(x / true_val);

    // Try to find root in [1+eps, rat]
    let result = brent_root(
        |t| deg_func(t, rat, biases.n, biases.d),
        1.0 + 1e-9,
        rat,
        1e-10,
        200,
    );

    match result {
        Ok(root) => Some(root),
        Err(_) => {
            // Extend interval upward (like R's extendInt="upX")
            let mut upper = rat;
            for _ in 0..50 {
                upper *= 2.0;
                if let Ok(root) = brent_root(
                    |t| deg_func(t, rat, biases.n, biases.d),
                    1.0 + 1e-9,
                    upper,
                    1e-10,
                    200,
                ) {
                    return Some(root);
                }
                if upper > 1e10 {
                    break;
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bias_spec::*;

    #[test]
    fn test_multi_evalue_confounding_only_rr() {
        // multi_evalue with confounding only should match evalues.RR
        let mb = multi_bias(&[confounding()]).unwrap();
        let res = multi_evalues_rr(&mb, 3.5, None, None, 1.0).unwrap();
        let expected = crate::math_utils::threshold(3.5, 1.0).unwrap();
        assert!((res.point_evalue().unwrap() - expected).abs() < 1e-5);
    }

    #[test]
    fn test_multi_evalue_selection_general() {
        let mb = multi_bias(&[selection(&["general"]).unwrap()]).unwrap();
        let res = multi_evalues_rr(&mb, 0.5, None, None, 1.0).unwrap();
        // Should match selection_evalue for general selection
        let e = res.point_evalue().unwrap();
        assert!(e >= 1.0, "E-value should be >= 1: {e}");
    }

    #[test]
    fn test_multi_evalue_selection_selected() {
        let mb = multi_bias(&[selection(&["selected"]).unwrap()]).unwrap();
        let res = multi_evalues_rr(&mb, 4.7, None, None, 1.0).unwrap();
        let e = res.point_evalue().unwrap();
        assert!(e >= 1.0);
    }

    #[test]
    fn test_multi_evalue_hr_rare() {
        let mb = multi_bias(&[confounding()]).unwrap();
        let res = multi_evalues_hr(&mb, 0.6, None, None, true, 1.0).unwrap();
        let expected_rr = Estimate::hr(0.6, true).to_rr().unwrap().est;
        let expected = crate::math_utils::threshold(expected_rr, 1.0).unwrap();
        assert!((res.point_evalue().unwrap() - expected).abs() < 1e-5);
    }
}
