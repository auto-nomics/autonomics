//! The `mr()` driver, the method catalog, and `default_parameters()` —
//! `R/mr.R`. This is the top-level orchestration layer: group harmonised data
//! by exposure/outcome pair, filter `mr_keep`, run each requested method, and
//! collect `(b, se, pval, nsnp)` rows.

use crate::harmonise::HarmoniseOutput;
use crate::methods;
use crate::{MrError, MrEstimate, Parameters, Result};

/// One entry in the method catalog (`mr_method_list()` in `R/mr.R:111`).
#[derive(Debug, Clone)]
pub struct MrMethod {
    /// Function object name (R `obj`), e.g. `"mr_ivw"`.
    pub obj: &'static str,
    /// Human-readable name, e.g. `"Inverse variance weighted"`.
    pub name: &'static str,
    pub use_by_default: bool,
    pub heterogeneity_test: bool,
}

/// `mr_method_list()` — `R/mr.R:111`. Methods not yet ported (RAPS, radial,
/// ML, sign, GRIP, bootstrap-Egger) are listed with their R names but flagged
/// via [`MrMethod::obj`] only for the implemented ones; unimplemented entries
/// are omitted from dispatch and documented in the crate root.
pub fn mr_method_list() -> Vec<MrMethod> {
    vec![
        MrMethod {
            obj: "mr_wald_ratio",
            name: "Wald ratio",
            use_by_default: true,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_two_sample_ml",
            name: "Maximum likelihood",
            use_by_default: false,
            heterogeneity_test: true,
        },
        MrMethod {
            obj: "mr_egger_regression",
            name: "MR Egger",
            use_by_default: true,
            heterogeneity_test: true,
        },
        MrMethod {
            obj: "mr_egger_regression_bootstrap",
            name: "MR Egger (bootstrap)",
            use_by_default: false,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_simple_median",
            name: "Simple median",
            use_by_default: false,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_weighted_median",
            name: "Weighted median",
            use_by_default: true,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_penalised_weighted_median",
            name: "Penalised weighted median",
            use_by_default: false,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_ivw",
            name: "Inverse variance weighted",
            use_by_default: true,
            heterogeneity_test: true,
        },
        MrMethod {
            obj: "mr_ivw_radial",
            name: "IVW radial",
            use_by_default: false,
            heterogeneity_test: true,
        },
        MrMethod {
            obj: "mr_ivw_mre",
            name: "Inverse variance weighted (multiplicative random effects)",
            use_by_default: false,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_ivw_fe",
            name: "Inverse variance weighted (fixed effects)",
            use_by_default: false,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_simple_mode",
            name: "Simple mode",
            use_by_default: true,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_weighted_mode",
            name: "Weighted mode",
            use_by_default: true,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_weighted_mode_nome",
            name: "Weighted mode (NOME)",
            use_by_default: false,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_simple_mode_nome",
            name: "Simple mode (NOME)",
            use_by_default: false,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_raps",
            name: "Robust adjusted profile score (RAPS)",
            use_by_default: false,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_sign",
            name: "Sign concordance test",
            use_by_default: false,
            heterogeneity_test: false,
        },
        MrMethod {
            obj: "mr_uwr",
            name: "Unweighted regression",
            use_by_default: false,
            heterogeneity_test: true,
        },
        MrMethod {
            obj: "mr_grip",
            name: "MR GRIP",
            use_by_default: false,
            heterogeneity_test: false,
        },
    ]
}

/// `default_parameters()` — `R/mr.R:305`. Convenience alias for
/// [`Parameters::default`].
pub fn default_parameters() -> Parameters {
    Parameters::default()
}

/// One row of `mr()` output.
#[derive(Debug, Clone)]
pub struct MrResultRow {
    pub id_exposure: String,
    pub id_outcome: String,
    pub method: String,
    pub nsnp: usize,
    pub b: f64,
    pub se: f64,
    pub pval: f64,
}

/// Run a single named method on one SNP set. Returns the method's estimate, or
/// `NotImplemented` for methods not ported in this build.
fn run_method(
    obj: &str,
    b_exp: &[f64],
    b_out: &[f64],
    se_exp: &[f64],
    se_out: &[f64],
    parameters: &Parameters,
) -> Result<MrEstimate> {
    Ok(match obj {
        "mr_wald_ratio" => methods::mr_wald_ratio(b_exp, b_out, se_exp, se_out, parameters),
        "mr_ivw" => methods::mr_ivw(b_exp, b_out, se_exp, se_out, parameters),
        "mr_ivw_fe" => methods::mr_ivw_fe(b_exp, b_out, se_exp, se_out, parameters),
        "mr_ivw_mre" => methods::mr_ivw_mre(b_exp, b_out, se_exp, se_out, parameters),
        "mr_uwr" => methods::mr_uwr(b_exp, b_out, se_exp, se_out, parameters),
        "mr_egger_regression" => {
            methods::mr_egger_regression(b_exp, b_out, se_exp, se_out, parameters)
        }
        "mr_simple_median" => methods::mr_simple_median(b_exp, b_out, se_exp, se_out, parameters),
        "mr_weighted_median" => {
            methods::mr_weighted_median(b_exp, b_out, se_exp, se_out, parameters)
        }
        "mr_penalised_weighted_median" => {
            methods::mr_penalised_weighted_median(b_exp, b_out, se_exp, se_out, parameters)
        }
        "mr_simple_mode" => methods::mr_simple_mode(b_exp, b_out, se_exp, se_out, parameters),
        "mr_weighted_mode" => methods::mr_weighted_mode(b_exp, b_out, se_exp, se_out, parameters),
        "mr_simple_mode_nome" => {
            methods::mr_simple_mode_nome(b_exp, b_out, se_exp, se_out, parameters)
        }
        "mr_weighted_mode_nome" => {
            methods::mr_weighted_mode_nome(b_exp, b_out, se_exp, se_out, parameters)
        }
        other => {
            return Err(MrError::NotImplemented(format!(
                "{other} (external-package method; not ported in this build)"
            )));
        }
    })
}

/// `mr(dat, parameters, method_list)` — `R/mr.R:13`.
///
/// `method_list` defaults to the `use_by_default` methods when empty. The
/// Wald-ratio special case mirrors R: it is dropped from a multi-method run on
/// multi-SNP data (it only applies to a single instrument).
pub fn mr(
    dat: &[HarmoniseOutput],
    parameters: &Parameters,
    method_list: &[&str],
) -> Result<Vec<MrResultRow>> {
    // Validate method names against the catalog.
    let catalog = mr_method_list();
    let known: std::collections::HashSet<&str> = catalog.iter().map(|m| m.obj).collect();
    for m in method_list {
        if !known.contains(m) {
            return Err(MrError::NotImplemented(format!("unknown method '{m}'")));
        }
    }
    let name_by_obj: std::collections::HashMap<&str, &str> =
        catalog.iter().map(|m| (m.obj, m.name)).collect();

    let methods_to_run: Vec<&str> = if method_list.is_empty() {
        catalog
            .iter()
            .filter(|m| m.use_by_default)
            .map(|m| m.obj)
            .collect()
    } else {
        method_list.to_vec()
    };

    // Group by (id_exposure, id_outcome), preserving first-seen order.
    let mut order: Vec<(String, String)> = Vec::new();
    let mut groups: std::collections::HashMap<(String, String), Vec<&HarmoniseOutput>> =
        std::collections::HashMap::new();
    for r in dat {
        let key = (r.id_exposure.clone(), r.id_outcome.clone());
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(r);
    }

    let mut out = Vec::new();
    for key in &order {
        let rows: Vec<&HarmoniseOutput> =
            groups[key].iter().copied().filter(|r| r.mr_keep).collect();
        if rows.is_empty() {
            continue;
        }
        let b_exp: Vec<f64> = rows.iter().map(|r| r.beta_exposure).collect();
        let b_out: Vec<f64> = rows.iter().map(|r| r.beta_outcome).collect();
        let se_exp: Vec<f64> = rows.iter().map(|r| r.se_exposure).collect();
        let se_out: Vec<f64> = rows.iter().map(|r| r.se_outcome).collect();

        // Wald-ratio only when it is the sole method or there is one SNP.
        let run: Vec<&str> = if b_exp.len() > 1 && methods_to_run.len() > 1 {
            methods_to_run
                .iter()
                .copied()
                .filter(|m| *m != "mr_wald_ratio")
                .collect()
        } else {
            methods_to_run.clone()
        };

        for obj in run {
            let est = run_method(obj, &b_exp, &b_out, &se_exp, &se_out, parameters)?;
            // R drops rows where b, se, pval are all NA.
            if est.b.is_nan() && est.se.is_nan() && est.pval.is_nan() {
                continue;
            }
            out.push(MrResultRow {
                id_exposure: key.0.clone(),
                id_outcome: key.1.clone(),
                method: (*name_by_obj.get(obj).unwrap_or(&obj)).to_string(),
                nsnp: est.nsnp,
                b: est.b,
                se: est.se,
                pval: est.pval,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_list_has_defaults() {
        let m = mr_method_list();
        let defaults: Vec<&str> = m
            .iter()
            .filter(|x| x.use_by_default)
            .map(|x| x.obj)
            .collect();
        assert!(defaults.contains(&"mr_wald_ratio"));
        assert!(defaults.contains(&"mr_ivw"));
        assert!(defaults.contains(&"mr_egger_regression"));
        assert!(defaults.contains(&"mr_weighted_median"));
        assert!(defaults.contains(&"mr_simple_mode"));
        assert!(defaults.contains(&"mr_weighted_mode"));
    }

    #[test]
    fn default_parameters_matches_r() {
        let p = default_parameters();
        assert_eq!(p.nboot, 1000);
        assert_eq!(p.penk, 20.0);
        assert_eq!(p.phi, 1.0);
        assert_eq!(p.alpha, 0.05);
        assert!(p.over_dispersion);
        assert!(!p.shrinkage);
    }
}
