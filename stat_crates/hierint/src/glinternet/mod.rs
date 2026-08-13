//! glinternet — Group-Lasso INTERaction-NET (Lim & Hastie 2015).
//!
//! Fits linear pairwise-interaction models satisfying **strong hierarchy** via
//! a group-lasso with FISTA. Supports categorical variables (arbitrary levels),
//! continuous variables, and their mixtures, under both Gaussian and logistic loss.

pub mod cv;
pub mod fista;
pub mod norms;
pub mod rescale;
pub mod screening;
pub mod transform;

use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════════════════════════
// Family
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Family {
    Gaussian,
    Binomial,
}

impl Family {
    pub fn is_gaussian(self) -> bool {
        matches!(self, Family::Gaussian)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Configuration
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlinternetConfig {
    pub family: Family,
    pub n_lambda: usize,
    pub lambda_min_ratio: f64,
    pub tol: f64,
    pub max_iter: usize,
    pub screen_limit: Option<usize>,
    pub num_to_find: Option<usize>,
    pub lambda: Option<Vec<f64>>,
}

impl Default for GlinternetConfig {
    fn default() -> Self {
        Self {
            family: Family::Gaussian,
            n_lambda: 50,
            lambda_min_ratio: 0.01,
            tol: 1e-5,
            max_iter: 5000,
            screen_limit: None,
            num_to_find: None,
            lambda: None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// ActiveSet — indices of variables/groups in the model
// ═══════════════════════════════════════════════════════════════════════

/// Active set of main effects and interactions.
///
/// Indices are 1-based within each type (matching R/C convention):
/// - `cat` / `cont`: single-column matrices (1 × k)
/// - `catcat` / `contcont` / `catcont`: 2-column matrices (each row is a pair)
#[derive(Debug, Clone, Default)]
pub struct ActiveSet {
    /// Categorical main effects: each row [cat_var_idx] (1-based within cat vars)
    pub cat: Option<Vec<[usize; 1]>>,
    /// Continuous main effects: each row [cont_var_idx]
    pub cont: Option<Vec<[usize; 1]>>,
    /// Cat×Cat interactions: each row [cat_var_i, cat_var_j]
    pub catcat: Option<Vec<[usize; 2]>>,
    /// Cont×Cont interactions: each row [cont_var_i, cont_var_j]
    pub contcont: Option<Vec<[usize; 2]>>,
    /// Cat×Cont interactions: each row [cat_var_idx, cont_var_idx]
    pub catcont: Option<Vec<[usize; 2]>>,
}

impl ActiveSet {
    pub fn is_empty(&self) -> bool {
        self.num_groups() == 0
    }

    pub fn num_groups(&self) -> usize {
        self.n_vars().iter().sum()
    }

    /// Number of groups per type: [cat, cont, catcat, contcont, catcont]
    pub fn n_vars(&self) -> [usize; 5] {
        fn count<T>(v: &Option<Vec<T>>) -> usize {
            v.as_ref().map_or(0, |x| x.len())
        }
        [
            count(&self.cat),
            count(&self.cont),
            count(&self.catcat),
            count(&self.contcont),
            count(&self.catcont),
        ]
    }

    /// Get group sizes: one entry per group (matching C `groupSizes`)
    pub fn group_sizes(&self, num_levels: &[usize]) -> Vec<usize> {
        let mut sizes = Vec::new();
        if let Some(ref cat) = self.cat {
            for &[i] in cat {
                sizes.push(num_levels[i - 1]);
            }
        }
        if let Some(ref cont) = self.cont {
            for _ in cont {
                sizes.push(1);
            }
        }
        if let Some(ref catcat) = self.catcat {
            for &[i, j] in catcat {
                sizes.push(num_levels[i - 1] * num_levels[j - 1]);
            }
        }
        if let Some(ref contcont) = self.contcont {
            for _ in contcont {
                sizes.push(3);
            }
        }
        if let Some(ref catcont) = self.catcont {
            for &[i, _] in catcont {
                sizes.push(2 * num_levels[i - 1]);
            }
        }
        sizes
    }

    /// Total coefficient length (sum of group sizes)
    pub fn beta_len(&self, num_levels: &[usize]) -> usize {
        self.group_sizes(num_levels).iter().sum()
    }

    /// Prune groups whose coefficients are all (near) zero and return pruned set.
    pub fn prune(&mut self, beta: &[f64], num_levels: &[usize]) {
        let sizes = self.group_sizes(num_levels);
        let mut offset = 0;
        let mut active_flags: Vec<Vec<bool>> = Vec::new(); // per-type active indices

        for group_type in 0..5 {
            let n_type = self.n_vars()[group_type];
            let mut active = Vec::new();
            for _ in 0..n_type {
                let size = sizes.get(offset).copied().unwrap_or(0);
                // In the C code, eps=0.0 — a group is active if any coefficient is nonzero
                let is_active =
                    (0..size).any(|i| beta.get(offset + i).copied().unwrap_or(0.0) != 0.0);
                active.push(is_active);
                offset += 1; // we advance through groups, but sizes are indexed differently
            }
            // Fix: we need to track offset properly through the flat beta
            active_flags.push(active);
        }

        // Redo with correct offset tracking
        let mut offset = 0usize;
        let mut keep: [Vec<usize>; 5] = Default::default();

        for group_type in 0..5 {
            let n_type = self.n_vars()[group_type];
            for g in 0..n_type {
                let size = match group_type {
                    0 => num_levels[self.cat.as_ref().unwrap()[g][0] - 1],
                    1 => 1,
                    2 => {
                        let [i, j] = self.catcat.as_ref().unwrap()[g];
                        num_levels[i - 1] * num_levels[j - 1]
                    }
                    3 => 3,
                    4 => 2 * num_levels[self.catcont.as_ref().unwrap()[g][0] - 1],
                    _ => unreachable!(),
                };
                let is_active = (0..size).any(|i| beta[offset + i] != 0.0);
                if is_active {
                    keep[group_type].push(g);
                }
                offset += size;
            }
        }

        // Apply pruning
        if !keep[0].is_empty() {
            let cat = self.cat.as_ref().unwrap();
            self.cat = Some(keep[0].iter().map(|&g| cat[g]).collect());
        } else {
            self.cat = None;
        }
        if !keep[1].is_empty() {
            let cont = self.cont.as_ref().unwrap();
            self.cont = Some(keep[1].iter().map(|&g| cont[g]).collect());
        } else {
            self.cont = None;
        }
        if !keep[2].is_empty() {
            let catcat = self.catcat.as_ref().unwrap();
            self.catcat = Some(keep[2].iter().map(|&g| catcat[g]).collect());
        } else {
            self.catcat = None;
        }
        if !keep[3].is_empty() {
            let contcont = self.contcont.as_ref().unwrap();
            self.contcont = Some(keep[3].iter().map(|&g| contcont[g]).collect());
        } else {
            self.contcont = None;
        }
        if !keep[4].is_empty() {
            let catcont = self.catcont.as_ref().unwrap();
            self.catcont = Some(keep[4].iter().map(|&g| catcont[g]).collect());
        } else {
            self.catcont = None;
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Candidates — full candidate set + precomputed norms for screening
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default)]
pub struct Candidates {
    pub variables: ActiveSet,
    pub norms: Norms,
}

#[derive(Debug, Clone, Default)]
pub struct Norms {
    pub cat: Vec<f64>,
    pub cont: Vec<f64>,
    pub catcat: Vec<f64>,
    pub contcont: Vec<f64>,
    pub catcont: Vec<f64>,
}

impl Norms {
    pub fn max_norm(&self) -> f64 {
        let m = |v: &[f64]| v.iter().fold(0.0f64, |a, &b| a.max(b));
        m(&self.cat)
            .max(m(&self.cont))
            .max(m(&self.catcat))
            .max(m(&self.contcont))
            .max(m(&self.catcont))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Fitted model
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct GlinternetFit {
    pub intercept: Vec<f64>, // per-lambda intercept (rescaled)
    pub lambda: Vec<f64>,
    pub active_set: Vec<ActiveSet>,
    pub betahat: Vec<Vec<f64>>, // per-lambda rescaled coefficients
    pub obj_value: Vec<f64>,
    pub num_levels: Vec<usize>,
    pub family: Family,
}

/// Data layout: categorical vars stored as integer codes, continuous standardized.
#[derive(Debug, Clone)]
pub struct GlinternetData {
    /// Categorical data: n × pCat matrix of integer level codes (0-based).
    /// Stored column-major: `xcat[j * n + i]` = level of variable j at observation i.
    pub xcat: Vec<usize>,
    /// Continuous data: n × pCont matrix, standardized to unit norm + mean zero.
    /// Stored column-major: `z[j * n + i]`.
    pub z: Vec<f64>,
    pub n: usize,
    pub p_cat: usize,
    pub p_cont: usize,
    /// Number of levels for each variable in the ORIGINAL column order.
    pub num_levels: Vec<usize>,
    /// Maps: cat_indices[i] = original column index of i-th categorical variable (0-based).
    pub cat_indices: Vec<usize>,
    /// Maps: cont_indices[i] = original column index of i-th continuous variable.
    pub cont_indices: Vec<usize>,
    /// Levels of categorical variables only (for the cat subset).
    pub levels: Vec<usize>,
}

impl GlinternetData {
    /// Prepare data from raw matrices.
    ///
    /// - `x_cat`: n × pCat, integer levels coded as {0, 1, ..., L-1}
    /// - `z_raw`: n × pCont, raw continuous values (will be standardized)
    /// - `num_levels`: per-variable level count (1 for continuous)
    pub fn new(x_cat_raw: &[usize], z_raw: &[f64], num_levels: &[usize], n: usize) -> Self {
        let cat_indices: Vec<usize> = (0..num_levels.len())
            .filter(|&i| num_levels[i] > 1)
            .collect();
        let cont_indices: Vec<usize> = (0..num_levels.len())
            .filter(|&i| num_levels[i] == 1)
            .collect();
        let p_cat = cat_indices.len();
        let p_cont = cont_indices.len();
        let levels: Vec<usize> = cat_indices.iter().map(|&i| num_levels[i]).collect();

        // Extract categorical columns (x_cat_raw is already column-major for cat vars only)
        let xcat: Vec<usize> = if p_cat > 0 {
            x_cat_raw[..p_cat * n].to_vec()
        } else {
            Vec::new()
        };

        // Standardize continuous columns: center to mean 0, scale to unit L2 norm
        let z: Vec<f64> = if p_cont > 0 {
            let mut v = vec![0.0f64; p_cont * n];
            for ci in 0..p_cont {
                let mut mean = 0.0;
                let mut norm_sq = 0.0;
                for i in 0..n {
                    let val = z_raw[ci * n + i];
                    v[ci * n + i] = val;
                    mean += val;
                    norm_sq += val * val;
                }
                mean /= n as f64;
                // Standardize: (x - mean) / norm, where norm = sqrt(sum((x-mean)^2))
                let var = norm_sq - n as f64 * mean * mean;
                let norm = if var.abs() > 1e-30 { var.sqrt() } else { 1.0 };
                for i in 0..n {
                    v[ci * n + i] = (v[ci * n + i] - mean) / norm;
                }
            }
            v
        } else {
            Vec::new()
        };

        Self {
            xcat,
            z,
            n,
            p_cat,
            p_cont,
            num_levels: num_levels.to_vec(),
            cat_indices,
            cont_indices,
            levels,
        }
    }

    /// Get the original (1-based) column index for a categorical variable at position `cat_pos`.
    /// Returns the position within the original categorical column subset.
    /// In the C code, catIndices[p] stores 1-based original indices.
    pub fn cat_orig_1idx(&self, cat_pos: usize) -> usize {
        cat_pos + 1
    }

    /// Get the original (1-based) column index for a continuous variable at position `cont_pos`.
    pub fn cont_orig_1idx(&self, cont_pos: usize) -> usize {
        cont_pos + 1
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Standardization helper for continuous raw data
// ═══════════════════════════════════════════════════════════════════════

/// Standardize a continuous vector: subtract mean, divide by L2 norm.
/// Returns (standardized_values, mean, norm).
pub fn standardize(x: &[f64]) -> (Vec<f64>, f64, f64) {
    let n = x.len();
    let mean = x.iter().sum::<f64>() / n as f64;
    let var: f64 = x.iter().map(|&v| (v - mean).powi(2)).sum();
    let norm = if var.abs() > 1e-30 { var.sqrt() } else { 1.0 };
    let standardized: Vec<f64> = x.iter().map(|&v| (v - mean) / norm).collect();
    (standardized, mean, norm)
}

// ═══════════════════════════════════════════════════════════════════════
// Main fit entry point
// ═══════════════════════════════════════════════════════════════════════

/// Fit glinternet model.
///
/// - `x_cat_raw`: n × pCat column-major integer codes (0-based levels). Empty if no categorical.
/// - `z_raw`: n × pCont column-major raw continuous values. Empty if no continuous.
/// - `y`: response vector (length n)
/// - `num_levels`: per-variable level count (1 = continuous)
/// - `config`: fitting options
pub fn fit(
    x_cat_raw: &[usize],
    z_raw: &[f64],
    y: &[f64],
    num_levels: &[usize],
    config: &GlinternetConfig,
) -> crate::Result<GlinternetFit> {
    let n = y.len();
    if n == 0 {
        return Err(crate::HierIntError::Empty);
    }
    let p_cat = num_levels.iter().filter(|&&l| l > 1).count();
    let p_cont = num_levels.iter().filter(|&&l| l == 1).count();

    let data = GlinternetData::new(x_cat_raw, z_raw, num_levels, n);

    // Initial residual: Y - mean(Y)
    let y_mean = y.iter().sum::<f64>() / n as f64;
    let mut res: Vec<f64> = y.iter().map(|&v| v - y_mean).collect();

    // Get candidate set + norms
    let mut candidates = screening::get_candidates(&data, &res);

    // Lambda grid
    let lambda: Vec<f64> = if let Some(ref lam) = config.lambda {
        let mut l = lam.clone();
        if l.len() == 1 {
            let lambda_max = norms::get_lambda_max(&candidates);
            l.push(lambda_max);
            l.sort_by(|a, b| b.partial_cmp(a).unwrap());
        }
        l
    } else {
        norms::get_lambda_grid(&candidates, config.n_lambda, config.lambda_min_ratio)
    };
    let n_lambda = lambda.len();

    // Storage
    let mut active_sets: Vec<ActiveSet> = vec![ActiveSet::default(); n_lambda];
    let mut betahats: Vec<Vec<f64>> = vec![Vec::new(); n_lambda];
    let mut obj_values: Vec<f64> = vec![0.0; n_lambda];
    let mut intercepts: Vec<f64> = vec![0.0; n_lambda];

    // Lambda 0: empty model
    let init_intercept = if config.family.is_gaussian() {
        y_mean
    } else {
        let p = y_mean;
        -(1.0 / p - 1.0).ln()
    };
    intercepts[0] = init_intercept;
    betahats[0] = vec![init_intercept];
    obj_values[0] = if config.family.is_gaussian() {
        res.iter().map(|r| r * r).sum::<f64>() / (2.0 * n as f64)
    } else {
        -y_mean * init_intercept + (1.0 / (1.0 - y_mean)).ln()
    };

    let mut last_idx = 0;

    for i in 1..n_lambda {
        // Strong rules screening
        let strong_set = screening::strong_rules(&candidates, lambda[i], lambda[i - 1]);

        // Initialize betahat via warm start
        let group_sizes = strong_set.group_sizes(&data.levels);
        let mut beta = rescale::initialize_betahat(
            &strong_set,
            &active_sets[i - 1],
            &betahats[i - 1],
            &data.levels,
        );
        let mut active = strong_set;

        // Iterate: fit group lasso, check KKT, repeat if violations
        loop {
            let total_groups = active.num_groups();
            if total_groups == 0 {
                // Empty active set — just intercept
                let mu = if config.family.is_gaussian() {
                    y_mean
                } else {
                    let p = y_mean;
                    -(1.0 / p - 1.0).ln()
                };
                beta = vec![mu];
                res = y.iter().map(|&v| v - mu).collect();
                let obj = if config.family.is_gaussian() {
                    res.iter().map(|r| r * r).sum::<f64>() / (2.0 * n as f64)
                } else {
                    -y_mean * mu + (1.0 / (1.0 - y_mean)).ln()
                };
                obj_values[i] = obj;
                break;
            }

            let (intercept, beta_fit, res_fit, obj, group_sizes_fit) = fista::group_lasso(
                &data,
                y,
                &active,
                &beta,
                lambda[i],
                config.family,
                config.tol,
                config.max_iter,
            );

            beta = {
                let mut b = vec![intercept];
                b.extend_from_slice(&beta_fit);
                b
            };

            // Prune zero groups
            let mut active_pruned = active.clone();
            // Extract non-intercept coefficients
            active_pruned.prune(&beta_fit, &data.levels);
            active = active_pruned;

            // Update residual
            res = res_fit;

            obj_values[i] = obj;

            // Check KKT conditions on full candidate set
            let kkt = screening::check_kkt(&data, &res, &candidates, &active, lambda[i]);
            candidates.norms = kkt.norms;

            if kkt.flag {
                break;
            }

            // Violators found — expand active set and reinitialize
            beta = rescale::initialize_betahat(&kkt.active_set, &active, &beta, &data.levels);
            active = kkt.active_set;
        }

        active_sets[i] = active;
        betahats[i] = beta;
        last_idx = i;

        // Update screening if screen_limit is set
        if let Some(_sl) = config.screen_limit {
            candidates = screening::get_candidates(&data, &res);
        }

        // Check numToFind
        if let Some(ntf) = config.num_to_find {
            let num_found = active_sets[i].n_vars()[2]
                + active_sets[i].n_vars()[3]
                + active_sets[i].n_vars()[4];
            if num_found >= ntf {
                break;
            }
        }
    }

    // Rescale betahat (undo standardization)
    let actual_n = last_idx + 1;
    let mut rescaled_intercepts = Vec::with_capacity(actual_n);
    let mut rescaled_betas = Vec::with_capacity(actual_n);

    for j in 0..actual_n {
        let (mu, beta_rescaled) = rescale::rescale_betahat(&active_sets[j], &betahats[j], &data);
        rescaled_intercepts.push(mu);
        rescaled_betas.push(beta_rescaled);
    }

    Ok(GlinternetFit {
        intercept: rescaled_intercepts,
        lambda: lambda[..actual_n].to_vec(),
        active_set: active_sets[..actual_n].to_vec(),
        betahat: rescaled_betas,
        obj_value: obj_values[..actual_n].to_vec(),
        num_levels: num_levels.to_vec(),
        family: config.family,
    })
}

/// Predict from a fitted glinternet model.
///
/// Returns predicted values for each lambda in the path.
/// For Gaussian: returns fitted values on the response scale.
/// For Binomial: returns predicted probabilities.
pub fn predict(
    fit: &GlinternetFit,
    x_cat_raw: &[usize],
    z_raw: &[f64],
    num_levels: &[usize],
    n: usize,
) -> Vec<Vec<f64>> {
    let data = GlinternetData::new(x_cat_raw, z_raw, num_levels, n);
    let n_lambda = fit.lambda.len();

    let mut predictions = Vec::with_capacity(n_lambda);
    for j in 0..n_lambda {
        let pred = rescale::predict_one(&fit.active_set[j], &fit.betahat[j], &data);
        predictions.push(pred);
    }
    predictions
}
