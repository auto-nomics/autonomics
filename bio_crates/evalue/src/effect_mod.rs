//! E-values for additive effect modification (interaction contrasts).
//!
//! Port of `R/effect_modification.R`.

use crate::error::{EvalueError, Result};
use crate::math_utils::{g, minimize_golden, threshold};
use statrs::distribution::{ContinuousCDF, Normal};

/// Bias direction for a stratum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BiasDir {
    Positive,
    Negative,
}

/// Unidirectional bias direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnidirBiasDirection {
    Positive,
    Negative,
    Unknown,
}

/// Input data for interaction-contrast E-values.
#[derive(Clone, Debug)]
pub struct IcData {
    // Stratum Z=1
    pub p1_1: f64,
    pub p1_0: f64,
    pub n1_1: f64,
    pub n1_0: f64,
    pub f1: f64,
    // Stratum Z=0
    pub p0_1: f64,
    pub p0_0: f64,
    pub n0_1: f64,
    pub n0_0: f64,
    pub f0: f64,
    // Significance level
    pub alpha: f64,
}

/// Result of `evalues.IC()`.
#[derive(Clone, Debug)]
pub struct IcResult {
    pub evalues: IcEvalueRow,
    pub rdt: Vec<RdtRow>,
    pub candidates: Option<Vec<CandidateRow>>,
}

#[derive(Clone, Debug, Default)]
pub struct IcEvalueRow {
    pub evalue: f64,
    pub bias_factor: f64,
    pub bound: f64,
    pub bias_dir: String,
}

#[derive(Clone, Debug)]
pub struct RdtRow {
    pub stratum: String,
    pub rd: f64,
    pub se: f64,
    pub lo: f64,
    pub hi: f64,
    pub pval: f64,
}

#[derive(Clone, Debug)]
pub struct CandidateRow {
    pub bias_dir: String,
    pub evalue: f64,
    pub bias_factor: f64,
    pub is_min: bool,
}

/// Compute E-value for an additive interaction contrast.
///
/// Port of `evalues.IC()` in `effect_modification.R:485`.
///
/// - `stat_type`: "est" for point estimate, "CI" for lower CI limit
/// - `true_val`: true IC to shift to (usually 0)
/// - `unidir_bias`: assume same direction in both strata?
/// - `unidir_dir`: direction (if unidir)
pub fn evalues_ic(
    data: &IcData,
    stat_type: &str,
    true_val: f64,
    unidir_bias: bool,
    unidir_dir: Option<UnidirBiasDirection>,
) -> Result<IcResult> {
    let var_name = match stat_type {
        "est" => "RD",
        "CI" => "lo",
        _ => return Err(EvalueError::Invalid("Argument 'stat' is invalid".into())),
    };

    // Case 0: Potentially multidirectional bias
    if !unidir_bias {
        let res = ic_evalue_inner(data, "effectMod", var_name, true_val, false, None)?;
        let mut row = res.evalues;
        row.bias_dir = "potentially multidirectional".into();
        return Ok(IcResult {
            evalues: row,
            rdt: res.rdt,
            candidates: None,
        });
    }

    let dir = unidir_dir.ok_or_else(|| {
        EvalueError::Invalid("If unidirBias is TRUE, must provide unidirBiasDirection".into())
    })?;

    // Cases 1-2: Known direction
    if dir != UnidirBiasDirection::Unknown {
        let res = ic_evalue_inner(data, "effectMod", var_name, true_val, true, Some(dir))?;
        let mut row = res.evalues;
        row.bias_dir = match dir {
            UnidirBiasDirection::Positive => "positive",
            UnidirBiasDirection::Negative => "negative",
            _ => "",
        }
        .into();
        return Ok(IcResult {
            evalues: row,
            rdt: res.rdt,
            candidates: None,
        });
    }

    // Case 3: Unknown direction — try both
    let cand1 = ic_evalue_inner(
        data,
        "effectMod",
        var_name,
        true_val,
        true,
        Some(UnidirBiasDirection::Positive),
    )?;
    let cand2 = ic_evalue_inner(
        data,
        "effectMod",
        var_name,
        true_val,
        true,
        Some(UnidirBiasDirection::Negative),
    )?;

    let cand1_e = cand1.evalues.evalue;
    let cand1_bf = cand1.evalues.bias_factor;
    let cand2_e = cand2.evalues.evalue;
    let cand2_bf = cand2.evalues.bias_factor;

    let (winner, winner_dir) = if cand1_e < cand2_e {
        (cand1, "positive")
    } else {
        (cand2, "negative")
    };

    let candidates = vec![
        CandidateRow {
            bias_dir: "positive".into(),
            evalue: cand1_e,
            bias_factor: cand1_bf,
            is_min: cand1_e <= cand2_e,
        },
        CandidateRow {
            bias_dir: "negative".into(),
            evalue: cand2_e,
            bias_factor: cand2_bf,
            is_min: cand2_e <= cand1_e,
        },
    ];

    let mut row = winner.evalues;
    row.bias_dir = winner_dir.into();

    Ok(IcResult {
        evalues: row,
        rdt: winner.rdt,
        candidates: Some(candidates),
    })
}

/// Inner helper for computing IC E-value.
///
/// Port of `IC_evalue_inner()` in `effect_modification.R:149`.
fn ic_evalue_inner(
    data: &IcData,
    stratum: &str,
    var_name: &str,
    true_val: f64,
    unidir_bias: bool,
    unidir_dir: Option<UnidirBiasDirection>,
) -> Result<IcResult> {
    // Check confounded IC > 0
    let rd_c = rdt_bound(data, 1.0, BiasDir::Positive, 1.0, BiasDir::Positive);
    let ic_c = rd_c[2].rd; // effectMod row

    if ic_c < 0.0 {
        return Err(EvalueError::Invalid(
            "The confounded interaction contrast is negative. Please recode the stratum variable."
                .into(),
        ));
    }

    // Check if already <= true
    if stratum == "effectMod" {
        let check_val = if var_name == "RD" { ic_c } else { rd_c[2].lo };
        if check_val <= true_val {
            return Ok(IcResult {
                evalues: IcEvalueRow {
                    evalue: 1.0,
                    bias_factor: 1.0,
                    bound: f64::NAN,
                    bias_dir: String::new(),
                },
                rdt: rd_c,
                candidates: None,
            });
        }
    }

    // Set up the distance function based on bias direction assumptions
    let dist_from_true: Box<dyn Fn(f64) -> f64> = if !unidir_bias {
        // Multidirectional: shift stratum 1 positive, stratum 0 negative
        Box::new(move |x| {
            let rdt = rdt_bound(data, x, BiasDir::Positive, x, BiasDir::Negative);
            let row = &rdt[stratum_idx(stratum)];
            let val = match var_name {
                "RD" => row.rd,
                "lo" => row.lo,
                _ => row.rd,
            };
            (val - true_val).abs()
        })
    } else {
        match unidir_dir.unwrap() {
            UnidirBiasDirection::Negative => Box::new(move |x| {
                let rdt = rdt_bound(data, 1.0, BiasDir::Negative, x, BiasDir::Negative);
                let row = &rdt[stratum_idx(stratum)];
                let val = match var_name {
                    "RD" => row.rd,
                    "lo" => row.lo,
                    _ => row.rd,
                };
                (val - true_val).abs()
            }),
            UnidirBiasDirection::Positive => Box::new(move |x| {
                let rdt = rdt_bound(data, x, BiasDir::Positive, 1.0, BiasDir::Positive);
                let row = &rdt[stratum_idx(stratum)];
                let val = match var_name {
                    "RD" => row.rd,
                    "lo" => row.lo,
                    _ => row.rd,
                };
                (val - true_val).abs()
            }),
            UnidirBiasDirection::Unknown => unreachable!(),
        }
    };

    // Iteratively increase search space upper bound
    let mut search_upper = 2.0_f64;
    let mut proximity = f64::INFINITY;
    let mut opt_x = 1.0_f64;
    let mut opt_val = f64::INFINITY;

    while proximity > 0.001 {
        search_upper *= 1.5;
        let (x, val) = minimize_golden(&dist_from_true, 1.0, search_upper, 1e-8, 200);
        proximity = val.abs();
        opt_x = x;
        opt_val = val;

        if search_upper >= 200.0 && proximity > 0.001 {
            return Err(EvalueError::Compute(
                "Tried bias factors up to 200, but could not move estimate close enough.".into(),
            ));
        }
    }

    // Get RDt at the optimal bias factor
    let final_rdt = if !unidir_bias {
        rdt_bound(data, opt_x, BiasDir::Positive, opt_x, BiasDir::Negative)
    } else {
        match unidir_dir.unwrap() {
            UnidirBiasDirection::Negative => {
                rdt_bound(data, 1.0, BiasDir::Negative, opt_x, BiasDir::Negative)
            }
            UnidirBiasDirection::Positive => {
                rdt_bound(data, opt_x, BiasDir::Positive, 1.0, BiasDir::Positive)
            }
            UnidirBiasDirection::Unknown => unreachable!(),
        }
    };

    Ok(IcResult {
        evalues: IcEvalueRow {
            evalue: g(opt_x),
            bias_factor: opt_x,
            bound: opt_val,
            bias_dir: String::new(),
        },
        rdt: final_rdt,
        candidates: None,
    })
}

/// Get stratum index: "1" → 0, "0" → 1, "effectMod" → 2.
fn stratum_idx(s: &str) -> usize {
    match s {
        "1" => 0,
        "0" => 1,
        "effectMod" => 2,
        _ => 2,
    }
}

/// Get field index: "RD" → 1, "lo" → 3.
fn field_idx(s: &str) -> usize {
    match s {
        "RD" => 1,
        "lo" => 3,
        _ => 1,
    }
}

/// Compute bias-corrected risk differences.
///
/// Port of `RDt_bound()` in `effect_modification.R:44`.
///
/// Returns [stratum1, stratum0, effectMod] rows.
fn rdt_bound(
    data: &IcData,
    max_b1: f64,
    bias_dir1: BiasDir,
    max_b0: f64,
    bias_dir0: BiasDir,
) -> Vec<RdtRow> {
    // Stratum 1 corrected RD
    let rdt_1 = match bias_dir1 {
        BiasDir::Positive => {
            let v = (data.p1_1 - data.p1_0 * max_b1) * (data.f1 + (1.0 - data.f1) / max_b1);
            v.max(-1.0)
        }
        BiasDir::Negative => {
            let v = (data.p1_1 * max_b1 - data.p1_0) * (data.f1 + (1.0 - data.f1) / max_b1);
            v.min(1.0)
        }
    };

    // Stratum 0 corrected RD
    let rdt_0 = match bias_dir0 {
        BiasDir::Positive => {
            let v = (data.p0_1 - data.p0_0 * max_b0) * (data.f0 + (1.0 - data.f0) / max_b0);
            v.max(-1.0)
        }
        BiasDir::Negative => {
            let v = (data.p0_1 * max_b0 - data.p0_0) * (data.f0 + (1.0 - data.f0) / max_b0);
            v.min(1.0)
        }
    };

    // Interaction contrast
    let ic_t = rdt_1 - rdt_0;

    // Variances
    let var_rdt_1 = rdt_var(data.f1, data.p1_1, data.p1_0, data.n1_1, data.n1_0, max_b1);
    let var_rdt_0 = rdt_var(data.f0, data.p0_1, data.p0_0, data.n0_1, data.n0_0, max_b0);
    let var_ic = var_rdt_1 + var_rdt_0;

    let z = Normal::new(0.0, 1.0).unwrap();
    let crit = z.inverse_cdf(1.0 - data.alpha / 2.0);

    let make_row = |stratum: &str, rd: f64, var: f64| {
        let se = var.sqrt();
        RdtRow {
            stratum: stratum.into(),
            rd,
            se,
            lo: rd - crit * se,
            hi: rd + crit * se,
            pval: 2.0 * (1.0 - z.cdf((rd / se).abs())),
        }
    };

    vec![
        make_row("1", rdt_1, var_rdt_1),
        make_row("0", rdt_0, var_rdt_0),
        make_row("effectMod", ic_t, var_ic),
    ]
}

/// Variance of bias-corrected risk difference.
///
/// Port of `RDt_var()` in `effect_modification.R:12`.
fn rdt_var(f: f64, p1: f64, p0: f64, n1: f64, n0: f64, max_b: f64) -> f64 {
    // If RD < 0, reverse coding
    let (p1, p0, n1, n0) = if p1 - p0 < 0.0 {
        (p0, p1, n0, n1)
    } else {
        (p1, p0, n1, n0)
    };

    let f_var = p1 * (1.0 - p1) / n1 + p0 * (1.0 - p0) / n0;
    let p1_var = p1 * (1.0 - p1) / n1;
    let p0_var = p0 * (1.0 - p0) / n0;

    let term1 = p1_var + p0_var * max_b * max_b;
    let term2 = (f + (1.0 - f) / max_b).powi(2);
    let term3 = (p1 - p0 * max_b).powi(2) * (1.0 - 1.0 / max_b).powi(2) * f_var;

    term1 * term2 + term3
}

#[cfg(test)]
mod tests {
    use super::*;

    // Letenneur et al. (2000) example data
    fn letenneur_data() -> IcData {
        // Women
        let nw_1 = 2988.0;
        let nw_0 = 364.0;
        let dw_y1_x1 = 158.0;
        let dw_y1_x0 = 6.0;
        let pw_1 = dw_y1_x1 / nw_1;
        let pw_0 = dw_y1_x0 / nw_0;
        let fw = nw_1 / (nw_1 + nw_0);

        // Men
        let nm_1 = 1790.0;
        let nm_0 = 605.0;
        let dm_y1_x1 = 64.0;
        let dm_y1_x0 = 17.0;
        let pm_1 = dm_y1_x1 / nm_1;
        let pm_0 = dm_y1_x0 / nm_0;
        let fm = nm_1 / (nm_1 + nm_0);

        IcData {
            p1_1: pw_1,
            p1_0: pw_0,
            n1_1: nw_1,
            n1_0: nw_0,
            f1: fw,
            p0_1: pm_1,
            p0_0: pm_0,
            n0_1: nm_1,
            n0_0: nm_0,
            f0: fm,
            alpha: 0.05,
        }
    }

    #[test]
    fn test_rdt_var_symmetry() {
        let v1 = rdt_var(0.25, 0.3, 0.1, 50.0, 100.0, 2.0);
        // Reverse sign by swapping p1 and p0 (and n1/n0)
        let v2 = rdt_var(0.25, 0.1, 0.3, 100.0, 50.0, 2.0);
        assert!((v1 - v2).abs() < 1e-10, "v1={v1}, v2={v2}");
    }

    #[test]
    fn test_evalues_ic_multidir() {
        let data = letenneur_data();
        let res = evalues_ic(&data, "est", 0.0, false, None).unwrap();
        assert!(res.evalues.evalue > 1.0, "E-value should be > 1");
        assert!(res.evalues.bias_factor > 1.0);
    }

    #[test]
    fn test_evalues_ic_unidir_unknown() {
        let data = letenneur_data();
        let res = evalues_ic(&data, "est", 0.0, true, Some(UnidirBiasDirection::Unknown)).unwrap();
        assert!(res.evalues.evalue > 1.0);
        assert!(res.candidates.is_some());
    }

    #[test]
    fn test_rdt_bound_clamping() {
        // Extreme bias factor should clamp RD to [-1, 1]
        let data = IcData {
            p1_1: 0.9,
            p1_0: 0.1,
            n1_1: 100.0,
            n1_0: 100.0,
            f1: 0.2,
            p0_1: 0.1,
            p0_0: 0.4,
            n0_1: 100.0,
            n0_0: 100.0,
            f0: 0.3,
            alpha: 0.05,
        };
        let rows = rdt_bound(&data, 50.0, BiasDir::Negative, 50.0, BiasDir::Positive);
        // Stratum 1 with negative bias → should be clamped to 1.0
        assert!(
            (rows[0].rd - 1.0).abs() < 1e-10,
            "stratum 1 RD: {}",
            rows[0].rd
        );
        // Stratum 0 with positive bias → should be clamped to -1.0
        assert!(
            (rows[1].rd - (-1.0)).abs() < 1e-10,
            "stratum 0 RD: {}",
            rows[1].rd
        );
    }
}
