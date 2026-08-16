//! Screening: candidate generation, strong rules, KKT checking.
//!
//! Port of `get_candidates()`, `strong_rules()`, and `check_kkt()` from R.

use super::norms;
use super::{ActiveSet, Candidates, GlinternetData, Norms};

/// Generate the full candidate set (all main effects + all pairwise interactions)
/// and compute their norms for screening.
///
/// Port of `get_candidates()` in R.
pub fn get_candidates(data: &GlinternetData, res: &[f64]) -> Candidates {
    let p_cat = data.p_cat;
    let p_cont = data.p_cont;

    let mut variables = ActiveSet::default();
    let mut norms = Norms::default();

    // Categorical main effects
    if p_cat > 0 {
        variables.cat = Some((1..=p_cat).map(|i| [i]).collect());
        norms.cat = norms::compute_norms_cat(data, res);
    }

    // Continuous main effects
    if p_cont > 0 {
        variables.cont = Some((1..=p_cont).map(|i| [i]).collect());
        norms.cont = norms::compute_norms_cont(data, res);
    }

    // Categorical × Categorical interactions
    if p_cat > 1 {
        let mut pairs: Vec<[usize; 2]> = Vec::new();
        for i in 1..p_cat {
            for j in (i + 1)..=p_cat {
                pairs.push([i, j]);
            }
        }
        if !pairs.is_empty() {
            norms.catcat = norms::compute_norms_cat_cat(data, res, &pairs);
            variables.catcat = Some(pairs);
        }
    }

    // Continuous × Continuous interactions
    if p_cont > 1 {
        let mut pairs: Vec<[usize; 2]> = Vec::new();
        for i in 1..p_cont {
            for j in (i + 1)..=p_cont {
                pairs.push([i, j]);
            }
        }
        if !pairs.is_empty() {
            norms.contcont = norms::compute_norms_cont_cont(data, &norms.cont, res, &pairs);
            variables.contcont = Some(pairs);
        }
    }

    // Categorical × Continuous interactions
    if p_cat > 0 && p_cont > 0 {
        let mut pairs: Vec<[usize; 2]> = Vec::new();
        for ci in 1..=p_cat {
            for cj in 1..=p_cont {
                pairs.push([ci, cj]);
            }
        }
        if !pairs.is_empty() {
            norms.catcont = norms::compute_norms_cat_cont(data, &norms.cat, res, &pairs);
            variables.catcont = Some(pairs);
        }
    }

    Candidates { variables, norms }
}

/// Filter helper for 1-element arrays
fn filter1(
    vars: &Option<Vec<[usize; 1]>>,
    norms: &[f64],
    threshold: f64,
) -> Option<Vec<[usize; 1]>> {
    vars.as_ref().and_then(|v| {
        let kept: Vec<[usize; 1]> = v
            .iter()
            .enumerate()
            .filter(|&(i, _)| norms[i] >= threshold)
            .map(|(_, pair)| *pair)
            .collect();
        if kept.is_empty() { None } else { Some(kept) }
    })
}

/// Filter helper for 2-element arrays
fn filter2(
    vars: &Option<Vec<[usize; 2]>>,
    norms: &[f64],
    threshold: f64,
) -> Option<Vec<[usize; 2]>> {
    vars.as_ref().and_then(|v| {
        let kept: Vec<[usize; 2]> = v
            .iter()
            .enumerate()
            .filter(|&(i, _)| norms[i] >= threshold)
            .map(|(_, pair)| *pair)
            .collect();
        if kept.is_empty() { None } else { Some(kept) }
    })
}

/// Apply strong rules to filter candidates.
///
/// Port of `strong_rules()` in R.
/// Keeps groups whose norm ≥ 2·λ_current - λ_prev.
pub fn strong_rules(candidates: &Candidates, lambda: f64, lambda_prev: f64) -> ActiveSet {
    let constant = 2.0 * lambda - lambda_prev;
    if constant <= 0.0 {
        return candidates.variables.clone();
    }

    ActiveSet {
        cat: filter1(&candidates.variables.cat, &candidates.norms.cat, constant),
        cont: filter1(&candidates.variables.cont, &candidates.norms.cont, constant),
        catcat: filter2(
            &candidates.variables.catcat,
            &candidates.norms.catcat,
            constant,
        ),
        contcont: filter2(
            &candidates.variables.contcont,
            &candidates.norms.contcont,
            constant,
        ),
        catcont: filter2(
            &candidates.variables.catcont,
            &candidates.norms.catcont,
            constant,
        ),
    }
}

/// Check KKT conditions on the full candidate set.
///
/// Port of `check_kkt()` in R.
/// Returns (norms, expanded_active_set, all_satisfied).
/// If flag=true, all KKT conditions are satisfied (no violators outside active set).
pub struct KktResult {
    pub norms: Norms,
    pub active_set: ActiveSet,
    pub flag: bool,
}

pub fn check_kkt(
    data: &GlinternetData,
    res: &[f64],
    candidates: &Candidates,
    active: &ActiveSet,
    lambda: f64,
) -> KktResult {
    let p_cat = data.p_cat;
    let p_cont = data.p_cont;

    // Recompute all norms from current residual
    let mut norms = Norms::default();
    if p_cat > 0 {
        norms.cat = norms::compute_norms_cat(data, res);
        if let Some(ref vars) = candidates.variables.catcat {
            norms.catcat = norms::compute_norms_cat_cat(data, res, vars);
        }
    }
    if p_cont > 0 {
        norms.cont = norms::compute_norms_cont(data, res);
        if let Some(ref vars) = candidates.variables.contcont {
            norms.contcont = norms::compute_norms_cont_cont(data, &norms.cont, res, vars);
        }
    }
    if let Some(ref vars) = candidates.variables.catcont {
        norms.catcont = norms::compute_norms_cat_cont(data, &norms.cat, res, vars);
    }

    // Find violators: groups with norm > lambda that are NOT in the active set
    let mut expanded = active.clone();
    let mut flag = true;

    // Main effects (single-column)
    if let Some(ref vars) = candidates.variables.cat {
        for (i, &[vi]) in vars.iter().enumerate() {
            if norms.cat[i] > lambda {
                let already = expanded
                    .cat
                    .as_ref()
                    .is_some_and(|av| av.iter().any(|a| a[0] == vi));
                if !already {
                    match &mut expanded.cat {
                        Some(v) => v.push([vi]),
                        None => expanded.cat = Some(vec![[vi]]),
                    }
                    flag = false;
                }
            }
        }
    }
    if let Some(ref vars) = candidates.variables.cont {
        for (i, &[vi]) in vars.iter().enumerate() {
            if norms.cont[i] > lambda {
                let already = expanded
                    .cont
                    .as_ref()
                    .is_some_and(|av| av.iter().any(|a| a[0] == vi));
                if !already {
                    match &mut expanded.cont {
                        Some(v) => v.push([vi]),
                        None => expanded.cont = Some(vec![[vi]]),
                    }
                    flag = false;
                }
            }
        }
    }

    // Cat × Cat violations
    if let Some(ref vars) = candidates.variables.catcat {
        for (i, pair) in vars.iter().enumerate() {
            if norms.catcat[i] > lambda {
                let already = expanded.catcat.as_ref().is_some_and(|av| {
                    av.iter().any(|a| {
                        (a[0] == pair[0] && a[1] == pair[1]) || (a[0] == pair[1] && a[1] == pair[0])
                    })
                });
                if !already {
                    match &mut expanded.catcat {
                        Some(v) => v.push(*pair),
                        None => expanded.catcat = Some(vec![*pair]),
                    }
                    flag = false;
                }
            }
        }
    }

    // Cont × Cont violations
    if let Some(ref vars) = candidates.variables.contcont {
        for (i, pair) in vars.iter().enumerate() {
            if norms.contcont[i] > lambda {
                let already = expanded.contcont.as_ref().is_some_and(|av| {
                    av.iter().any(|a| {
                        (a[0] == pair[0] && a[1] == pair[1]) || (a[0] == pair[1] && a[1] == pair[0])
                    })
                });
                if !already {
                    match &mut expanded.contcont {
                        Some(v) => v.push(*pair),
                        None => expanded.contcont = Some(vec![*pair]),
                    }
                    flag = false;
                }
            }
        }
    }

    // Cat × Cont violations (order matters: [cat, cont])
    if let Some(ref vars) = candidates.variables.catcont {
        for (i, pair) in vars.iter().enumerate() {
            if norms.catcont[i] > lambda {
                let already = expanded
                    .catcont
                    .as_ref()
                    .is_some_and(|av| av.iter().any(|a| a[0] == pair[0] && a[1] == pair[1]));
                if !already {
                    match &mut expanded.catcont {
                        Some(v) => v.push(*pair),
                        None => expanded.catcont = Some(vec![*pair]),
                    }
                    flag = false;
                }
            }
        }
    }

    KktResult {
        norms,
        active_set: expanded,
        flag,
    }
}
