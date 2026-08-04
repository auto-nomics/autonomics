//! Multi-bias bound calculation.
//!
//! Port of `R/multi_bound.R`. Given a set of biases and values for each
//! parameter, compute the maximum bias factor.

use std::collections::HashMap;

use crate::bias_spec::{MultiBias, ParamRow};
use crate::error::{EvalueError, Result};
use crate::math_utils::{bf_func, threshold};

/// Compute the multi-bias bound given parameter values.
///
/// Port of `multi_bound()` in `multi_bound.R:55`.
///
/// `params` is a map from argument name (e.g. `"RRAUc"`) to value.
pub fn multi_bound(biases: &MultiBias, params: &HashMap<String, f64>) -> Result<f64> {
    let necc_params = &biases.parameters;

    // Check all necessary parameters are provided
    let missing: Vec<&str> = necc_params
        .iter()
        .filter(|p| !params.contains_key(&p.argument))
        .map(|p| p.argument.as_str())
        .collect();

    if !missing.is_empty() {
        return Err(EvalueError::Invalid(format!(
            "Missing parameters: {}. Run summary for more information.",
            missing.join(" = , ")
        )));
    }

    // Check for extra params
    let extra: Vec<&str> = params
        .keys()
        .filter(|k| !necc_params.iter().any(|p| &p.argument == *k))
        .map(|s| s.as_str())
        .collect();
    if !extra.is_empty() {
        eprintln!(
            "Warning: unnecessary parameters provided: {}",
            extra.join(", ")
        );
    }

    // Get values for each parameter row
    let vals: Vec<f64> = necc_params
        .iter()
        .map(|p| {
            *params
                .get(&p.argument)
                .unwrap_or(&1.0)
        })
        .collect();

    // Check if any bias has SU
    let su = biases.biases.iter().any(|b| b.su);

    // Classify values by bias type
    let conf_vals: Vec<f64> = necc_params
        .iter()
        .zip(vals.iter())
        .filter(|(p, _)| p.bias.contains("confounding"))
        .map(|(_, &v)| v)
        .collect();

    let miscl_vals: Vec<f64> = necc_params
        .iter()
        .zip(vals.iter())
        .filter(|(p, _)| p.bias.contains("misclassification"))
        .map(|(_, &v)| v)
        .collect();

    // Selection values split by A=1 vs A=0 (latex contains "1" or "0")
    let sel1_vals: Vec<f64> = necc_params
        .iter()
        .zip(vals.iter())
        .filter(|(p, _)| p.bias == "selection" && p.latex.contains("= 1}$"))
        .map(|(_, &v)| v)
        .collect();

    let sel0_vals: Vec<f64> = necc_params
        .iter()
        .zip(vals.iter())
        .filter(|(p, _)| p.bias == "selection" && p.latex.contains("= 0}$"))
        .map(|(_, &v)| v)
        .collect();

    // Handle SU doubling (R: if SU && length == 1, double via threshold)
    let sel1_vals = if su && sel1_vals.len() == 1 {
        let t = threshold(sel1_vals[0], 1.0).unwrap_or(sel1_vals[0]);
        vec![t, t]
    } else if sel1_vals.len() == 1 {
        vec![sel1_vals[0], 1.0]
    } else {
        sel1_vals
    };

    let sel0_vals = if su && sel0_vals.len() == 1 {
        let t = threshold(sel0_vals[0], 1.0).unwrap_or(sel0_vals[0]);
        vec![t, t]
    } else if sel0_vals.len() == 1 {
        vec![sel0_vals[0], 1.0]
    } else {
        sel0_vals
    };

    let conf_prod = if conf_vals.len() > 1 {
        bf_func(conf_vals[0], conf_vals[1])
    } else {
        1.0
    };

    let miscl_prod = if !miscl_vals.is_empty() {
        miscl_vals.into_iter().product::<f64>()
    } else {
        1.0
    };

    let sel1_prod = if sel1_vals.len() > 1 {
        bf_func(sel1_vals[0], sel1_vals[1])
    } else {
        1.0
    };

    let sel0_prod = if sel0_vals.len() > 1 {
        bf_func(sel0_vals[0], sel0_vals[1])
    } else {
        1.0
    };

    Ok(conf_prod * miscl_prod * sel1_prod * sel0_prod)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bias_spec::*;
    use std::collections::HashMap;

    fn make_params(args: &[(&str, f64)]) -> HashMap<String, f64> {
        args.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn test_mb01_bound() {
        // confounding only: bf_func(RRAUc, RRUcY) = bf(2, 2) = 4/3
        let mb = multi_bias(&[confounding()]).unwrap();
        let p = make_params(&[("RRAUc", 2.0), ("RRUcY", 2.0)]);
        let b = multi_bound(&mb, &p).unwrap();
        assert!((b - 1.33333).abs() < 1e-4, "got {b}");
    }

    #[test]
    fn test_mb03_bound() {
        // selection("selected"): after "c" stripping, params are RRAUsS, RRUsYS
        // The R test provides all params; necc picks RRAUsS=5, RRUsYS=5
        // bf_func(5, 5) = 25/9 ≈ 2.77778
        let mb = multi_bias(&[selection(&["selected"]).unwrap()]).unwrap();
        // Check actual parameter names
        let args: Vec<&str> = mb.parameters.iter().map(|p| p.argument.as_str()).collect();
        let p = make_params(&[
            ("RRAUsS", 5.0),
            ("RRUsYS", 5.0),
            // Also provide the "c" versions (ignored as extras)
            ("RRAUscS", 6.0),
            ("RRUscYS", 6.0),
        ]);
        let b = multi_bound(&mb, &p).unwrap();
        assert!((b - 2.77778).abs() < 1e-4, "got {b}, args={args:?}");
    }

    #[test]
    fn test_mb07_bound() {
        // outcome misclassification: RRAYy = 7
        let mb = multi_bias(&[misclassification("outcome", false, false).unwrap()]).unwrap();
        let p = make_params(&[("RRAYy", 7.0)]);
        let b = multi_bound(&mb, &p).unwrap();
        assert!((b - 7.0).abs() < 1e-6, "got {b}");
    }
}
