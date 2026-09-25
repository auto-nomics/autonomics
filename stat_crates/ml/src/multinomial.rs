//! Multinomial logistic regression with elastic-net penalty.
//!
//! Symmetric parameterisation `W ∈ R^{k×p}` + unpenalised per-class
//! intercepts, glmnet-compatible objective
//!
//! `L(W, b) = −(1/n) Σ_i ω_i·log p_{i,y_i}
//!            + λ·(α‖W‖₁ + (1−α)/2·‖W‖²_F)`
//!
//! with per-sample loss weights `ω_i` (1, or balanced).  Solved by FISTA
//! with soft-thresholding and backtracking line search (Beck–Teboulle),
//! warm-started down a λ path.
//!
//! glmnet(`family="multinomial"`) conventions verified numerically
//! (2026-09-25, glmnet 5.1, machine precision unless noted):
//!
//! - features standardise internally with mean and **n-divisor** SD; the
//!   reported coefficients are back-transformed to the original scale
//!   (`w_orig = w_z/s`, `b_orig = b_z − Σ w_orig·mean`), and
//!   `predict(type="response")` is exactly `softmax(b_orig + X w_orig)`;
//! - `λ_max = max_{k,j} |(1/n) Σ_i ω_i(π_k − y_ik)·z_ij| / α` (gradient of
//!   the unpenalised NLL at `W = 0`, `π` = weighted class frequencies);
//!   for `α = 0` the division is skipped (path anchor only);
//! - `df` counts features nonzero in **any** class;
//! - `cvm` = pooled out-of-fold mean of the per-sample multinomial
//!   deviance `−2·log p_{i,y_i}`, `cvsd = sqrt(Σ_f ω_f (d̄_f −
//!   cvm)² / Σ ω_f / (nfolds − 1))` with fold sizes `ω_f`;
//!   `λ.min` = first minimum along the descending path (ties → larger λ),
//!   `λ.1se` = largest λ with `cvm ≤ cvm_min + cvsd_min`.
//!
//! The multinomial objective has flat valleys (e.g. adding a common
//! `v(x)` to every class's score leaves the likelihood unchanged), so
//! coefficient vectors from different solvers drift along them even at
//! equal objective value — two glmnet runs on equivalent
//! parameterisations differ by ~2e-2 in coefficients at objective gap
//! 1e-7.  Probabilities, support sets, objective values and the λ grid
//! are the identified quantities; the glmnet golden compares those
//! (probabilities agree to ~3e-4), not coefficients at 1e-3.

use faer::Mat;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::centroid::ClassProbs;
use crate::split::Fold;

/// Artifact kind tag for multinomial elastic-net models.
pub const MULTINOMIAL_ENET_KIND: &str = "multinomial_enet:v1";

#[derive(Debug, Error)]
pub enum MultinomialError {
    #[error("empty input")]
    Empty,
    #[error("labels length ({labels}) doesn't match data rows ({rows})")]
    LabelMismatch { labels: usize, rows: usize },
    #[error("need >= 2 classes, got {0}")]
    TooFewClasses(usize),
    #[error("label {label} is not < k = {k} (labels must be contiguous 0..k)")]
    BadLabel { label: usize, k: usize },
    #[error("class {0} has no training samples in a CV fold")]
    MissingClass(usize),
    #[error("feature matrix contains non-finite values")]
    NonFinite,
    #[error("bad config: {0}")]
    BadConfig(String),
    #[error("FISTA did not converge at lambda={lambda:.6} within {iters} iterations")]
    NoConverge { lambda: f64, iters: usize },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, MultinomialError>;

/// How `lambda_selected` is chosen from the CV path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LambdaSelect {
    /// λ of the minimum CV deviance (first minimum along the descending
    /// path, i.e. ties resolve to the larger λ — glmnet `lambda.min`).
    Min,
    /// Largest λ with CV deviance <= min + SE of the minimum (glmnet
    /// `lambda.1se`).
    OneSe,
}

/// Per-sample loss weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassWeight {
    /// Uniform weights (glmnet default).
    None,
    /// `ω_i ∝ 1/class frequency`, normalised to mean 1.
    Balanced,
}

/// Solver + path configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MnetConfig {
    /// Elastic-net mixing; 1 = lasso, 0 = ridge.  Default 1.
    pub alpha: f64,
    /// Path length when `lambda_grid` is not given.  Default 100.
    pub n_lambda: usize,
    /// `λ_min/λ_max`; None → 1e-3 when n >= p else 1e-2 (glmnet default).
    pub min_ratio: Option<f64>,
    /// Explicit descending λ path (overrides n_lambda/min_ratio).
    pub lambda_grid: Option<Vec<f64>>,
    /// FISTA iteration cap per λ.  Default 1000.
    pub max_iter: usize,
    /// Fractional objective-change convergence tolerance (glmnet-style
    /// `thresh`).  Default 1e-6.
    pub tol: f64,
    /// Per-sample loss weighting.  Default none.
    pub class_weight: ClassWeight,
}

impl Default for MnetConfig {
    fn default() -> Self {
        MnetConfig {
            alpha: 1.0,
            n_lambda: 100,
            min_ratio: None,
            lambda_grid: None,
            max_iter: 1000,
            tol: 1e-6,
            class_weight: ClassWeight::None,
        }
    }
}

impl MnetConfig {
    fn validate(&self) -> Result<()> {
        if !(0.0..=1.0).contains(&self.alpha) || !self.alpha.is_finite() {
            return Err(MultinomialError::BadConfig(format!(
                "alpha must be in [0, 1], got {}",
                self.alpha
            )));
        }
        if let Some(r) = self.min_ratio {
            if !r.is_finite() || !(0.0..1.0).contains(&r) {
                return Err(MultinomialError::BadConfig(format!(
                    "min_ratio must be in (0, 1), got {r}"
                )));
            }
        }
        if let Some(g) = &self.lambda_grid {
            if g.len() < 2 {
                return Err(MultinomialError::BadConfig(
                    "lambda_grid needs >= 2 descending values".into(),
                ));
            }
            if g.iter().any(|v| !v.is_finite() || *v <= 0.0) {
                return Err(MultinomialError::BadConfig(
                    "lambda_grid values must be positive and finite".into(),
                ));
            }
            if g.windows(2).any(|w| w[0] < w[1]) {
                return Err(MultinomialError::BadConfig(
                    "lambda_grid must be descending".into(),
                ));
            }
        } else if self.n_lambda < 2 {
            return Err(MultinomialError::BadConfig(format!(
                "n_lambda must be >= 2, got {}",
                self.n_lambda
            )));
        }
        if self.max_iter == 0 {
            return Err(MultinomialError::BadConfig("max_iter must be >= 1".into()));
        }
        if !self.tol.is_finite() || self.tol <= 0.0 {
            return Err(MultinomialError::BadConfig(format!(
                "tol must be > 0, got {}",
                self.tol
            )));
        }
        Ok(())
    }
}

/// Fitted multinomial elastic-net model (`kind = "multinomial_enet:v1"`).
///
/// Coefficients are stored on the **original** feature scale (glmnet-style
/// back-transform); `x_mean`/`x_sd` are the training standardisation
/// parameters, kept for transparency.  `mnet_predict` needs nothing else.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultinomialEnetModel {
    pub class_labels: Vec<String>,
    /// Per-class intercepts, original scale.
    pub intercept: Vec<f64>,
    /// `k × p` coefficient rows, original scale.
    pub weights: Vec<Vec<f64>>,
    pub x_mean: Vec<f64>,
    pub x_sd: Vec<f64>,
    pub alpha: f64,
    pub lambda_selected: f64,
}

/// CV deviance curve over the λ path.
#[derive(Debug, Clone)]
pub struct MnetCvPath {
    /// The evaluated grid (descending).
    pub lambda: Vec<f64>,
    /// Pooled out-of-fold mean deviance per λ (empty without CV).
    pub cv_deviance: Vec<f64>,
    /// Fold-level SE per λ (empty without CV).
    pub cv_se: Vec<f64>,
    /// Features with any nonzero coefficient per λ (glmnet `df`).
    pub nonzero_features: Vec<usize>,
    /// λ of the minimum CV deviance (grid[0] without CV).
    pub lambda_min: f64,
    /// Largest λ within one SE of the minimum (grid[0] without CV).
    pub lambda_1se: f64,
}

/// One coef-table row: a nonzero coefficient at the selected λ.
#[derive(Debug, Clone)]
pub struct CoefEntry {
    pub class_index: usize,
    pub class_label: String,
    pub feature_index: usize,
    pub feature_name: String,
    /// Original-scale coefficient.
    pub coef: f64,
}

// ── standardisation ───────────────────────────────────────────────────────

struct Standardized {
    z: Mat<f64>,
    mean: Vec<f64>,
    sd: Vec<f64>,
}

/// glmnet-style: mean and **n-divisor** SD; zero-variance columns get
/// `sd = 1` and a zero z column (their coefficient stays exactly 0 under
/// the penalty, like glmnet).
fn standardize(x: &Mat<f64>) -> Standardized {
    let n = x.nrows();
    let p = x.ncols();
    let mut mean = vec![0.0; p];
    for i in 0..n {
        for j in 0..p {
            mean[j] += x[(i, j)];
        }
    }
    for m in &mut mean {
        *m /= n as f64;
    }
    let mut sd = vec![0.0; p];
    for i in 0..n {
        for j in 0..p {
            let d = x[(i, j)] - mean[j];
            sd[j] += d * d;
        }
    }
    for s in &mut sd {
        *s = (*s / n as f64).sqrt();
        if *s == 0.0 {
            *s = 1.0;
        }
    }
    let z = Mat::from_fn(n, p, |i, j| (x[(i, j)] - mean[j]) / sd[j]);
    Standardized { z, mean, sd }
}

fn sample_weights(y: &[usize], n: usize, k: usize, cw: ClassWeight) -> Vec<f64> {
    match cw {
        ClassWeight::None => vec![1.0; n],
        ClassWeight::Balanced => {
            let mut nk = vec![0usize; k];
            for &c in y {
                nk[c] += 1;
            }
            let mut w: Vec<f64> = y
                .iter()
                .map(|&c| n as f64 / (k as f64 * nk[c].max(1) as f64))
                .collect();
            let total: f64 = w.iter().sum();
            for v in &mut w {
                *v *= n as f64 / total; // mean 1, keeps the λ scale comparable
            }
            w
        }
    }
}

// ── λ path ────────────────────────────────────────────────────────────────

/// glmnet `λ_max`: max |gradient of the unpenalised NLL at W = 0| on the
/// standardised scale, divided by α (skipped when α = 0).
fn lambda_max(z: &Mat<f64>, y: &[usize], sw: &[f64], k: usize, alpha: f64) -> f64 {
    let n = z.nrows();
    let p = z.ncols();
    let total_w: f64 = sw.iter().sum();
    let mut pi = vec![0.0; k];
    for (i, &c) in y.iter().enumerate() {
        pi[c] += sw[i];
    }
    for v in &mut pi {
        *v /= total_w;
    }
    let mut best = 0.0f64;
    for c in 0..k {
        for j in 0..p {
            let mut g = 0.0;
            for i in 0..n {
                let resid = if y[i] == c { pi[c] - 1.0 } else { pi[c] };
                g += sw[i] * resid * z[(i, j)];
            }
            best = best.max((g / n as f64).abs());
        }
    }
    if alpha > 1e-12 { best / alpha } else { best }
}

fn default_path(z: &Mat<f64>, y: &[usize], sw: &[f64], k: usize, cfg: &MnetConfig) -> Vec<f64> {
    let lmax = lambda_max(z, y, sw, k, cfg.alpha);
    let n = z.nrows();
    let p = z.ncols();
    let min_ratio = cfg.min_ratio.unwrap_or(if n >= p { 1e-3 } else { 1e-2 });
    let lmin = lmax * min_ratio;
    let nl = cfg.n_lambda;
    let log_step = (lmin.ln() - lmax.ln()) / (nl - 1) as f64;
    (0..nl)
        .map(|i| (lmax.ln() + i as f64 * log_step).exp())
        .collect()
}

// ── FISTA solver ──────────────────────────────────────────────────────────

/// z-scale solution at one λ (intercept `b` + flat `w`, k*p).
#[derive(Clone)]
struct ZParams {
    b: Vec<f64>,
    w: Vec<f64>,
}

/// One solution point per λ, with warm-start state carried down the path.
struct PathFit {
    coefs: Vec<ZParams>,
    iters: Vec<usize>,
}

struct Fista<'a> {
    /// Row-major copy of the design — the hot loops index this flat layout.
    zf: Vec<f64>,
    y: &'a [usize],
    sw: &'a [f64],
    n: usize,
    p: usize,
    k: usize,
    max_iter: usize,
    tol: f64,
    // scratch
    eta: Vec<f64>,
    probs: Vec<f64>,
    g_b: Vec<f64>,
    g_w: Vec<f64>,
}

fn soft(v: f64, t: f64) -> f64 {
    v.signum() * (v.abs() - t).max(0.0)
}

impl<'a> Fista<'a> {
    fn new(
        z: &Mat<f64>,
        y: &'a [usize],
        sw: &'a [f64],
        k: usize,
        max_iter: usize,
        tol: f64,
    ) -> Self {
        let n = z.nrows();
        let p = z.ncols();
        let mut zf = vec![0.0; n * p];
        for i in 0..n {
            for j in 0..p {
                zf[i * p + j] = z[(i, j)];
            }
        }
        Fista {
            zf,
            y,
            sw,
            n,
            p,
            k,
            max_iter,
            tol,
            eta: vec![0.0; n * k],
            probs: vec![0.0; n * k],
            g_b: vec![0.0; k],
            g_w: vec![0.0; k * p],
        }
    }

    /// η = b + Z·W into self.eta (row-major n×k).
    fn fill_eta(&mut self, b: &[f64], w: &[f64]) {
        for i in 0..self.n {
            for c in 0..self.k {
                let mut s = b[c];
                let base = c * self.p;
                let zrow = i * self.p;
                for j in 0..self.p {
                    s += self.zf[zrow + j] * w[base + j];
                }
                self.eta[i * self.k + c] = s;
            }
        }
    }

    /// Smooth part of the objective: weighted mean NLL + ridge.
    fn smooth_obj(&mut self, b: &[f64], w: &[f64], lam: f64, alpha: f64) -> f64 {
        self.fill_eta(b, w);
        let mut nll = 0.0;
        for i in 0..self.n {
            let row = &self.eta[i * self.k..(i + 1) * self.k];
            let mx = row.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let lse: f64 = row.iter().map(|&v| (v - mx).exp()).sum::<f64>().ln();
            nll += self.sw[i] * (lse + mx - row[self.y[i]]);
        }
        nll /= self.n as f64;
        let ridge = 0.5 * (1.0 - alpha) * lam * w.iter().map(|v| v * v).sum::<f64>();
        nll + ridge
    }

    /// Gradient of the smooth part at (b, w); also leaves softmax
    /// probabilities in self.probs.
    fn grad(&mut self, b: &[f64], w: &[f64], lam: f64, alpha: f64) {
        self.fill_eta(b, w);
        for i in 0..self.n {
            let row = &self.eta[i * self.k..(i + 1) * self.k];
            let mx = row.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let sum: f64 = row.iter().map(|&v| (v - mx).exp()).sum();
            for c in 0..self.k {
                self.probs[i * self.k + c] = (row[c] - mx).exp() / sum;
            }
        }
        for v in self.g_b.iter_mut() {
            *v = 0.0;
        }
        for v in self.g_w.iter_mut() {
            *v = 0.0;
        }
        for i in 0..self.n {
            let wi = self.sw[i] / self.n as f64;
            for c in 0..self.k {
                let resid = wi * (self.probs[i * self.k + c] - f64::from(self.y[i] == c));
                self.g_b[c] += resid;
                let base = c * self.p;
                let zrow = i * self.p;
                for j in 0..self.p {
                    self.g_w[base + j] += resid * self.zf[zrow + j];
                }
            }
        }
        let ridge = (1.0 - alpha) * lam;
        if ridge > 0.0 {
            for c in 0..self.k {
                let base = c * self.p;
                for j in 0..self.p {
                    self.g_w[base + j] += ridge * w[base + j];
                }
            }
        }
    }

    /// FISTA with backtracking line search at one λ, warm-started from
    /// `start`.  Returns the iteration count; `L` carries the final
    /// Lipschitz estimate for the next λ.
    fn solve(
        &mut self,
        lam: f64,
        alpha: f64,
        start: &ZParams,
        lip: &mut f64,
    ) -> Result<(ZParams, usize)> {
        let threshold = alpha * lam;
        let mut x = start.clone();
        // initial Lipschitz estimate: curvature bound of the CE loss plus
        // the ridge term, then let backtracking correct upwards.
        let mut l = *lip;
        let mut u = x.clone();
        let mut t_prev = 1.0f64;
        let mut f_x = self.smooth_obj(&x.b, &x.w, lam, alpha);
        let mut iter = 0usize;
        for _ in 0..self.max_iter {
            iter += 1;
            self.grad(&u.b, &u.w, lam, alpha);
            let f_u = self.smooth_obj(&u.b, &u.w, lam, alpha);
            // backtracking proximal-gradient step
            let mut cand = ZParams {
                b: vec![0.0; self.k],
                w: vec![0.0; self.k * self.p],
            };
            let mut f_cand;
            loop {
                for c in 0..self.k {
                    cand.b[c] = u.b[c] - self.g_b[c] / l;
                    let base = c * self.p;
                    for j in 0..self.p {
                        cand.w[base + j] =
                            soft(u.w[base + j] - self.g_w[base + j] / l, threshold / l);
                    }
                }
                f_cand = self.smooth_obj(&cand.b, &cand.w, lam, alpha);
                // sufficient-descent check at u
                let mut quad = 0.0;
                let mut dot = 0.0;
                for c in 0..self.k {
                    let db = cand.b[c] - u.b[c];
                    quad += db * db;
                    dot += self.g_b[c] * db;
                }
                for idx in 0..self.k * self.p {
                    let dw = cand.w[idx] - u.w[idx];
                    quad += dw * dw;
                    dot += self.g_w[idx] * dw;
                }
                if f_cand <= f_u + dot + 0.5 * l * quad + 1e-12 {
                    break;
                }
                l *= 2.0;
                if l > 1e18 {
                    return Err(MultinomialError::Other(
                        "backtracking line search diverged".into(),
                    ));
                }
            }
            // convergence: fractional objective change (glmnet-style
            // `thresh`; a parameter-change criterion never fires on
            // separable data, where the MLE diverges linearly)
            let f_prev = f_x;
            // adaptive restart (O'Donoghue & Candès): when the objective
            // rises, the momentum direction is bad — reset the sequence
            let restart = f_cand > f_prev + 1e-14;
            // FISTA momentum
            let t_new = 0.5 * (1.0 + (1.0 + 4.0 * t_prev * t_prev).sqrt());
            let mom = if restart { 0.0 } else { (t_prev - 1.0) / t_new };
            for c in 0..self.k {
                u.b[c] = cand.b[c] + mom * (cand.b[c] - x.b[c]);
            }
            for idx in 0..self.k * self.p {
                u.w[idx] = cand.w[idx] + mom * (cand.w[idx] - x.w[idx]);
            }
            x = cand;
            f_x = f_cand;
            if (f_prev - f_cand).abs() <= self.tol * f_cand.abs().max(1.0) {
                *lip = l;
                return Ok((x, iter));
            }
            t_prev = if restart { 1.0 } else { t_new };
        }
        Err(MultinomialError::NoConverge {
            lambda: lam,
            iters: self.max_iter,
        })
    }

    /// Fit the whole descending λ path with warm starts.
    fn path(&mut self, grid: &[f64], alpha: f64, lip0: f64) -> Result<PathFit> {
        let mut cur = ZParams {
            b: vec![0.0; self.k],
            w: vec![0.0; self.k * self.p],
        };
        let mut lip = lip0;
        let mut coefs = Vec::with_capacity(grid.len());
        let mut iters = Vec::with_capacity(grid.len());
        for &lam in grid {
            let (sol, it) = self.solve(lam, alpha, &cur, &mut lip)?;
            coefs.push(sol.clone());
            iters.push(it);
            cur = sol;
        }
        Ok(PathFit { coefs, iters })
    }
}

/// Features with any nonzero coefficient at this solution (glmnet `df`).
fn df_count(sol: &ZParams, p: usize) -> usize {
    let mut any = vec![false; p];
    for c in 0..sol.w.len() / p {
        for j in 0..p {
            if sol.w[c * p + j] != 0.0 {
                any[j] = true;
            }
        }
    }
    any.iter().filter(|&&a| a).count()
}

/// Back-transform a z-scale solution to the original feature scale.
fn to_original(sol: &ZParams, std: &Standardized, k: usize, p: usize) -> (Vec<f64>, Vec<Vec<f64>>) {
    let mut w_orig = vec![vec![0.0; p]; k];
    for c in 0..k {
        for j in 0..p {
            w_orig[c][j] = sol.w[c * p + j] / std.sd[j];
        }
    }
    let b_orig: Vec<f64> = (0..k)
        .map(|c| sol.b[c] - (0..p).map(|j| w_orig[c][j] * std.mean[j]).sum::<f64>())
        .collect();
    (b_orig, w_orig)
}

// ── public API ────────────────────────────────────────────────────────────

/// Fit the multinomial elastic-net, optionally cross-validating the λ path.
///
/// `folds` = `(train, test)` index pairs from the caller.  Without folds
/// the model is fit at the grid's first λ (strongest regularisation) and
/// the curve's error columns stay empty.  A single-value... a short
/// explicit `lambda_grid` pins the path; selection then picks within it.
#[allow(clippy::too_many_arguments)]
pub fn mnet_fit(
    x: &Mat<f64>,
    y: &[usize],
    class_labels: &[String],
    feature_names: &[String],
    cfg: &MnetConfig,
    folds: Option<&[Fold]>,
    select: LambdaSelect,
) -> Result<(MultinomialEnetModel, MnetCvPath, Vec<CoefEntry>)> {
    let n = x.nrows();
    let p = x.ncols();
    if n == 0 || p == 0 {
        return Err(MultinomialError::Empty);
    }
    if y.len() != n {
        return Err(MultinomialError::LabelMismatch {
            labels: y.len(),
            rows: n,
        });
    }
    if feature_names.len() != p {
        return Err(MultinomialError::Other(format!(
            "feature_names has {} entries, data has {p} columns",
            feature_names.len()
        )));
    }
    let k = class_labels.len();
    if k < 2 {
        return Err(MultinomialError::TooFewClasses(k));
    }
    for &c in y {
        if c >= k {
            return Err(MultinomialError::BadLabel { label: c, k });
        }
    }
    if !x.col_iter().all(|col| col.is_all_finite()) {
        return Err(MultinomialError::NonFinite);
    }
    cfg.validate()?;

    let std = standardize(x);
    let sw = sample_weights(y, n, k, cfg.class_weight);

    // λ grid (descending)
    let grid: Vec<f64> = match &cfg.lambda_grid {
        Some(g) => g.clone(),
        None => default_path(&std.z, y, &sw, k, cfg),
    };

    // full-data path (warm-started)
    let lip0 = {
        let mut bound = 0.0;
        for i in 0..n {
            let mut zsq = 1.0; // intercept dimension
            for j in 0..p {
                zsq += std.z[(i, j)] * std.z[(i, j)];
            }
            bound += sw[i] * zsq;
        }
        0.25 * bound / n as f64 + (1.0 - cfg.alpha) * grid[0]
    };
    let mut fista = Fista::new(&std.z, y, &sw, k, cfg.max_iter, cfg.tol);
    let full = fista.path(&grid, cfg.alpha, lip0)?;

    let nonzero_features: Vec<usize> = full.coefs.iter().map(|s| df_count(s, p)).collect();

    // CV deviance curve: per fold, refit the path on the training part
    // (own standardisation, like cv.glmnet) and score the held-out rows.
    let (cv_deviance, cv_se, lambda_min, lambda_1se) = match folds {
        Some(folds) => {
            let nf = folds.len();
            if nf < 2 {
                return Err(MultinomialError::BadConfig("CV needs >= 2 folds".into()));
            }
            // per-sample OOF deviance: n rows × grid points
            let mut dev = vec![f64::NAN; n * grid.len()];
            let mut fold_sizes = vec![0usize; nf];
            let mut fold_of = vec![usize::MAX; n];
            for (f, (train, test)) in folds.iter().enumerate() {
                let y_train: Vec<usize> = train.iter().map(|&i| y[i]).collect();
                let mut present = vec![false; k];
                for &c in &y_train {
                    present[c] = true;
                }
                if let Some((c, _)) = present.iter().enumerate().find(|&(_, &v)| !v) {
                    return Err(MultinomialError::MissingClass(c));
                }
                let x_train = Mat::from_fn(train.len(), p, |r, j| x[(train[r], j)]);
                let std_f = standardize(&x_train);
                let sw_f = sample_weights(&y_train, train.len(), k, cfg.class_weight);
                let grid_f = match &cfg.lambda_grid {
                    Some(g) => g.clone(),
                    None => default_path(&std_f.z, &y_train, &sw_f, k, cfg),
                };
                let mut fista_f = Fista::new(&std_f.z, &y_train, &sw_f, k, cfg.max_iter, cfg.tol);
                let fit_f = fista_f.path(&grid_f, cfg.alpha, lip0)?;
                // score every λ on the held-out rows (η on the fold's own
                // standardisation)
                for (li, sol) in fit_f.coefs.iter().enumerate() {
                    for &i in test {
                        let mut row = vec![0.0; k];
                        for c in 0..k {
                            let mut s = sol.b[c];
                            for j in 0..p {
                                let z = (x[(i, j)] - std_f.mean[j]) / std_f.sd[j];
                                s += sol.w[c * p + j] * z;
                            }
                            row[c] = s;
                        }
                        let mx = row.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                        let lse: f64 = row.iter().map(|&v| (v - mx).exp()).sum::<f64>().ln();
                        let pv = (row[y[i]] - mx - lse).exp().clamp(1e-15, 1.0);
                        dev[i * grid.len() + li] = -2.0 * pv.ln();
                    }
                }
                fold_sizes[f] = test.len();
                for &i in test {
                    fold_of[i] = f;
                }
            }
            if dev.iter().any(|v| v.is_nan()) {
                return Err(MultinomialError::Other(
                    "CV folds did not cover every row exactly once".into(),
                ));
            }
            // cvm = pooled mean; cvsd from fold-level means (glmnet formula)
            let cvm: Vec<f64> = (0..grid.len())
                .map(|li| (0..n).map(|i| dev[i * grid.len() + li]).sum::<f64>() / n as f64)
                .collect();
            let mut cvsd = vec![0.0f64; grid.len()];
            for f in 0..nf {
                let wf = fold_sizes[f] as f64;
                for li in 0..grid.len() {
                    let fm: f64 = (0..n)
                        .filter(|&i| fold_of[i] == f)
                        .map(|i| dev[i * grid.len() + li])
                        .sum::<f64>()
                        / wf;
                    cvsd[li] += wf * (fm - cvm[li]) * (fm - cvm[li]);
                }
            }
            let wsum = fold_sizes.iter().map(|&s| s as f64).sum::<f64>();
            for v in &mut cvsd {
                *v = (*v / wsum / (nf - 1) as f64).sqrt();
            }
            // λ.min: first minimum along the descending path (ties → larger λ)
            let mut min_i = 0;
            for (i, &e) in cvm.iter().enumerate() {
                if e < cvm[min_i] {
                    min_i = i;
                }
            }
            let bound = cvm[min_i] + cvsd[min_i];
            let mut se_i = min_i;
            for (i, &e) in cvm.iter().enumerate() {
                if e <= bound && grid[i] >= grid[se_i] {
                    se_i = i;
                }
            }
            (cvm, cvsd, grid[min_i], grid[se_i])
        }
        None => (Vec::new(), Vec::new(), grid[0], grid[0]),
    };

    let lambda_selected = match folds {
        Some(_) => match select {
            LambdaSelect::Min => lambda_min,
            LambdaSelect::OneSe => lambda_1se,
        },
        None => grid[0], // no CV evidence: strongest regularisation
    };
    let sel_i = grid.iter().position(|&l| l == lambda_selected).unwrap_or(0);
    let (b_orig, w_orig) = to_original(&full.coefs[sel_i], &std, k, p);

    let model = MultinomialEnetModel {
        class_labels: class_labels.to_vec(),
        intercept: b_orig,
        weights: w_orig,
        x_mean: std.mean,
        x_sd: std.sd,
        alpha: cfg.alpha,
        lambda_selected,
    };
    let curve = MnetCvPath {
        lambda: grid,
        cv_deviance,
        cv_se,
        nonzero_features,
        lambda_min,
        lambda_1se,
    };

    let mut coef_table = Vec::new();
    for c in 0..k {
        for j in 0..p {
            if model.weights[c][j] != 0.0 {
                coef_table.push(CoefEntry {
                    class_index: c,
                    class_label: class_labels[c].clone(),
                    feature_index: j,
                    feature_name: feature_names[j].clone(),
                    coef: model.weights[c][j],
                });
            }
        }
    }

    let _ = full.iters; // diagnostics available for training_meta
    Ok((model, curve, coef_table))
}

/// Predict classes and probabilities for new samples.
pub fn mnet_predict(model: &MultinomialEnetModel, x: &Mat<f64>) -> Result<ClassProbs> {
    let k = model.class_labels.len();
    let p = model.x_mean.len();
    if x.nrows() == 0 {
        return Err(MultinomialError::Empty);
    }
    if x.ncols() != p {
        return Err(MultinomialError::Other(format!(
            "model has {p} features, data has {}",
            x.ncols()
        )));
    }
    if !x.col_iter().all(|col| col.is_all_finite()) {
        return Err(MultinomialError::NonFinite);
    }
    let mut predictions = Vec::with_capacity(x.nrows());
    let mut probabilities = Vec::with_capacity(x.nrows());
    for i in 0..x.nrows() {
        let mut row = vec![0.0; k];
        for c in 0..k {
            let mut s = model.intercept[c];
            for j in 0..p {
                s += model.weights[c][j] * x[(i, j)];
            }
            row[c] = s;
        }
        let mx = row.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let exps: Vec<f64> = row.iter().map(|&v| (v - mx).exp()).collect();
        let sum: f64 = exps.iter().sum();
        let probs: Vec<f64> = exps.iter().map(|e| e / sum).collect();
        let mut best = 0;
        for (c, &pr) in probs.iter().enumerate() {
            if pr > probs[best] {
                best = c;
            }
        }
        predictions.push(best);
        probabilities.push(probs);
    }
    Ok(ClassProbs {
        predictions,
        probabilities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelArtifact;

    fn mat(rows: &[Vec<f64>]) -> Mat<f64> {
        Mat::from_fn(rows.len(), rows[0].len(), |i, j| rows[i][j])
    }

    fn toy_data() -> (Mat<f64>, Vec<usize>, Vec<String>, Vec<String>) {
        // 3 classes × 8 samples; first three features informative
        let mut rows = Vec::new();
        let mut y = Vec::new();
        for c in 0..3usize {
            for r in 0..8 {
                let base = (c as f64 - 1.0) * 2.5 + (r as f64 - 3.5) * 0.1;
                rows.push(vec![base, base * 0.8, -base * 0.5, 0.3, -0.1]);
                y.push(c);
            }
        }
        let labels: Vec<String> = (0..3).map(|c| format!("class_{c}")).collect();
        let feats: Vec<String> = (0..5).map(|j| format!("f{j}")).collect();
        (mat(&rows), y, labels, feats)
    }

    fn tight() -> MnetConfig {
        MnetConfig {
            n_lambda: 12,
            max_iter: 5000,
            tol: 1e-8,
            ..MnetConfig::default()
        }
    }

    #[test]
    fn test_fit_predict_separable() {
        let (x, y, labels, feats) = toy_data();
        let folds = crate::split::stratified_kfold(&y, 3, true, 42).unwrap();
        let (model, curve, coefs) = mnet_fit(
            &x,
            &y,
            &labels,
            &feats,
            &tight(),
            Some(&folds),
            LambdaSelect::Min,
        )
        .unwrap();
        let out = mnet_predict(&model, &x).unwrap();
        assert_eq!(out.predictions, y);
        for (i, row) in out.probabilities.iter().enumerate() {
            assert!((row.iter().sum::<f64>() - 1.0).abs() < 1e-12, "row {i}");
            assert!(row[y[i]] > 0.9, "row {i}: {row:?}");
        }
        assert!(!coefs.is_empty());
        // informative features selected, noise features not
        let sel: std::collections::BTreeSet<usize> =
            coefs.iter().map(|e| e.feature_index).collect();
        assert!(sel.contains(&0) && sel.contains(&1) && sel.contains(&2));
        assert!(!sel.contains(&3) && !sel.contains(&4));
        assert_eq!(curve.lambda.len(), curve.nonzero_features.len());
        // descending grid, monotone nonzero count
        assert!(curve.lambda.windows(2).all(|w| w[0] > w[1]));
        assert!(curve.nonzero_features.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(curve.nonzero_features[0], 0); // λ_max: intercept only
        // λ.min within grid
        assert!(curve.lambda.contains(&curve.lambda_min));
    }

    #[test]
    fn test_lambda_max_intercept_only() {
        let (x, y, labels, feats) = toy_data();
        // single-point grid exactly at λ_max → all weights zero
        let lmax = {
            let std = standardize(&x);
            lambda_max(&std.z, &y, &vec![1.0; x.nrows()], 3, 1.0)
        };
        let cfg = MnetConfig {
            lambda_grid: Some(vec![lmax, lmax * 0.999]),
            ..tight()
        };
        let (model, _, coefs) =
            mnet_fit(&x, &y, &labels, &feats, &cfg, None, LambdaSelect::Min).unwrap();
        assert!(coefs.is_empty());
        assert!(model.weights.iter().all(|r| r.iter().all(|&v| v == 0.0)));
        let out = mnet_predict(&model, &x).unwrap();
        // uniform priors → all rows predicted class 0 with p = 1/3
        assert!(out.predictions.iter().all(|&c| c == 0));
        assert!(
            out.probabilities[0]
                .iter()
                .all(|&v| (v - 1.0 / 3.0).abs() < 1e-9)
        );
    }

    #[test]
    fn test_warm_start_matches_cold_start() {
        let (x, y, _labels, _feats) = toy_data();
        let grid: Vec<f64> = (0..8)
            .map(|i| (0.2f64).ln() + i as f64 * ((0.005f64).ln() - 0.2f64.ln()) / 7.0)
            .map(|v| v.exp())
            .collect();
        let zero = ZParams {
            b: vec![0.0; 3],
            w: vec![0.0; 15],
        };
        let std = standardize(&x);
        let sw = vec![1.0; x.nrows()];
        // warm-started path solution at grid[4]
        let mut f2 = Fista::new(&std.z, &y, &sw, 3, 5000, 1e-8);
        let path = f2.path(&grid, 1.0, 1.0).unwrap();
        // cold start directly at grid[4]
        let mut f3 = Fista::new(&std.z, &y, &sw, 3, 5000, 1e-8);
        let mut lip3 = 1.0;
        let (cold, _) = f3.solve(grid[4], 1.0, &zero, &mut lip3).unwrap();
        let warm = &path.coefs[4];
        // both converge to the same optimum: close objective values
        let mut g = Fista::new(&std.z, &y, &sw, 3, 5000, 1e-8);
        let mut obj = |p: &ZParams| {
            g.smooth_obj(&p.b, &p.w, grid[4], 1.0)
                + grid[4] * p.w.iter().map(|v| v.abs()).sum::<f64>()
        };
        let ow = obj(warm);
        let oc = obj(&cold);
        assert!((ow - oc).abs() < 1e-8, "warm {ow} vs cold {oc}");
    }

    #[test]
    fn test_ridge_alpha_zero() {
        let (x, y, labels, feats) = toy_data();
        let folds = crate::split::stratified_kfold(&y, 3, true, 7).unwrap();
        let cfg = MnetConfig {
            alpha: 0.0,
            max_iter: 5000,
            ..tight()
        };
        let (model, curve, _) = mnet_fit(
            &x,
            &y,
            &labels,
            &feats,
            &cfg,
            Some(&folds),
            LambdaSelect::Min,
        )
        .unwrap();
        assert!(curve.lambda.iter().all(|l| l.is_finite() && *l > 0.0));
        let out = mnet_predict(&model, &x).unwrap();
        assert_eq!(out.predictions, y);
    }

    #[test]
    fn test_balanced_weights_help_imbalanced_data() {
        // overlapping 3-class data with a 24/6/6 imbalance: balanced
        // weights must lift the mean probability assigned to minority rows
        let mut rows = Vec::new();
        let mut y = Vec::new();
        for c in 0..3usize {
            let reps = if c == 0 { 24 } else { 6 };
            for r in 0..reps {
                let base = (c as f64 - 1.0) * 0.9 + (r as f64 % 6.0 - 2.5) * 0.55;
                rows.push(vec![base, base * 0.8]);
                y.push(c);
            }
        }
        let x = mat(&rows);
        let labels: Vec<String> = (0..3).map(|c| format!("class_{c}")).collect();
        let feats = vec!["f0".to_string(), "f1".to_string()];
        let folds = crate::split::stratified_kfold(&y, 3, true, 3).unwrap();
        let cfg_u = tight();
        let cfg_b = MnetConfig {
            class_weight: ClassWeight::Balanced,
            ..tight()
        };
        let (m_u, _, _) = mnet_fit(
            &x,
            &y,
            &labels,
            &feats,
            &cfg_u,
            Some(&folds),
            LambdaSelect::Min,
        )
        .unwrap();
        let (m_b, _, _) = mnet_fit(
            &x,
            &y,
            &labels,
            &feats,
            &cfg_b,
            Some(&folds),
            LambdaSelect::Min,
        )
        .unwrap();
        let out_u = mnet_predict(&m_u, &x).unwrap();
        let out_b = mnet_predict(&m_b, &x).unwrap();
        // mean p(true class) over the minority rows, balanced vs uniform
        let mean_true = |o: &ClassProbs| {
            let idx: Vec<usize> = y
                .iter()
                .enumerate()
                .filter(|&(_, &c)| c > 0)
                .map(|(i, _)| i)
                .collect();
            idx.iter().map(|&i| o.probabilities[i][y[i]]).sum::<f64>() / idx.len() as f64
        };
        assert!(
            mean_true(&out_b) > mean_true(&out_u),
            "balanced {} vs uniform {}",
            mean_true(&out_b),
            mean_true(&out_u)
        );
    }

    #[test]
    fn test_validation_errors() {
        let (x, y, labels, feats) = toy_data();
        let call = |cfg: &MnetConfig, y: &[usize]| {
            mnet_fit(&x, y, &labels, &feats, cfg, None, LambdaSelect::Min)
        };
        assert!(call(&tight(), &y[..5]).is_err()); // label mismatch
        assert!(
            call(
                &MnetConfig {
                    alpha: 1.5,
                    ..tight()
                },
                &y
            )
            .is_err()
        );
        assert!(
            call(
                &MnetConfig {
                    lambda_grid: Some(vec![0.1, 0.2]),
                    ..tight()
                },
                &y
            )
            .is_err()
        ); // ascending
        assert!(
            call(
                &MnetConfig {
                    lambda_grid: Some(vec![0.1, -0.2]),
                    ..tight()
                },
                &y
            )
            .is_err()
        );
        assert!(
            call(
                &MnetConfig {
                    n_lambda: 1,
                    lambda_grid: None,
                    ..tight()
                },
                &y
            )
            .is_err()
        );
        assert!(
            call(
                &MnetConfig {
                    tol: 0.0,
                    ..tight()
                },
                &y
            )
            .is_err()
        );
        // bad labels
        let bad_y: Vec<usize> = y.iter().map(|&c| c + 5).collect();
        assert!(call(&tight(), &bad_y).is_err());
        // one class label list
        let one_label = vec!["a".to_string()];
        assert!(
            mnet_fit(
                &x,
                &y,
                &one_label,
                &feats,
                &tight(),
                None,
                LambdaSelect::Min
            )
            .is_err()
        );
        // non-finite
        let mut bad_rows = vec![vec![0.0; 5]; 6];
        bad_rows.push(vec![f64::NAN; 5]);
        let bad_x = mat(&bad_rows);
        let bad_y2 = vec![0, 1, 0, 1, 0, 1, 0];
        assert!(
            mnet_fit(
                &bad_x,
                &bad_y2,
                &labels,
                &feats,
                &tight(),
                None,
                LambdaSelect::Min
            )
            .is_err()
        );
        // wrong-width predict
        let (model, _, _) =
            mnet_fit(&x, &y, &labels, &feats, &tight(), None, LambdaSelect::Min).unwrap();
        let bad = Mat::zeros(2, 3);
        assert!(mnet_predict(&model, &bad).is_err());
    }

    #[test]
    fn test_artifact_roundtrip() {
        let (x, y, labels, feats) = toy_data();
        let (model, _, _) =
            mnet_fit(&x, &y, &labels, &feats, &tight(), None, LambdaSelect::Min).unwrap();
        let artifact = ModelArtifact::new(
            MULTINOMIAL_ENET_KIND,
            &model,
            feats.clone(),
            serde_json::json!({"lambda": model.lambda_selected, "alpha": model.alpha}),
        )
        .unwrap();
        let bytes = artifact.to_bytes().unwrap();
        let back = ModelArtifact::from_bytes(&bytes).unwrap();
        assert_eq!(back.kind, MULTINOMIAL_ENET_KIND);
        let restored: MultinomialEnetModel = back.deserialize_fitted().unwrap();
        assert_eq!(restored.class_labels, model.class_labels);
        let o1 = mnet_predict(&model, &x).unwrap();
        let o2 = mnet_predict(&restored, &x).unwrap();
        assert_eq!(o1.predictions, o2.predictions);
        for (a, b) in o1.probabilities.iter().zip(&o2.probabilities) {
            assert!(a.iter().zip(b).all(|(u, v)| (u - v).abs() < 1e-12));
        }
    }

    #[test]
    fn test_backtransform_predict_consistency() {
        // z-space solution must predict identically to its original-scale
        // back-transform — the invariant glmnet's coef reporting relies on
        let (x, y, labels, _feats) = toy_data();
        let std = standardize(&x);
        let sw = vec![1.0; x.nrows()];
        let mut f = Fista::new(&std.z, &y, &sw, 3, 5000, 1e-9);
        let mut lip = 1.0;
        let (sol, _) = f
            .solve(
                0.05,
                1.0,
                &ZParams {
                    b: vec![0.0; 3],
                    w: vec![0.0; 15],
                },
                &mut lip,
            )
            .unwrap();
        let (b_orig, w_orig) = to_original(&sol, &std, 3, 5);
        let model = MultinomialEnetModel {
            class_labels: labels,
            intercept: b_orig,
            weights: w_orig,
            x_mean: std.mean.clone(),
            x_sd: std.sd.clone(),
            alpha: 1.0,
            lambda_selected: 0.05,
        };
        let out = mnet_predict(&model, &x).unwrap();
        for (i, row) in out.probabilities.iter().enumerate() {
            // recompute in z space
            let mut zrow = [0.0; 3];
            for c in 0..3 {
                let mut s = sol.b[c];
                for j in 0..5 {
                    s += sol.w[c * 5 + j] * ((x[(i, j)] - std.mean[j]) / std.sd[j]);
                }
                zrow[c] = s;
            }
            let mx = zrow.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let exps: Vec<f64> = zrow.iter().map(|&v| (v - mx).exp()).collect();
            let sum: f64 = exps.iter().sum();
            for c in 0..3 {
                assert!((row[c] - exps[c] / sum).abs() < 1e-10);
            }
        }
    }
}
