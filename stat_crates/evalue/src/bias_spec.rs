//! Bias specification system for multi-bias sensitivity analysis.
//!
//! Port of `R/biases.R`. Provides typed bias declarations (`confounding()`,
//! `selection(...)`, `misclassification(...)`) and the `multi_bias()` combinator
//! that determines which parameters apply for a given set of biases.

use crate::error::{EvalueError, Result};

// ── Bias types ────────────────────────────────────────────────────────

/// A single declared bias.
#[derive(Clone, Debug)]
pub struct Bias {
    pub kind: BiasKind,
    /// Selection-specific attributes.
    pub selected: bool,
    pub increased_risk: bool,
    pub decreased_risk: bool,
    pub su: bool, // S = U assumption
    /// Misclassification-specific attributes.
    pub rare_outcome: bool,
    pub rare_exposure: bool,
    /// Polynomial degree: numerator (`n`) and denominator (`d`).
    pub n: i32,
    pub d: i32,
    /// Messages/warnings.
    pub messages: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BiasKind {
    Confounding,
    Selection,
    OutcomeMisclassification,
    ExposureMisclassification,
}

/// A combined set of biases for multi-bias analysis.
#[derive(Clone, Debug)]
pub struct MultiBias {
    pub biases: Vec<Bias>,
    /// Total polynomial degree: numerator and denominator.
    pub n: i32,
    pub d: i32,
    /// Parameters table (which RR arguments apply).
    pub parameters: Vec<ParamRow>,
    /// Combined messages.
    pub messages: Vec<String>,
}

/// A row in the parameter table (subset of `get_arg_tab()` columns).
#[derive(Clone, Debug, PartialEq)]
pub struct ParamRow {
    pub bias: String,
    pub output: String,
    pub argument: String,
    pub latex: String,
}

// ── Constructors ──────────────────────────────────────────────────────

/// Declare unmeasured confounding. Port of `confounding()`.
pub fn confounding() -> Bias {
    Bias {
        kind: BiasKind::Confounding,
        selected: false,
        increased_risk: false,
        decreased_risk: false,
        su: false,
        rare_outcome: false,
        rare_exposure: false,
        n: 2,
        d: 1,
        messages: vec![],
    }
}

/// Declare selection bias. Port of `selection(...)`.
///
/// Options:
/// - `"general"` (default): general selection bias
/// - `"selected"`: target is selected population only
/// - `"S = U"`: unmeasured factor defines selected population
/// - `"increased risk"` / `"decreased risk"`: direction assumption
pub fn selection(opts: &[&str]) -> Result<Bias> {
    let mut arguments: Vec<String> = opts.iter().map(|s| s.to_string()).collect();

    let mut mess = vec![];

    if arguments.is_empty() {
        arguments.push("general".into());
        mess.push("The default option, general selection bias, is being used.".into());
    }

    // Normalize "S = U" variants
    for a in arguments.iter_mut() {
        if a.to_lowercase().replace(' ', "") == "s=u" {
            *a = "S = U".into();
        }
    }

    let valid = [
        "general",
        "selected",
        "S = U",
        "increased risk",
        "decreased risk",
    ];
    let mut types: Vec<&str> = arguments
        .iter()
        .filter(|a| valid.contains(&a.as_str()))
        .map(|s| s.as_str())
        .collect();

    // Check for invalid arguments
    let invalid: Vec<&str> = arguments
        .iter()
        .filter(|a| !valid.contains(&a.as_str()))
        .map(|s| s.as_str())
        .collect();
    if !invalid.is_empty() {
        mess.push(format!(
            "\"{}\" is/are not valid options for selection bias and have been ignored.",
            invalid.join("\", \"")
        ));
    }

    // Check incompatible assumptions
    if types.contains(&"selected") && types.len() > 1 {
        return Err(EvalueError::Invalid(
            "These assumptions are incompatible; choose \"general\" instead of \"selected\"."
                .into(),
        ));
    }
    if types.contains(&"increased risk") && types.contains(&"decreased risk") {
        return Err(EvalueError::Invalid(
            "These assumptions are incompatible; choose either increased or decreased risk.".into(),
        ));
    }

    // If neither target population selected
    if !types.contains(&"general") && !types.contains(&"selected") {
        types.push("general");
        mess.push("The default option, general selection bias, is being used as well.".into());
    }

    // Deduplicate "general"
    let mut seen_general = false;
    types.retain(|&t| {
        if t == "general" {
            if seen_general {
                false
            } else {
                seen_general = true;
                true
            }
        } else {
            true
        }
    });

    // Compute n, d from the poss_args table
    let mut n = 0i32;
    let mut d = 0i32;

    for &t in &types {
        let (tn, td) = match t {
            "general" => (4, 2),
            "selected" => (2, 1),
            "S = U" => (-2, -2),
            "increased risk" => (-2, -1),
            "decreased risk" => (-2, -1),
            _ => unreachable!(),
        };
        n += tn;
        d += td;
    }

    // If both S=U and increased/decreased risk, overlap correction
    if types.contains(&"S = U")
        && (types.contains(&"increased risk") || types.contains(&"decreased risk"))
    {
        n += 1;
        d += 1;
    }

    let selected = types.contains(&"selected");
    let increased_risk = types.contains(&"increased risk");
    let decreased_risk = types.contains(&"decreased risk");
    let su = types.contains(&"S = U");

    Ok(Bias {
        kind: BiasKind::Selection,
        selected,
        increased_risk,
        decreased_risk,
        su,
        rare_outcome: false,
        rare_exposure: false,
        n,
        d,
        messages: mess,
    })
}

/// Declare misclassification bias. Port of `misclassification(...)`.
///
/// - `target`: `"outcome"` or `"exposure"`
/// - `rare_outcome`: for exposure misclass, must be true
/// - `rare_exposure`: for exposure misclass
pub fn misclassification(target: &str, rare_outcome: bool, rare_exposure: bool) -> Result<Bias> {
    let mut mess = vec![];

    let kind = match target {
        "outcome" => BiasKind::OutcomeMisclassification,
        "exposure" => BiasKind::ExposureMisclassification,
        _ => {
            return Err(EvalueError::Invalid(format!(
                "Either \"outcome\" or \"exposure\" must be chosen. Got '{target}'"
            )));
        }
    };

    if target == "outcome" && (rare_outcome || rare_exposure) {
        mess.push(
            "No rare outcome/exposure arguments are necessary for outcome misclassification; they have been ignored.".into(),
        );
    }

    if target == "exposure" && !rare_outcome {
        return Err(EvalueError::Invalid(
                "Exposure misclassification with non-rare outcomes not currently available; set rare_outcome = TRUE.".into(),
            ));
    }

    let (n, d) = match kind {
        BiasKind::OutcomeMisclassification => (1, 0),
        BiasKind::ExposureMisclassification => {
            if rare_exposure {
                (1, 0)
            } else {
                (2, 0)
            }
        }
        _ => unreachable!(),
    };

    Ok(Bias {
        kind,
        selected: false,
        increased_risk: false,
        decreased_risk: false,
        su: false,
        rare_outcome: target == "exposure",
        rare_exposure,
        n,
        d,
        messages: mess,
    })
}

// ── get_arg_tab ───────────────────────────────────────────────────────

/// The full parameter table from `get_arg_tab()` in `biases.R:315`.
/// Each tuple: (bias, order, selected, rare_outcome, rare_exposure,
///              increased_risk, decreased_risk, SU, latex, output, argument)
#[allow(clippy::type_complexity)]
fn arg_tab() -> Vec<ArgTabRow> {
    macro_rules! r {
        ($bias:expr, $order:expr, $sel:expr, $ro:expr, $re:expr, $ir:expr, $dr:expr, $su:expr,
         $latex:expr, $output:expr, $argument:expr) => {
            ArgTabRow {
                bias: $bias.into(),
                order: $order,
                selected: $sel,
                rare_outcome: $ro,
                rare_exposure: $re,
                increased_risk: $ir,
                decreased_risk: $dr,
                su: $su,
                latex: $latex.into(),
                output: $output.into(),
                argument: $argument.into(),
            }
        };
    }
    vec![
        r!(
            "confounding",
            1,
            false,
            false,
            false,
            false,
            false,
            false,
            r"$\text{RR}_{AU_c}$",
            "RR_AUc",
            "RRAUc"
        ),
        r!(
            "confounding",
            1,
            false,
            false,
            false,
            false,
            false,
            false,
            r"$\text{RR}_{U_cY}$",
            "RR_UcY",
            "RRUcY"
        ),
        r!(
            "selection",
            2,
            false,
            false,
            false,
            true,
            false,
            false,
            r"$\text{RR}_{U_sY \mid A = 1}$",
            "RR_UsY|A=1",
            "RRUsYA1"
        ),
        r!(
            "selection",
            23,
            false,
            false,
            false,
            true,
            false,
            true,
            r"$\text{RR}_{SU_s \mid A = 1}$",
            "RR_SUs|A=1",
            "RRSUsA1"
        ),
        r!(
            "selection",
            2,
            false,
            false,
            false,
            false,
            true,
            false,
            r"$\text{RR}_{U_sY \mid A = 0}$",
            "RR_UsY|A=0",
            "RRUsYA0"
        ),
        r!(
            "selection",
            23,
            false,
            false,
            false,
            false,
            true,
            true,
            r"$\text{RR}_{SU_s \mid A = 0}$",
            "RR_SUs|A=0",
            "RRSUsA0"
        ),
        r!(
            "selection",
            3,
            false,
            false,
            false,
            true,
            false,
            false,
            r"$\text{RR}_{U_sY^* \mid A = 1}$",
            "RR_UsY*|A=1",
            "RRUsYA1"
        ),
        r!(
            "selection",
            3,
            false,
            false,
            false,
            false,
            true,
            false,
            r"$\text{RR}_{U_sY^* \mid A = 0}$",
            "RR_UsY*|A=0",
            "RRUsYA0"
        ),
        r!(
            "selection",
            3,
            false,
            false,
            false,
            true,
            false,
            false,
            r"$\text{RR}_{U_sY \mid A^* = 1}$",
            "RR_UsY|A*=1",
            "RRUsYA1"
        ),
        r!(
            "selection",
            3,
            false,
            false,
            false,
            true,
            false,
            false,
            r"$\text{RR}_{SU_s \mid A^* = 1}$",
            "RR_SUs|A*=1",
            "RRSUsA1"
        ),
        r!(
            "selection",
            3,
            false,
            false,
            false,
            false,
            true,
            false,
            r"$\text{RR}_{U_sY \mid A^* = 0}$",
            "RR_UsY|A*=0",
            "RRUsYA0"
        ),
        r!(
            "selection",
            3,
            false,
            false,
            false,
            false,
            true,
            false,
            r"$\text{RR}_{SU_s \mid A^* = 0}$",
            "RR_SUs|A*=1",
            "RRSUsA1"
        ),
        r!(
            "confounding and selection",
            1,
            true,
            false,
            false,
            false,
            false,
            false,
            r"$\text{RR}_{AU_{sc}\mid S = 1}$",
            "RR_AUsc|S",
            "RRAUscS"
        ),
        r!(
            "confounding and selection",
            1,
            true,
            false,
            false,
            false,
            false,
            false,
            r"$\text{RR}_{U_{sc}Y\mid S = 1}$",
            "RR_UscY|S",
            "RRUscYS"
        ),
        r!(
            "outcome misclassification",
            2,
            false,
            false,
            false,
            false,
            false,
            false,
            r"$\text{RR}_{AY^* \mid y}$",
            "RR_AY*|y",
            "RRAYy"
        ),
        r!(
            "exposure misclassification",
            2,
            false,
            false,
            false,
            false,
            false,
            false,
            r"$\text{OR}_{YA^* \mid a}$",
            "OR_YA*|a",
            "ORYAa"
        ),
        r!(
            "exposure misclassification",
            2,
            false,
            true,
            true,
            false,
            false,
            false,
            r"$\text{RR}_{YA^* \mid a}$",
            "RR_YA*|a",
            "RRYAa"
        ),
        r!(
            "outcome misclassification",
            3,
            true,
            false,
            false,
            false,
            false,
            false,
            r"$\text{RR}_{AY^* \mid y, S = 1}$",
            "RR_AY*|y,S",
            "RRAYyS"
        ),
        r!(
            "exposure misclassification",
            3,
            true,
            true,
            false,
            false,
            false,
            false,
            r"$\text{OR}_{YA^* \mid a, S = 1}$",
            "OR_YA*|a,S",
            "ORYAaS"
        ),
        r!(
            "exposure misclassification",
            3,
            true,
            true,
            true,
            false,
            false,
            false,
            r"$\text{RR}_{YA^* \mid a, S = 1}$",
            "RR_YA*|a,S",
            "RRYAaS"
        ),
    ]
}

#[derive(Clone, Debug)]
struct ArgTabRow {
    bias: String,
    order: i32,
    selected: bool,
    rare_outcome: bool,
    rare_exposure: bool,
    increased_risk: bool,
    decreased_risk: bool,
    su: bool,
    latex: String,
    output: String,
    argument: String,
}

// ── multi_bias ────────────────────────────────────────────────────────

/// Combine multiple biases into a `MultiBias` object.
///
/// Port of `multi_bias()` in `biases.R:427`. This is the most complex
/// non-mathematical function — it determines which parameters apply
/// for a given set of biases by filtering the `arg_tab` table.
pub fn multi_bias(biases: &[Bias]) -> Result<MultiBias> {
    if biases.is_empty() {
        return Err(EvalueError::Invalid(
            "multi_bias requires at least one bias".into(),
        ));
    }

    let yes_confounding = biases.iter().any(|b| b.kind == BiasKind::Confounding);
    let yes_selection = biases.iter().any(|b| b.kind == BiasKind::Selection);
    let yes_misclass = biases.iter().any(|b| {
        matches!(
            b.kind,
            BiasKind::OutcomeMisclassification | BiasKind::ExposureMisclassification
        )
    });

    // Get the selection bias (if any)
    let sel_bias = biases.iter().find(|b| b.kind == BiasKind::Selection);
    let misclass_bias = biases.iter().find(|b| {
        matches!(
            b.kind,
            BiasKind::OutcomeMisclassification | BiasKind::ExposureMisclassification
        )
    });

    let yes_both =
        yes_selection && yes_confounding && sel_bias.map(|b| b.selected).unwrap_or(false);

    // Determine ordering info for selection + misclassification
    let mut first_b = String::new();
    let mut next_b = String::new();

    if yes_selection && yes_misclass {
        let sel_idx = biases.iter().position(|b| b.kind == BiasKind::Selection);
        let mis_idx = biases.iter().position(|b| {
            matches!(
                b.kind,
                BiasKind::OutcomeMisclassification | BiasKind::ExposureMisclassification
            )
        });
        if let (Some(si), Some(mi)) = (sel_idx, mis_idx) {
            if !sel_bias.map(|b| b.selected).unwrap_or(false) {
                if si < mi {
                    first_b = "selection".into();
                    next_b = if matches!(biases[mi].kind, BiasKind::OutcomeMisclassification) {
                        "outcome misclassification".into()
                    } else {
                        "exposure misclassification".into()
                    };
                } else {
                    first_b = if matches!(biases[mi].kind, BiasKind::OutcomeMisclassification) {
                        "outcome misclassification".into()
                    } else {
                        "exposure misclassification".into()
                    };
                    next_b = "selection".into();
                }
            }
        }
    }

    // Determine effective bias names
    let mut new_biases: Vec<String>;
    if yes_both {
        next_b = biases
            .iter()
            .filter(|b| !matches!(b.kind, BiasKind::Selection | BiasKind::Confounding))
            .map(bias_name)
            .collect::<Vec<_>>()
            .join("");
        new_biases = vec!["confounding and selection".into()];
        // extend with non-confounding, non-selection biases
        for b in biases.iter() {
            if !matches!(b.kind, BiasKind::Selection | BiasKind::Confounding) {
                new_biases.push(bias_name(b));
            }
        }
    } else {
        new_biases = biases.iter().map(bias_name).collect();
    }

    let mut arg_rows = arg_tab();

    // If selected population and not confounding: rename rows and strip "c"
    if yes_selection && !yes_confounding && sel_bias.map(|b| b.selected).unwrap_or(false) {
        for row in &mut arg_rows {
            if row.bias == "confounding and selection" {
                row.bias = "selection".into();
            }
            // Strip "c" from text fields (R: gsub pattern "c" replacement "")
            row.latex = row.latex.replace('c', "");
            row.output = row.output.replace('c', "");
            row.argument = row.argument.replace('c', "");
        }
    }

    // Filter to relevant rows
    let mut sub_tab: Vec<ArgTabRow> = arg_rows
        .iter()
        .filter(|r| new_biases.iter().any(|nb| nb.as_str() == r.bias))
        .cloned()
        .collect();

    // Remove order=3 rows for first_b, order=2 rows for next_b
    if !first_b.is_empty() && !next_b.is_empty() {
        sub_tab.retain(|r| {
            !((r.bias.as_str() == first_b.as_str() && r.order == 3)
                || (r.bias.as_str() == next_b.as_str() && r.order == 2))
        });
    }

    // Process selection rows
    let mut selection_rows: Vec<ArgTabRow> = vec![];
    if yes_selection || yes_both {
        let sel = if yes_both {
            // For "confounding and selection" combined
            sub_tab
                .iter()
                .filter(|r| r.bias == "confounding and selection")
                .cloned()
                .collect::<Vec<_>>()
        } else {
            let sb = sel_bias.unwrap();
            // Filter by matching attributes (R merge on key columns)
            let mut filtered: Vec<ArgTabRow> = sub_tab
                .iter()
                .filter(|r| r.bias == "selection")
                .filter(|r| r.selected == sb.selected)
                .filter(|r| {
                    // Match SU (only if sb.su is true)
                    if sb.su { r.su } else { true }
                })
                .filter(|r| {
                    // Match increased_risk / decreased_risk (only if set)
                    if sb.increased_risk {
                        r.increased_risk
                    } else if sb.decreased_risk {
                        r.decreased_risk
                    } else {
                        true
                    }
                })
                .cloned()
                .collect();

            // Remove rows with A* or Y* if no misclassification after selection
            let to_remove_pattern = if next_b == "selection" || yes_misclass {
                if first_b == "exposure misclassification" {
                    Some("A\\=")
                } else {
                    Some("A*\\=")
                }
            } else {
                Some("(A*)|(Y*)")
            };

            if to_remove_pattern.is_some() {
                // Remove rows whose output contains A*= or Y*= patterns
                // depending on whether we have misclassification
                if !yes_misclass && next_b != "selection" {
                    // Remove rows with A* or Y* in output
                    filtered.retain(|r| !r.output.contains("A*") && !r.output.contains("Y*"));
                } else if next_b == "selection" || yes_misclass {
                    // Contains both types of misclassification
                    // Keep only non-misclassified rows (no A* or Y*)
                    filtered.retain(|r| !r.output.contains("A*") && !r.output.contains("Y*"));
                }
            }

            filtered
        };
        selection_rows = sel;

        // If SU and only 1 sel row, double it (for bf_func)
        // This is handled in multi_bound, not here
    }

    // Process misclassification rows
    let mut misclass_rows: Vec<ArgTabRow> = vec![];
    if yes_misclass {
        let mb = misclass_bias.unwrap();
        let mut filtered: Vec<ArgTabRow> = sub_tab
            .iter()
            .filter(|r| {
                r.bias == "outcome misclassification" || r.bias == "exposure misclassification"
            })
            .filter(|r| {
                if matches!(mb.kind, BiasKind::ExposureMisclassification) {
                    r.rare_exposure == mb.rare_exposure && r.rare_outcome == mb.rare_outcome
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        // Handle S rows filtering
        let s_rows: Vec<usize> = filtered
            .iter()
            .enumerate()
            .filter(|(_, r)| r.output.contains('S'))
            .map(|(i, _)| i)
            .collect();

        if !s_rows.is_empty() {
            if next_b == "selection" || !(yes_selection || yes_both) {
                // Remove S rows
                let s_set: std::collections::HashSet<usize> = s_rows.into_iter().collect();
                filtered = filtered
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| !s_set.contains(i))
                    .map(|(_, r)| r)
                    .collect();
            } else if first_b == "selection" || yes_selection {
                // Keep only S rows
                filtered.retain(|r| r.output.contains('S'));
            }
        }

        misclass_rows = filtered;
    }

    // Process confounding rows
    let confounding_rows: Vec<ArgTabRow> = if yes_confounding {
        sub_tab
            .iter()
            .filter(|r| r.bias == "confounding")
            .cloned()
            .collect()
    } else {
        vec![]
    };

    // Combined selection + confounding rows
    let combined_rows: Vec<ArgTabRow> = if yes_both {
        sub_tab
            .iter()
            .filter(|r| r.bias == "confounding and selection")
            .cloned()
            .collect()
    } else {
        vec![]
    };

    // Assemble parameters
    let mut params = vec![];
    for r in &confounding_rows {
        params.push(ParamRow {
            bias: r.bias.to_string(),
            output: r.output.to_string(),
            argument: r.argument.to_string(),
            latex: r.latex.to_string(),
        });
    }
    for r in &combined_rows {
        params.push(ParamRow {
            bias: r.bias.to_string(),
            output: r.output.to_string(),
            argument: r.argument.to_string(),
            latex: r.latex.to_string(),
        });
    }
    for r in &selection_rows {
        params.push(ParamRow {
            bias: r.bias.to_string(),
            output: r.output.to_string(),
            argument: r.argument.to_string(),
            latex: r.latex.to_string(),
        });
    }
    for r in &misclass_rows {
        params.push(ParamRow {
            bias: r.bias.to_string(),
            output: r.output.to_string(),
            argument: r.argument.to_string(),
            latex: r.latex.to_string(),
        });
    }

    // Compute total n, d
    let mut total_n: i32 = biases.iter().map(|b| b.n).sum();
    let mut total_d: i32 = biases.iter().map(|b| b.d).sum();
    if yes_both {
        total_n -= 2; // combined selection/confounding
        total_d -= 1;
    }

    let messages: Vec<String> = biases.iter().flat_map(|b| b.messages.clone()).collect();

    Ok(MultiBias {
        biases: biases.to_vec(),
        n: total_n,
        d: total_d,
        parameters: params,
        messages,
    })
}

/// Get the bias name string for table matching.
fn bias_name(b: &Bias) -> String {
    match b.kind {
        BiasKind::Confounding => "confounding".into(),
        BiasKind::Selection => "selection".into(),
        BiasKind::OutcomeMisclassification => "outcome misclassification".into(),
        BiasKind::ExposureMisclassification => "exposure misclassification".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_confounding() {
        let c = confounding();
        assert_eq!(c.n, 2);
        assert_eq!(c.d, 1);
    }

    #[test]
    fn test_selection_general() {
        let s = selection(&["general"]).unwrap();
        assert_eq!(s.n, 4);
        assert_eq!(s.d, 2);
    }

    #[test]
    fn test_selection_selected() {
        let s = selection(&["selected"]).unwrap();
        assert_eq!(s.n, 2);
        assert_eq!(s.d, 1);
        assert!(s.selected);
    }

    #[test]
    fn test_selection_increased_risk() {
        let s = selection(&["general", "increased risk"]).unwrap();
        assert_eq!(s.n, 2); // 4 + (-2) = 2
        assert_eq!(s.d, 1); // 2 + (-1) = 1
        assert!(s.increased_risk);
    }

    #[test]
    fn test_selection_decreased_su() {
        let s = selection(&["general", "decreased risk", "S = U"]).unwrap();
        // general(4,2) + decreased(-2,-1) + S=U(-2,-2) + overlap(1,1)
        // n = 4-2-2+1 = 1
        // d = 2-1-2+1 = 0
        assert_eq!(s.n, 1);
        assert_eq!(s.d, 0);
    }

    #[test]
    fn test_selection_su() {
        let s = selection(&["general", "S = U"]).unwrap();
        // general(4,2) + S=U(-2,-2)
        assert_eq!(s.n, 2);
        assert_eq!(s.d, 0);
    }

    #[test]
    fn test_misclassification_outcome() {
        let m = misclassification("outcome", false, false).unwrap();
        assert_eq!(m.n, 1);
        assert_eq!(m.d, 0);
    }

    #[test]
    fn test_misclassification_exposure_rare() {
        let m = misclassification("exposure", true, false).unwrap();
        assert_eq!(m.n, 2);
        assert_eq!(m.d, 0);
    }

    #[test]
    fn test_misclassification_exposure_both_rare() {
        let m = misclassification("exposure", true, true).unwrap();
        assert_eq!(m.n, 1);
        assert_eq!(m.d, 0);
    }

    #[test]
    fn test_selection_incompatible() {
        assert!(selection(&["general", "selected"]).is_err());
        assert!(selection(&["general", "increased risk", "decreased risk"]).is_err());
    }

    // Multi-bias parameter count tests (from test-multiple_biases.R)
    #[test]
    fn test_mb01_params() {
        let mb = multi_bias(&[confounding()]).unwrap();
        assert_eq!(mb.parameters.len(), 2);
        assert_eq!(mb.n, 2);
        assert_eq!(mb.d, 1);
    }

    #[test]
    fn test_mb02_params() {
        let mb = multi_bias(&[selection(&["general"]).unwrap()]).unwrap();
        assert_eq!(mb.parameters.len(), 4);
        assert_eq!(mb.n, 4);
        assert_eq!(mb.d, 2);
    }

    #[test]
    fn test_mb03_params() {
        let mb = multi_bias(&[selection(&["selected"]).unwrap()]).unwrap();
        assert_eq!(mb.parameters.len(), 2);
        assert_eq!(mb.n, 2);
        assert_eq!(mb.d, 1);
    }

    #[test]
    fn test_mb04_params() {
        let mb = multi_bias(&[selection(&["general", "increased risk"]).unwrap()]).unwrap();
        assert_eq!(mb.parameters.len(), 2);
        assert_eq!(mb.n, 2);
        assert_eq!(mb.d, 1);
    }

    #[test]
    fn test_mb05_params() {
        let mb =
            multi_bias(&[selection(&["general", "decreased risk", "S = U"]).unwrap()]).unwrap();
        assert_eq!(mb.parameters.len(), 1);
        assert_eq!(mb.n, 1);
        assert_eq!(mb.d, 0);
    }

    #[test]
    fn test_mb06_params() {
        let mb = multi_bias(&[selection(&["general", "S = U"]).unwrap()]).unwrap();
        assert_eq!(mb.parameters.len(), 2);
        assert_eq!(mb.n, 2);
        assert_eq!(mb.d, 0);
    }

    #[test]
    fn test_mb07_params() {
        let mb = multi_bias(&[misclassification("outcome", false, false).unwrap()]).unwrap();
        assert_eq!(mb.parameters.len(), 1);
        assert_eq!(mb.n, 1);
        assert_eq!(mb.d, 0);
    }

    #[test]
    fn test_mb10_params() {
        let mb = multi_bias(&[confounding(), selection(&["general"]).unwrap()]).unwrap();
        assert_eq!(mb.parameters.len(), 6);
        assert_eq!(mb.n, 6);
        assert_eq!(mb.d, 3);
    }

    #[test]
    fn test_mb11_params() {
        let mb = multi_bias(&[
            confounding(),
            selection(&["general"]).unwrap(),
            misclassification("outcome", false, false).unwrap(),
        ])
        .unwrap();
        assert_eq!(mb.parameters.len(), 7);
        assert_eq!(mb.n, 7);
        assert_eq!(mb.d, 3);
    }
}
