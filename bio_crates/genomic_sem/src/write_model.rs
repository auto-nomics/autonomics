//! Lavaan model syntax generation — port of `R/write.model.R`.

/// Write a lavaan model string from a factor loading matrix.
///
/// Generates model syntax for an exploratory factor analysis model:
/// ```text
/// F1 =~ V1 + V2 + V3
/// F2 =~ V2 + V4
/// F1 ~~ F2
/// V1 ~~ V1
/// V2 ~~ V2
/// ...
/// ```
pub fn write_model(
    loadings: &[Vec<f64>],  // n_vars × n_factors
    var_names: &[String],
    cutoff: f64,
    fix_resid: bool,
    bifactor: bool,
    must_load: bool,
    common: bool,
) -> String {
    let n_vars = loadings.len();
    let n_factors = loadings[0].len();
    let mut model = String::new();

    if common {
        // Single common factor
        let names: Vec<&str> = var_names.iter().map(|s| s.as_str()).collect();
        model.push_str(&format!("F1 =~ {}\n", names.join(" + ")));
    } else {
        let mut adjusted_loadings = loadings.to_vec();

        if must_load {
            // Ensure each variable loads on at least one factor
            for i in 0..n_vars {
                let max_val = adjusted_loadings[i].iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                for f in 0..n_factors {
                    if (adjusted_loadings[i][f] - max_val).abs() < 1e-10 {
                        adjusted_loadings[i][f] = cutoff + 0.01;
                    }
                }
            }
        }

        for f in 0..n_factors {
            let loading_vars: Vec<&str> = (0..n_vars)
                .filter(|&i| adjusted_loadings[i][f].abs() > cutoff)
                .map(|i| var_names[i].as_str())
                .collect();
            if !loading_vars.is_empty() {
                model.push_str(&format!("F{} =~ {}\n", f + 1, loading_vars.join(" + ")));
            }
        }

        if bifactor {
            // Add common bifactor
            let all_vars: Vec<&str> = var_names.iter().map(|s| s.as_str()).collect();
            model.push_str(&format!("Common_F =~ {}\n", all_vars.join(" + ")));
            for f in 0..n_factors {
                model.push_str(&format!("Common_F ~~ 0*F{}\n", f + 1));
            }
            for i in 0..n_factors {
                for j in (i + 1)..n_factors {
                    model.push_str(&format!("F{} ~~ 0*F{}\n", i + 1, j + 1));
                }
            }
        }
    }

    if fix_resid {
        for name in var_names {
            if model.contains(name) {
                model.push_str(&format!("{name} ~~ {name}\n"));
            }
        }
    }

    model
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_model_common() {
        let loadings = vec![vec![0.8], vec![0.6], vec![0.7]];
        let names = vec!["V1".into(), "V2".into(), "V3".into()];
        let model = write_model(&loadings, &names, 0.0, true, false, false, true);
        assert!(model.contains("F1 =~ V1 + V2 + V3"));
        assert!(model.contains("V1 ~~ V1"));
    }

    #[test]
    fn test_write_model_two_factor() {
        let loadings = vec![vec![0.8, 0.1], vec![0.7, 0.2], vec![0.1, 0.8], vec![0.2, 0.7]];
        let names = vec!["V1".into(), "V2".into(), "V3".into(), "V4".into()];
        let model = write_model(&loadings, &names, 0.3, true, false, false, false);
        assert!(model.contains("F1 =~ V1 + V2"));
        assert!(model.contains("F2 =~ V3 + V4"));
    }
}
