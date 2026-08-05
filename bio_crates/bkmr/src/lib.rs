//! Rust port of the R package [`bkmr`](https://github.com/jenfb/bkmr)
//! (Bobb et al. 2015, *Biostatistics*) — Bayesian Kernel Machine Regression.
//!
//! BKMR models an outcome `y` as `g(E[y]) = β'X + h(Z)`, where `h(·)` is a
//! flexible non-parametric function of a mixture of exposures `Z`, realised
//! as a Gaussian-process realisation with kernel `K(Z_i, Z_j; r, λ) =
//! λ·exp(-Σ_m r_m·(Z_im - Z_jm)²)`. Variable selection places a spike-and-slab
//! prior on each exposure via binary inclusion indicators `δ_m`.
//!
//! Inference is by Gibbs / Metropolis–Hastings MCMC. This crate reproduces the
//! R package's algorithm faithfully and reproduces R's RNG stream exactly
//! under a shared `set.seed`, so MCMC chains can be cross-validated against
//! the reference implementation. Linear-algebra uses `faer`, whose Cholesky
//! agrees with R's LAPACK to ~1e-15; transcendental functions (`lgamma`,
//! `pnorm`) use `statrs` and may differ from Rmath by ~1e-12, so over long
//! MCMC chains an acceptance decision may rarely flip — posterior summaries
//! agree to Monte-Carlo error.
//!
//! # Scope
//!
//! This port covers the **gaussian** family with component-wise variable
//! selection (varsel) and no random intercept / knots / predictive process.
//! The binomial family, grouped variable selection, the random intercept,
//! and the Gaussian predictive process are out of scope.
//!
//! # References
//!
//! - Bobb, JF, Valeri L, Claus Henn B, Christiani DC, Wright RO, Mazumdar M,
//!   Godleski JJ, Coull BA (2015). *Bayesian Kernel Machine Regression for
//!   Estimating the Health Effects of Multi-Pollutant Mixtures.*
//!   Biostatistics 16(3): 493–508.

pub mod rng;
pub use rng::Rng;

use faer::{Mat, Side};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BkmrError {
    #[error("invalid input: {0}")]
    BadInput(String),
    #[error("linear algebra failed: {0}")]
    Linalg(String),
}
pub type Result<T> = std::result::Result<T, BkmrError>;

// =====================================================================
// Types
// =====================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RPrior {
    Gamma,
    Invunif,
    Unif,
}
impl Default for RPrior {
    fn default() -> Self {
        RPrior::Invunif
    }
}
impl std::str::FromStr for RPrior {
    type Err = BkmrError;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "gamma" => Ok(RPrior::Gamma),
            "invunif" => Ok(RPrior::Invunif),
            "unif" => Ok(RPrior::Unif),
            other => Err(BkmrError::BadInput(format!("unknown r.prior '{other}'"))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RMethod {
    Varying,
    Equal,
    Fixed,
}
impl Default for RMethod {
    fn default() -> Self {
        RMethod::Varying
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlParams {
    pub lambda_jump: Vec<f64>,
    pub mu_lambda: Vec<f64>,
    pub sigma_lambda: Vec<f64>,
    pub a_p0: f64,
    pub b_p0: f64,
    pub r_prior: RPrior,
    pub a_sigsq: f64,
    pub b_sigsq: f64,
    pub mu_r: f64,
    pub sigma_r: f64,
    pub r_muprop: f64,
    pub r_jump: f64,
    pub r_jump1: f64,
    pub r_jump2: f64,
    pub r_a: f64,
    pub r_b: f64,
}
impl Default for ControlParams {
    fn default() -> Self {
        ControlParams {
            lambda_jump: vec![10.0],
            mu_lambda: vec![10.0],
            sigma_lambda: vec![10.0],
            a_p0: 1.0,
            b_p0: 1.0,
            r_prior: RPrior::Invunif,
            a_sigsq: 1e-3,
            b_sigsq: 1e-3,
            mu_r: 5.0,
            sigma_r: 5.0,
            r_muprop: 1.0,
            r_jump: 0.1,
            r_jump1: 2.0,
            r_jump2: 0.1,
            r_a: 0.0,
            r_b: 100.0,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StartingValues {
    pub h_hat: Option<Vec<f64>>,
    pub beta: Option<Vec<f64>>,
    pub sigsq_eps: Option<f64>,
    pub r: Option<Vec<f64>>,
    pub lambda: Option<Vec<f64>>,
    pub delta: Option<Vec<f64>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KmbayesOptions {
    pub iter: usize,
    pub varsel: bool,
    pub rmethod: RMethod,
    pub r_prior: RPrior,
    pub starting_values: StartingValues,
    pub control_params: ControlParams,
    /// 1-based indices of Z columns for variable selection.
    pub ztest: Option<Vec<usize>>,
}
impl Default for KmbayesOptions {
    fn default() -> Self {
        KmbayesOptions {
            iter: 1000,
            varsel: false,
            rmethod: RMethod::Varying,
            r_prior: RPrior::Invunif,
            starting_values: StartingValues::default(),
            control_params: ControlParams::default(),
            ztest: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BkmrFit {
    pub h_hat: Mat<f64>,
    pub beta: Mat<f64>,
    pub lambda: Mat<f64>,
    pub sigsq_eps: Vec<f64>,
    pub r: Mat<f64>,
    pub delta: Mat<f64>,
    pub acc_r: Mat<f64>,
    pub acc_lambda: Mat<f64>,
    pub acc_rdelta: Vec<f64>,
    pub move_type: Vec<f64>,
    pub iter: usize,
    pub varsel: bool,
    pub ztest: Vec<usize>,
    pub control_params: ControlParams,
}

// =====================================================================
// Special functions
// =====================================================================

const LN_SQRT_2PI: f64 = 0.91893853320467274178032973640561763986139747363778;

fn lgamma_fn(x: f64) -> f64 {
    statrs::function::gamma::ln_gamma(x)
}
fn erf(x: f64) -> f64 {
    statrs::function::erf::erf(x)
}
fn pnorm_std(x: f64) -> f64 {
    0.5 * (1.0 + erf(x / std::f64::consts::SQRT_2))
}
fn dnorm_std_log(z: f64) -> f64 {
    -0.5 * z * z - LN_SQRT_2PI
}

/// `dgamma(x, shape, rate, log=TRUE)`.
fn dgamma_log(x: f64, shape: f64, rate: f64) -> f64 {
    if x <= 0.0 {
        return f64::NEG_INFINITY;
    }
    shape * rate.ln() - lgamma_fn(shape) + (shape - 1.0) * x.ln() - rate * x
}

/// `truncnorm::dtruncnorm(x, a, b, mean, sd, log=TRUE)`.
fn dtruncnorm_log(x: f64, a: f64, b: f64, mean: f64, sd: f64) -> f64 {
    if x < a || x > b {
        return f64::NEG_INFINITY;
    }
    let z = (x - mean) / sd;
    let phi_x = dnorm_std_log(z) - sd.ln();
    let c1 = pnorm_std((a - mean) / sd);
    let c2 = pnorm_std((b - mean) / sd);
    phi_x - (sd * (c2 - c1)).ln()
}

// =====================================================================
// Linear algebra
// =====================================================================

type Mref<'a> = faer::MatRef<'a, f64>;

/// R's `chol(V)` → upper triangular U with `V = U' U`.
fn chol_upper(v: Mref) -> Result<Mat<f64>> {
    use faer::linalg::solvers::Llt;
    let llt = Llt::new(v, Side::Lower).map_err(|e| BkmrError::Linalg(e.to_string()))?;
    let l = llt.L();
    let n = l.nrows();
    let mut u = Mat::zeros(n, n);
    for i in 0..n {
        for j in i..n {
            u[(i, j)] = l[(j, i)];
        }
    }
    Ok(u)
}

/// R's `chol2inv(U)` → `(U' U)⁻¹` from upper Cholesky factor.
fn chol2inv(u: Mref) -> Mat<f64> {
    use faer::linalg::solvers::{Llt, Solve};
    let n = u.nrows();
    let mut v = Mat::zeros(n, n);
    for i in 0..n {
        for j in i..n {
            let mut s = 0.0;
            for k in 0..=i {
                s += u[(k, i)] * u[(k, j)];
            }
            v[(i, j)] = s;
            v[(j, i)] = s;
        }
    }
    let llt = Llt::new(v.as_ref(), Side::Lower).expect("chol2inv: not PD");
    let ident = Mat::identity(n, n);
    llt.solve(ident.as_ref())
}

// =====================================================================
// Kernel / Vcomps
// =====================================================================

fn make_kpart(r: &[f64], z1: Mref, z2: Option<Mref>) -> Mat<f64> {
    let n1 = z1.nrows();
    let m = z1.ncols();
    let z2r = z2.unwrap_or(z1);
    let n2 = z2r.nrows();
    let sr: Vec<f64> = r.iter().map(|x| x.sqrt()).collect();
    // scaled Z
    let scaled = |z: Mref, n: usize| -> Mat<f64> {
        let mut s = Mat::zeros(n, m);
        for j in 0..m {
            for i in 0..n {
                s[(i, j)] = z[(i, j)] * sr[j];
            }
        }
        s
    };
    let z1s = scaled(z1, n1);
    let z2s = scaled(z2r, n2);
    let mut k = Mat::zeros(n1, n2);
    for i in 0..n1 {
        for j in 0..n2 {
            let mut s = 0.0;
            for t in 0..m {
                let d = z1s[(i, t)] - z2s[(j, t)];
                s += d * d;
            }
            k[(i, j)] = s;
        }
    }
    k
}

#[derive(Clone)]
struct VComps {
    vinv: Mat<f64>,
    logdet_vinv: f64,
}

fn make_vcomps(r: &[f64], lambda: &[f64], z: Mref) -> Result<VComps> {
    let n = z.nrows();
    let kpart = make_kpart(r, z, None);
    let mut v = Mat::zeros(n, n);
    for i in 0..n {
        for j in 0..n {
            let base = if i == j { 1.0 } else { 0.0 };
            v[(i, j)] = base + lambda[0] * (-kpart[(i, j)]).exp();
        }
    }
    let cu = chol_upper(v.as_ref())?;
    let mut ld = 0.0;
    for i in 0..n {
        ld += cu[(i, i)].ln();
    }
    let vinv = chol2inv(cu.as_ref());
    Ok(VComps {
        vinv,
        logdet_vinv: -2.0 * ld,
    })
}

// =====================================================================
// r proposal / prior
// =====================================================================

#[derive(Clone, Copy)]
struct Rp {
    mu_r: f64,
    sigma_r: f64,
    r_muprop: f64,
    r_jump: f64,
    r_jump1: f64,
    r_jump2: f64,
    r_a: f64,
    r_b: f64,
}
fn rp(p: &ControlParams) -> Rp {
    Rp {
        mu_r: p.mu_r,
        sigma_r: p.sigma_r,
        r_muprop: p.r_muprop,
        r_jump: p.r_jump,
        r_jump1: p.r_jump1,
        r_jump2: p.r_jump2,
        r_a: p.r_a,
        r_b: p.r_b,
    }
}

fn rprior_logdens(x: f64, p: Rp, prior: RPrior) -> f64 {
    match prior {
        RPrior::Gamma => dgamma_log(
            x,
            p.mu_r * p.mu_r / (p.sigma_r * p.sigma_r),
            p.mu_r / (p.sigma_r * p.sigma_r),
        ),
        RPrior::Invunif => {
            if 1.0 / p.r_b <= x && x <= 1.0 / p.r_a {
                -2.0 * x.ln() - (p.r_b - p.r_a).ln()
            } else {
                f64::NEG_INFINITY
            }
        }
        RPrior::Unif => {
            if p.r_a <= x && x <= p.r_b {
                -(p.r_b - p.r_a).ln()
            } else {
                f64::NEG_INFINITY
            }
        }
    }
}

fn rprop_gen1(rng: &mut Rng, p: Rp, prior: RPrior) -> f64 {
    match prior {
        RPrior::Gamma => {
            let sh = p.r_muprop * p.r_muprop / (p.r_jump1 * p.r_jump1);
            let sc = p.r_muprop / (p.r_muprop * p.r_muprop / (p.r_jump1 * p.r_jump1));
            rng.rgamma(sh, sc)
        }
        RPrior::Invunif => 1.0 / rng.runif_range(p.r_a, p.r_b),
        RPrior::Unif => rng.runif_range(p.r_a, p.r_b),
    }
}

fn rprop_logdens1(x: f64, p: Rp, prior: RPrior) -> f64 {
    match prior {
        RPrior::Gamma => {
            let sh = p.r_muprop * p.r_muprop / (p.r_jump1 * p.r_jump1);
            let rate = p.r_muprop / (p.r_jump1 * p.r_jump1);
            dgamma_log(x, sh, rate)
        }
        RPrior::Invunif => {
            if 1.0 / p.r_b <= x && x <= 1.0 / p.r_a {
                -2.0 * x.ln() - (p.r_b - p.r_a).ln()
            } else {
                f64::NEG_INFINITY
            }
        }
        RPrior::Unif => {
            if p.r_a <= x && x <= p.r_b {
                -(p.r_b - p.r_a).ln()
            } else {
                f64::NEG_INFINITY
            }
        }
    }
}

fn rprop_gen2(rng: &mut Rng, current: f64, p: Rp, prior: RPrior) -> f64 {
    match prior {
        RPrior::Gamma => {
            let sh = current * current / (p.r_jump2 * p.r_jump2);
            let sc = p.r_jump2 * p.r_jump2 / current;
            rng.rgamma(sh, sc)
        }
        RPrior::Invunif => rng.rtruncnorm(1.0 / p.r_b, 1.0 / p.r_a, current, p.r_jump2),
        RPrior::Unif => rng.rtruncnorm(p.r_a, p.r_b, current, p.r_jump2),
    }
}

fn rprop_logdens2(prop: f64, current: f64, p: Rp, prior: RPrior) -> f64 {
    match prior {
        RPrior::Gamma => {
            let sh = current * current / (p.r_jump2 * p.r_jump2);
            let rate = current / (p.r_jump2 * p.r_jump2);
            dgamma_log(prop, sh, rate)
        }
        RPrior::Invunif => dtruncnorm_log(prop, 1.0 / p.r_b, 1.0 / p.r_a, current, p.r_jump2),
        RPrior::Unif => dtruncnorm_log(prop, p.r_a, p.r_b, current, p.r_jump2),
    }
}

fn rprop_gen(rng: &mut Rng, current: f64, p: Rp, prior: RPrior) -> f64 {
    match prior {
        RPrior::Gamma => {
            let sh = current * current / (p.r_jump * p.r_jump);
            let sc = p.r_jump * p.r_jump / current;
            rng.rgamma(sh, sc)
        }
        RPrior::Invunif => rng.rtruncnorm(1.0 / p.r_b, 1.0 / p.r_a, current, p.r_jump),
        RPrior::Unif => rng.rtruncnorm(p.r_a, p.r_b, current, p.r_jump),
    }
}

fn rprop_logdens(prop: f64, current: f64, p: Rp, prior: RPrior) -> f64 {
    match prior {
        RPrior::Gamma => {
            let sh = current * current / (p.r_jump * p.r_jump);
            let rate = current / (p.r_jump * p.r_jump);
            dgamma_log(prop, sh, rate)
        }
        RPrior::Invunif => dtruncnorm_log(prop, 1.0 / p.r_b, 1.0 / p.r_a, current, p.r_jump),
        RPrior::Unif => dtruncnorm_log(prop, p.r_a, p.r_b, current, p.r_jump),
    }
}

// =====================================================================
// MCMC updates
// =====================================================================

/// `beta.update(X, Vinv, y, sigsq.eps)` — conjugate Gaussian update.
fn beta_update(rng: &mut Rng, x: Mref, vinv: Mref, y: &[f64], sigsq: f64) -> Vec<f64> {
    let n = x.nrows();
    let k = x.ncols();
    // XVinv = X' Vinv  (k×n)
    let mut xvinv = vec![vec![0.0f64; n]; k];
    for a in 0..k {
        for b in 0..n {
            let mut s = 0.0;
            for t in 0..n {
                s += x[(t, a)] * vinv[(t, b)];
            }
            xvinv[a][b] = s;
        }
    }
    // XvX = XVinv · X  (k×k)
    let mut xvx = Mat::zeros(k, k);
    for a in 0..k {
        for b in 0..k {
            let mut s = 0.0;
            for t in 0..n {
                s += xvinv[a][t] * x[(t, b)];
            }
            xvx[(a, b)] = s;
        }
    }
    // Vbeta = (XvX)⁻¹  (this is the covariance)
    let cu_xvx = chol_upper(xvx.as_ref()).expect("XvX not PD");
    let vbeta = chol2inv(cu_xvx.as_ref());
    // XVy = XVinv · y
    let mut xvy = vec![0.0f64; k];
    for a in 0..k {
        let mut s = 0.0;
        for t in 0..n {
            s += xvinv[a][t] * y[t];
        }
        xvy[a] = s;
    }
    // betahat = Vbeta · XVy
    let mut betahat = vec![0.0f64; k];
    for a in 0..k {
        let mut s = 0.0;
        for b in 0..k {
            s += vbeta[(a, b)] * xvy[b];
        }
        betahat[a] = s;
    }
    // chol(Vbeta)
    let cu_vbeta = chol_upper(vbeta.as_ref()).expect("Vbeta not PD");
    // n01 <- rnorm(ncol(X))
    let n01: Vec<f64> = (0..k).map(|_| rng.rnorm(0.0, 1.0)).collect();
    // betahat + crossprod(sqrt(sigsq)*cholVbeta, n01)
    // crossprod(A, v) = A' v; result[a] = betahat[a] + sum_b sqrt(sigsq)*A[b,a]*n01[b]
    let mut beta = vec![0.0f64; k];
    for a in 0..k {
        let mut s = betahat[a];
        for b in 0..=a {
            // cu_vbeta is upper triangular: cu_vbeta[(b,a)] nonzero only for b <= a
            s += sigsq.sqrt() * cu_vbeta[(b, a)] * n01[b];
        }
        beta[a] = s;
    }
    beta
}

/// `sigsq.eps.update(y, X, beta, Vinv, a.eps, b.eps)`.
fn sigsq_eps_update(
    rng: &mut Rng,
    y: &[f64],
    x: Mref,
    beta: &[f64],
    vinv: Mref,
    a_eps: f64,
    b_eps: f64,
) -> f64 {
    let n = x.nrows();
    // mu = y - X %*% beta
    let mut mu = vec![0.0f64; n];
    for i in 0..n {
        let mut s = y[i];
        for j in 0..x.ncols() {
            s -= x[(i, j)] * beta[j];
        }
        mu[i] = s;
    }
    // quad = crossprod(mu, Vinv) %*% mu
    let mut vinv_mu = vec![0.0f64; n];
    for i in 0..n {
        let mut s = 0.0;
        for j in 0..n {
            s += vinv[(i, j)] * mu[j];
        }
        vinv_mu[i] = s;
    }
    let mut quad = 0.0;
    for i in 0..n {
        quad += mu[i] * vinv_mu[i];
    }
    let shape = a_eps + n as f64 / 2.0;
    let rate = b_eps + 0.5 * quad;
    // prec.y <- rgamma(1, shape, rate); 1/prec.y
    // rgamma(shape, scale=1/rate)
    let prec = rng.rgamma(shape, 1.0 / rate);
    1.0 / prec
}

/// `lamAdj(lam)` — proposal mean correction for very small lambda.
fn lam_adj(lam: f64) -> f64 {
    if lam <= 2.0 {
        3.0
    } else {
        lam * lam / (lam - 1.0) - 1.0
    }
}

/// Result of a Metropolis–Hastings step for (r, lambda, delta).
struct MhResult {
    r: Vec<f64>,
    lambda: Vec<f64>,
    delta: Vec<f64>,
    acc: bool,
    vcomps: VComps,
    move_type: f64,
}

/// `MHstep(...)` — core Metropolis–Hastings acceptance step. Consumes one
/// `runif` for the acceptance test. Returns the updated state (accepted or
/// not) and the new VComps.
fn mh_step_rng(
    rng: &mut Rng,
    r: &[f64],
    lambda: &[f64],
    lambda_star: &[f64],
    r_star: &[f64],
    delta: &[f64],
    delta_star: &[f64],
    y: &[f64],
    x: Mref,
    z: Mref,
    beta: &[f64],
    sigsq: f64,
    diffpriors: f64,
    negdifflogproposal: f64,
    vcomps: &VComps,
    move_type: f64,
) -> Result<MhResult> {
    let vcomps_star = make_vcomps(r_star, lambda_star, z)?;
    let n = x.nrows();
    let mut mu = vec![0.0f64; n];
    for i in 0..n {
        let mut s = y[i];
        for j in 0..x.ncols() {
            s -= x[(i, j)] * beta[j];
        }
        mu[i] = s;
    }
    let mut tmp = vec![0.0f64; n];
    for i in 0..n {
        let mut s = 0.0;
        for j in 0..n {
            s += (vcomps_star.vinv[(i, j)] - vcomps.vinv[(i, j)]) * mu[j];
        }
        tmp[i] = s;
    }
    let mut quad = 0.0;
    for i in 0..n {
        quad += mu[i] * tmp[i];
    }
    let diffliks = 0.5 * vcomps_star.logdet_vinv - 0.5 * vcomps.logdet_vinv - 0.5 / sigsq * quad;
    let log_mh_ratio = diffliks + diffpriors + negdifflogproposal;
    let log_alpha = log_mh_ratio.min(0.0);
    let acc = rng.runif().ln() <= log_alpha;
    if acc {
        Ok(MhResult {
            r: r_star.to_vec(),
            lambda: lambda_star.to_vec(),
            delta: delta_star.to_vec(),
            acc: true,
            vcomps: vcomps_star,
            move_type,
        })
    } else {
        Ok(MhResult {
            r: r.to_vec(),
            lambda: lambda.to_vec(),
            delta: delta.to_vec(),
            acc: false,
            vcomps: vcomps.clone(),
            move_type,
        })
    }
}

// =====================================================================
// SimData
// =====================================================================

/// One of the three true h(z) functions used by [`sim_data`].
fn hfun_calc(z: &[f64], ind: &[usize], which: usize) -> f64 {
    match which {
        1 => 4.0 * plogis(z[ind[0] - 1], 0.0, 0.3),
        2 => 0.25 * (z[ind[0] - 1] + z[ind[1] - 1] + 0.5 * z[ind[0] - 1] * z[ind[1] - 1]),
        3 => {
            4.0 * plogis(
                0.25 * (z[ind[0] - 1] + z[ind[1] - 1] + 0.5 * z[ind[0] - 1] * z[ind[1] - 1]),
                0.0,
                0.3,
            )
        }
        _ => 0.0,
    }
}

/// R's `plogis(x, location, scale)` = logistic CDF.
fn plogis(x: f64, location: f64, scale: f64) -> f64 {
    let z = (x - location) / scale;
    1.0 / (1.0 + (-z).exp())
}

/// Simulated dataset — output of [`sim_data`].
#[derive(Clone, Debug)]
pub struct SimData {
    pub n: usize,
    pub m: usize,
    pub sigsq_true: f64,
    pub beta_true: f64,
    pub z: Mat<f64>,
    pub h: Vec<f64>,
    pub x: Mat<f64>,
    pub y: Vec<f64>,
    pub hfun: usize,
}

/// `SimData(n, M, sigsq.true, beta.true, hfun, Zgen, ind)` — faithful port
/// of the R package's data generator. Supports `Zgen ∈ {"norm", "unif"}`
/// (the `"corr"` and `"realistic"` modes use MASS::mvrnorm, omitted here).
pub fn sim_data(
    rng: &mut Rng,
    n: usize,
    m: usize,
    sigsq_true: f64,
    beta_true: f64,
    hfun: usize,
    zgen: &str,
    ind: &[usize],
) -> SimData {
    // Z
    let mut z = Mat::zeros(n, m);
    match zgen {
        "norm" => {
            // R: matrix(rnorm(n*M), n, M) fills column-major
            for j in 0..m {
                for i in 0..n {
                    z[(i, j)] = rng.rnorm(0.0, 1.0);
                }
            }
        }
        "unif" => {
            // R: matrix(runif(n*M, -2, 2), n, M) fills column-major
            for j in 0..m {
                for i in 0..n {
                    z[(i, j)] = rng.runif_range(-2.0, 2.0);
                }
            }
        }
        _ => panic!("Zgen='{zgen}' not supported; use 'norm' or 'unif'"),
    }
    // X = 3*cos(Z[,1]) + 2*rnorm(n)
    let mut x = Mat::zeros(n, 1);
    for i in 0..n {
        x[(i, 0)] = 3.0 * z[(i, 0)].cos() + 2.0 * rng.rnorm(0.0, 1.0);
    }
    // eps = rnorm(n, sd=sqrt(sigsq.true))
    // h = apply(Z, 1, HFun) — row-wise
    let h: Vec<f64> = (0..n)
        .map(|i| {
            let row: Vec<f64> = (0..m).map(|j| z[(i, j)]).collect();
            hfun_calc(&row, ind, hfun)
        })
        .collect();
    // y = X*beta + h + eps
    let mut y = vec![0.0f64; n];
    for i in 0..n {
        let eps = rng.rnorm(0.0, sigsq_true.sqrt());
        y[i] = x[(i, 0)] * beta_true + h[i] + eps;
    }
    SimData {
        n,
        m,
        sigsq_true,
        beta_true,
        z,
        h,
        x,
        y,
        hfun,
    }
}

// =====================================================================
// kmbayes — main MCMC
// =====================================================================

/// Fit BKMR via MCMC. Mirrors R's `kmbayes()` for the gaussian family,
/// component-wise (or no) variable selection, no random intercept / knots.
pub fn kmbayes(
    rng: &mut Rng,
    y: &[f64],
    z: Mref,
    x: Mref,
    opts: &KmbayesOptions,
) -> Result<BkmrFit> {
    let n = y.len();
    let m = z.ncols();
    let k = x.ncols();
    let nsamp = opts.iter;
    let varsel = opts.varsel;
    let cp = &opts.control_params;
    let prior = cp.r_prior;

    // ztest (1-based → 0-based internal)
    let ztest_1based: Vec<usize> = match (&opts.ztest, varsel) {
        (Some(v), _) => v.clone(),
        (None, true) => (1..=m).collect(),
        (None, false) => Vec::new(),
    };
    // forced-in components (those NOT in ztest), 1-based
    let forced: Vec<usize> = (1..=m).filter(|c| !ztest_1based.contains(c)).collect();

    // ── storage ──
    let mut beta = Mat::zeros(nsamp, k);
    let mut lambda = Mat::zeros(nsamp, 1);
    let mut sigsq_eps = vec![f64::NAN; nsamp];
    let mut r = Mat::zeros(nsamp, m);
    let mut delta = Mat::ones(nsamp, m);
    let mut acc_r = Mat::zeros(nsamp, m);
    let mut acc_lambda = Mat::zeros(nsamp, 1);
    let mut acc_rdelta = vec![0.0f64; nsamp];
    let mut move_type = vec![0.0f64; nsamp];
    let mut h_hat = Mat::zeros(nsamp, n);

    // ── initial values ──
    let sv = &opts.starting_values;
    let beta0: Vec<f64> = sv.beta.clone().unwrap_or_else(|| vec![0.0; k]);
    let sigsq0: f64 = sv.sigsq_eps.unwrap_or(0.5);
    let r0: Vec<f64> = sv.r.clone().unwrap_or_else(|| vec![1.0; m]);
    let lambda0: Vec<f64> = sv.lambda.clone().unwrap_or_else(|| vec![10.0]);
    let delta0: Vec<f64> = sv.delta.clone().unwrap_or_else(|| vec![1.0; m]);

    for j in 0..k.min(beta0.len()) {
        beta[(0, j)] = beta0[j];
    }
    lambda[(0, 0)] = lambda0[0];
    sigsq_eps[0] = sigsq0;
    for j in 0..m.min(r0.len()) {
        r[(0, j)] = r0[j];
    }
    if varsel {
        for (idx, &zt) in ztest_1based.iter().enumerate() {
            delta[(0, zt - 1)] = delta0.get(idx).copied().unwrap_or(1.0);
        }
    }
    // h_hat init
    let h0 = sv.h_hat.clone().unwrap_or_else(|| vec![1.0; n]);
    for i in 0..n.min(h0.len()) {
        h_hat[(0, i)] = h0[i];
    }

    let mut vcomps = make_vcomps(
        &(0..m).map(|j| r[(0, j)]).collect::<Vec<_>>(),
        &[lambda[(0, 0)]],
        z,
    )?;

    let rparams = rp(cp);

    // ── MCMC loop ──
    for s in 1..nsamp {
        let ycont: Vec<f64> = y.to_vec();

        // beta update
        let beta_prev: Vec<f64> = (0..k).map(|j| beta[(s - 1, j)]).collect();
        let beta_new = if k > 0 {
            beta_update(rng, x, vcomps.vinv.as_ref(), &ycont, sigsq_eps[s - 1])
        } else {
            beta_prev.clone()
        };
        for j in 0..k {
            beta[(s, j)] = beta_new[j];
        }

        // sigsq.eps update
        sigsq_eps[s] = sigsq_eps_update(
            rng,
            &ycont,
            x,
            &beta_new,
            vcomps.vinv.as_ref(),
            cp.a_sigsq,
            cp.b_sigsq,
        );

        // lambda update
        let mut lambda_sim: Vec<f64> = vec![lambda[(s - 1, 0)]];
        let lambda_jump = cp.lambda_jump[0];
        let mu_lambda = cp.mu_lambda[0];
        let sigma_lambda = cp.sigma_lambda[0];
        let lambdacomp = lambda_sim[0];
        let adj = lam_adj(lambdacomp);
        let lcs = rng.rgamma(
            adj * adj / (lambda_jump * lambda_jump),
            (lambda_jump * lambda_jump) / adj,
        );
        let lambda_star = vec![lcs];
        let negdiff = -dgamma_log(
            lcs,
            adj * adj / (lambda_jump * lambda_jump),
            adj / (lambda_jump * lambda_jump),
        ) + dgamma_log(
            lambdacomp,
            lam_adj(lcs) * lam_adj(lcs) / (lambda_jump * lambda_jump),
            lam_adj(lcs) / (lambda_jump * lambda_jump),
        );
        let diffpriors = dgamma_log(
            lcs,
            mu_lambda * mu_lambda / (sigma_lambda * sigma_lambda),
            mu_lambda / (sigma_lambda * sigma_lambda),
        ) - dgamma_log(
            lambdacomp,
            mu_lambda * mu_lambda / (sigma_lambda * sigma_lambda),
            mu_lambda / (sigma_lambda * sigma_lambda),
        );
        let r_prev: Vec<f64> = (0..m).map(|j| r[(s - 1, j)]).collect();
        let delta_prev: Vec<f64> = (0..m)
            .map(|j| if varsel { delta[(s - 1, j)] } else { 1.0 })
            .collect();
        let r_same = r_prev.clone();
        let delta_same = delta_prev.clone();
        let mh = mh_step_rng(
            rng,
            &r_same,
            &lambda_sim,
            &lambda_star,
            &r_same,
            &delta_same,
            &delta_same,
            &ycont,
            x,
            z,
            &beta_new,
            sigsq_eps[s],
            diffpriors,
            negdiff,
            &vcomps,
            f64::NAN,
        )?;
        lambda_sim = mh.lambda.clone();
        if mh.acc {
            vcomps = mh.vcomps;
            acc_lambda[(s, 0)] = 1.0;
        }
        lambda[(s, 0)] = lambda_sim[0];

        // r update for forced-in variables
        let mut r_sim = r_prev.clone();
        if !forced.is_empty() && opts.rmethod != RMethod::Fixed {
            match opts.rmethod {
                RMethod::Equal => {
                    // single r for all forced
                    let res = r_update_one(
                        rng,
                        &r_sim,
                        &forced,
                        &delta_prev,
                        &lambda_sim,
                        &ycont,
                        x,
                        z,
                        &beta_new,
                        sigsq_eps[s],
                        &vcomps,
                        rparams,
                        prior,
                    )?;
                    if res.acc {
                        vcomps = res.vcomps;
                        for &c in &forced {
                            acc_r[(s, c - 1)] = 1.0;
                        }
                    }
                    r_sim = res.r;
                }
                RMethod::Varying => {
                    for &whichr in &forced {
                        let res = r_update_one(
                            rng,
                            &r_sim,
                            &[whichr],
                            &delta_prev,
                            &lambda_sim,
                            &ycont,
                            x,
                            z,
                            &beta_new,
                            sigsq_eps[s],
                            &vcomps,
                            rparams,
                            prior,
                        )?;
                        r_sim = res.r;
                        if res.acc {
                            vcomps = res.vcomps;
                            acc_r[(s, whichr - 1)] = 1.0;
                        }
                    }
                }
                _ => {}
            }
        }

        // rdelta update (varsel)
        if varsel {
            let res = rdelta_comp_update(
                rng,
                &r_sim,
                &delta_prev,
                &lambda_sim,
                &ycont,
                x,
                z,
                &beta_new,
                sigsq_eps[s],
                &vcomps,
                &ztest_1based,
                cp,
                rparams,
                prior,
            )?;
            for j in 0..m {
                delta[(s, j)] = res.delta[j];
            }
            r_sim = res.r;
            move_type[s] = res.move_type;
            if res.acc {
                vcomps = res.vcomps;
                acc_rdelta[s] = 1.0;
            }
        } else {
            for j in 0..m {
                delta[(s, j)] = delta_prev[j];
            }
        }
        for j in 0..m {
            r[(s, j)] = r_sim[j];
        }
    }

    Ok(BkmrFit {
        h_hat,
        beta,
        lambda,
        sigsq_eps,
        r,
        delta,
        acc_r,
        acc_lambda,
        acc_rdelta,
        move_type,
        iter: nsamp,
        varsel,
        ztest: ztest_1based,
        control_params: cp.clone(),
    })
}

/// `r.update(r, whichcomp, ...)` — single Metropolis step for the r value(s)
/// at indices `whichcomp` (1-based). When `whichcomp` has multiple entries,
/// they share a single r value (the "equal" method).
fn r_update_one(
    rng: &mut Rng,
    r: &[f64],
    whichcomp: &[usize],
    delta: &[f64],
    lambda: &[f64],
    y: &[f64],
    x: Mref,
    z: Mref,
    beta: &[f64],
    sigsq: f64,
    vcomps: &VComps,
    p: Rp,
    prior: RPrior,
) -> Result<MhResult> {
    // rcomp = unique(r[whichcomp]); should be scalar
    let rcomp = r[whichcomp[0] - 1];
    // proposal
    let rcomp_star = rprop_gen(rng, rcomp, p, prior);
    let negdiff =
        -rprop_logdens(rcomp_star, rcomp, p, prior) + rprop_logdens(rcomp, rcomp_star, p, prior);
    let diffpriors = rprior_logdens(rcomp_star, p, prior) - rprior_logdens(rcomp, p, prior);
    let mut r_star = r.to_vec();
    for &c in whichcomp {
        r_star[c - 1] = rcomp_star;
    }
    let delta_star = delta.to_vec();
    let lambda_star = lambda.to_vec();
    mh_step_rng(
        rng,
        r,
        lambda,
        &lambda_star,
        &r_star,
        delta,
        &delta_star,
        y,
        x,
        z,
        beta,
        sigsq,
        diffpriors,
        negdiff,
        vcomps,
        f64::NAN,
    )
}

/// `rdelta.comp.update` — joint (r, delta) update for component-wise variable
/// selection.
fn rdelta_comp_update(
    rng: &mut Rng,
    r: &[f64],
    delta: &[f64],
    lambda: &[f64],
    y: &[f64],
    x: Mref,
    z: Mref,
    beta: &[f64],
    sigsq: f64,
    vcomps: &VComps,
    ztest: &[usize],
    cp: &ControlParams,
    p: Rp,
    prior: RPrior,
) -> Result<MhResult> {
    let a_p0 = cp.a_p0;
    let b_p0 = cp.b_p0;
    let mut delta_star = delta.to_vec();
    let mut r_star = r.to_vec();

    // move type
    let all_zero = ztest.iter().all(|&zt| delta[zt - 1] == 0.0);
    let (move_type, move_prob): (f64, f64) = if all_zero {
        (1.0, 1.0)
    } else {
        // sample c(1,2)
        let choices = [1usize, 2];
        let picked = *rng.sample_one(&choices) as f64;
        (picked, 0.5)
    };

    let diffpriors;
    let negdiff;

    if move_type == 1.0 {
        // comp <- sample(ztest, 1)
        let comp = if ztest.len() == 1 {
            ztest[0]
        } else {
            let idx = rng.sample_int_one(ztest.len()) - 1;
            ztest[idx]
        };
        // delta.star[comp] <- 1 - delta[comp]
        delta_star[comp - 1] = 1.0 - delta[comp - 1];
        let all_zero_star = ztest.iter().all(|&zt| delta_star[zt - 1] == 0.0);
        let move_prob_star: f64 = if all_zero_star { 1.0 } else { 0.5 };
        // r.star[comp] <- ifelse(delta.star[comp]==0, 0, rprop.gen1)
        r_star[comp - 1] = if delta_star[comp - 1] == 0.0 {
            0.0
        } else {
            rprop_gen1(rng, p, prior)
        };
        // diffpriors: lgamma sums + rprior term
        let sum_delta_star: f64 = ztest.iter().map(|&zt| delta_star[zt - 1]).sum();
        let sum_delta: f64 = ztest.iter().map(|&zt| delta[zt - 1]).sum();
        let nz = ztest.len() as f64;
        let sign = if delta[comp - 1] == 1.0 { -1.0 } else { 1.0 };
        // r.sel = if delta[comp]==1 { r[comp] } else { r_star[comp] }
        let r_sel = if delta[comp - 1] == 1.0 {
            r[comp - 1]
        } else {
            r_star[comp - 1]
        };
        diffpriors = (lgamma_fn(sum_delta_star + a_p0) + lgamma_fn(nz - sum_delta_star + b_p0)
            - lgamma_fn(sum_delta + a_p0)
            - lgamma_fn(nz - sum_delta + b_p0))
            + sign * rprior_logdens(r_sel, p, prior);
        negdiff = -move_prob_star.ln() + move_prob.ln() - sign * rprop_logdens1(r_sel, p, prior);
    } else {
        // move type 2: update r of a randomly selected included component
        let included: Vec<usize> = (0..delta.len())
            .filter(|&i| delta[i] == 1.0)
            .map(|i| i + 1)
            .collect();
        let comp = if included.len() == 1 {
            included[0]
        } else {
            let idx = rng.sample_int_one(included.len()) - 1;
            included[idx]
        };
        r_star[comp - 1] = rprop_gen2(rng, r[comp - 1], p, prior);
        diffpriors =
            rprior_logdens(r_star[comp - 1], p, prior) - rprior_logdens(r[comp - 1], p, prior);
        negdiff = -rprop_logdens2(r_star[comp - 1], r[comp - 1], p, prior)
            + rprop_logdens2(r[comp - 1], r_star[comp - 1], p, prior);
    }

    let lambda_star = lambda.to_vec();
    mh_step_rng(
        rng,
        r,
        lambda,
        &lambda_star,
        &r_star,
        delta,
        &delta_star,
        y,
        x,
        z,
        beta,
        sigsq,
        diffpriors,
        negdiff,
        vcomps,
        move_type,
    )
}
