//! Rust port of the R package [`lcmm`](https://cecileproust-lima.github.io/lcmm/)
//! (Proust-Lima, Philipps, Liquet 2017, *JSS* 78(2)) — the `hlme` function
//! (latent class linear mixed models), which subsumes GBTM / LCGA / LGMM.

#![allow(non_snake_case)]
//!
//! ## Model
//!
//! For subject *i* (with *n_i* observations) and latent class *g*:
//! ```text
//! Y_i = X0_i β₀ + X2_i β₂_g + Z_i b_i,g + ε_i
//! b_i,g ~ N(0, B_g),   ε_i ~ N(0, σ²I + Corr_i)
//! P(class(i)=g | classmb covariates) ∝ exp(w_i' γ_g)   (multinomial logit)
//! ```
//! with class-shared (`nwg=FALSE`) or class-scaled (`nwg=TRUE`) random-effect
//! covariance `B_g`, and optional residual correlation `Corr_i` from Brownian
//! motion (`cor=BM(t)`) or AR(1) (`cor=AR(t)`).
//!
//! **GBTM / LCGA** is the special case with `random=~-1` (no random effects):
//! `B = 0`, all within-class variability is homoscedastic residual σ².
//!
//! ## Parameter vector layout (matches R `hlme` exactly)
//!
//! `B = [NPROB | NEF | NVC | NW | NCOR | STDERR]`
//! 1. `NPROB = (# classmb covars) × (ng−1)`  — multinomial logit class membership
//! 2. `NEF   = (# idg=1 covars) + (# idg=2 covars) × ng` — trajectory means
//! 3. `NVC   = nea` (idiag) or `nea(nea+1)/2` (full) — Cholesky upper of B
//! 4. `NW    = ng−1` if `nwg` else 0 — class-specific RE scaling
//! 5. `NCOR  = 0/1/2` — BM(1) / AR(2) residual correlation params
//! 6. last  — residual standard deviation σ
//!
//! ## Cross-validation
//!
//! Faithful port validated against R `lcmm::hlme` on `data_hlme` across all
//! feature branches (ng=1/2/3, random full/diagonal, nwg, GBTM, classmb).
//! See `tests/cross_validation.rs`.
//!
//! # References
//! - Proust-Lima C, Philipps V, Liquet B (2017). *Estimation of Extended Mixed
//!   Models Using Latent Classes and Latent Processes: The R Package lcmm.*
//!   JSS 78(2), 1–56.
//! - Nagin DS (1999/2005). *Group-Based Modeling of Development.* Harvard UP.
//! - Philipps V et al. (2021). *marqLevAlg Parallel R Package.* R Journal 13(2).

pub mod data;
pub mod likelihood;
pub mod optimizer;
pub mod posterior;
pub mod spec;

pub use data::LongData;
pub use likelihood::{HlmeLikelihood, loglik_hlme};
pub use optimizer::{MlaControl, MlaResult, marq_lev_alg};
pub use posterior::{PosteriorResult, compute_posterior, predict_y};
pub use spec::{ModelSpec, ParamLayout};

use serde::{Deserialize, Serialize};
use thiserror::Error;

// =====================================================================
// Error
// =====================================================================

#[derive(Debug, Error)]
pub enum LcmmError {
    #[error("invalid input: {0}")]
    BadInput(String),
    #[error("linear algebra failed: {0}")]
    Linalg(String),
    #[error("optimization failed to converge (istop={istop}): {msg}")]
    NoConverge { istop: i32, msg: String },
}
pub type Result<T> = std::result::Result<T, LcmmError>;

// =====================================================================
// Fit result
// =====================================================================

/// Convergence status (mirrors R `lcmm` `conv` / marqLevAlg `istop`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConvStatus {
    /// istop=1: all three convergence criteria satisfied.
    Converged = 1,
    /// istop=2: maxiter reached.
    MaxIter = 2,
    /// istop=3: convergence with partial Hessian.
    PartialH = 3,
    /// istop=4: numerical problem (infinite params/function).
    Failed = 4,
}

impl ConvStatus {
    pub fn from_istop(istop: i32) -> Self {
        match istop {
            1 => ConvStatus::Converged,
            2 => ConvStatus::MaxIter,
            3 => ConvStatus::PartialH,
            _ => ConvStatus::Failed,
        }
    }
    pub fn istop(self) -> i32 {
        self as i32
    }
    /// R lcmm treats istop ∈ {1,2,3} as "usable for posterior computation".
    pub fn usable(self) -> bool {
        matches!(
            self,
            ConvStatus::Converged | ConvStatus::MaxIter | ConvStatus::PartialH
        )
    }
}

/// A complete `hlme` fit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HlmeFit {
    pub best: Vec<f64>,
    /// Upper-triangular variance-covariance of `best` (packed, length NPM*(NPM+1)/2).
    pub v: Vec<f64>,
    pub loglik: f64,
    pub conv: ConvStatus,
    pub niter: usize,
    /// [ca, cb, rdm] convergence criteria at stopping point.
    pub gconv: [f64; 3],
    pub aic: f64,
    pub bic: f64,
    pub ns: usize,
    pub ng: usize,
    pub layout: ParamLayout,
    pub posterior: Option<PosteriorResult>,
}

impl HlmeFit {
    /// Number of estimated parameters (length of `best`).
    pub fn npm(&self) -> usize {
        self.best.len()
    }
}

// =====================================================================
// Top-level hlme() entry point
// =====================================================================

/// Tunable controls for the `hlme` fit (mirror R `hlme()` arguments).
#[derive(Clone, Debug)]
pub struct HlmeControl {
    pub convB: f64,
    pub convL: f64,
    pub convG: f64,
    pub maxiter: usize,
    /// Indices (1-based, matching R) of parameters held fixed at their init value.
    pub posfix: Vec<usize>,
    pub verbose: bool,
    pub multiple_try: i32,
    pub blinding: bool,
}

impl Default for HlmeControl {
    fn default() -> Self {
        Self {
            convB: 0.0001,
            convL: 0.0001,
            convG: 0.0001,
            maxiter: 500,
            posfix: Vec::new(),
            verbose: false,
            multiple_try: 25,
            blinding: false,
        }
    }
}

/// Fit an `hlme` model given pre-assembled long-format data + model spec +
/// initial parameter vector `b`.
///
/// `b` is the *free* parameter vector (posfix entries already excluded), and
/// `bfix` holds the posfix values in the order they appear in the full layout.
/// For no posfix, pass `bfix = &[]`.
pub fn hlme(
    data: &LongData,
    spec: &ModelSpec,
    b: &[f64],
    bfix: &[f64],
    control: &HlmeControl,
) -> Result<HlmeFit> {
    let layout = spec.layout();
    let npm_free = b.len();
    let nfix = control.posfix.len();

    if nfix > 0 {
        // Validate posfix indices are within [1, NPM_total].
        let npm_tot = layout.npm;
        for &idx in &control.posfix {
            if idx < 1 || idx > npm_tot {
                return Err(LcmmError::BadInput(format!(
                    "posfix index {idx} out of range [1, {npm_tot}]"
                )));
            }
        }
        if npm_free + nfix != npm_tot {
            return Err(LcmmError::BadInput(format!(
                "len(b) + len(bfix) = {} != NPM total {}",
                npm_free + nfix,
                npm_tot
            )));
        }
        if bfix.len() != nfix {
            return Err(LcmmError::BadInput(format!(
                "bfix length {} != posfix length {}",
                bfix.len(),
                nfix
            )));
        }
    } else if npm_free != layout.npm {
        return Err(LcmmError::BadInput(format!(
            "len(b) = {} != NPM = {}",
            npm_free, layout.npm
        )));
    }

    // Build the fixed-parameter mask in the full layout.
    let fix0: Vec<u8> = {
        let mut f = vec![0u8; layout.npm];
        for &idx in &control.posfix {
            f[idx - 1] = 1;
        }
        f
    };

    let ll = HlmeLikelihood::new(data, spec, &fix0, bfix);

    // Evaluate loglik at init; if maxiter=0, just return the eval.
    let init_ll = ll.eval_full(b);
    if control.maxiter == 0 {
        return Ok(HlmeFit {
            best: b.to_vec(),
            v: vec![f64::NAN; layout.npm * (layout.npm + 1) / 2],
            loglik: init_ll,
            conv: ConvStatus::MaxIter,
            niter: 0,
            gconv: [f64::NAN; 3],
            aic: 2.0 * (npm_free as f64) - 2.0 * init_ll,
            bic: (npm_free as f64) * (data.ns as f64).ln() - 2.0 * init_ll,
            ns: data.ns,
            ng: spec.ng,
            layout,
            posterior: None,
        });
    }

    // Run Marquardt-Levenberg.
    let mla_ctrl = MlaControl {
        maxiter: control.maxiter,
        epsa: control.convB,
        epsb: control.convL,
        epsd: control.convG,
        minimize: false,
        blinding: control.blinding,
        multiple_try: control.multiple_try,
        verbose: control.verbose,
        partialH: Vec::new(),
    };
    let res: MlaResult = marq_lev_alg(&ll, b, &mla_ctrl)?;

    // Recover the full parameter vector (re-insert posfix).
    let best_full: Vec<f64> = if nfix > 0 {
        let mut full = vec![0.0_f64; layout.npm];
        let mut kf = 0;
        let mut kb = 0;
        for k in 0..layout.npm {
            if fix0[k] == 1 {
                full[k] = bfix[kf];
                kf += 1;
            } else {
                full[k] = res.best[kb];
                kb += 1;
            }
        }
        full
    } else {
        res.best.clone()
    };

    // Build full V (upper-tri packed) by expanding the free-block V and
    // zeroing rows/cols of fixed params (matching R's behavior).
    let v_full = expand_v(&res.v, layout.npm, &fix0, res.istop);

    let n_param_eff = layout.npm - nfix;
    let aic = 2.0 * (n_param_eff as f64) - 2.0 * res.fn_value;
    let bic = (n_param_eff as f64) * (data.ns as f64).ln() - 2.0 * res.fn_value;
    let conv = ConvStatus::from_istop(res.istop);

    // Posterior classification + predictions when usable.
    let posterior = if conv.usable() {
        Some(compute_posterior(&ll, &best_full))
    } else {
        None
    };

    Ok(HlmeFit {
        best: best_full,
        v: v_full,
        loglik: res.fn_value,
        conv,
        niter: res.niter,
        gconv: res.gconv,
        aic,
        bic,
        ns: data.ns,
        ng: spec.ng,
        layout,
        posterior,
    })
}

/// Expand a free-block packed upper-triangular V back into the full NPM×NPM
/// layout, inserting zeros at posfix positions (R lcmm convention when
/// `istop != 3`, NA when istop==3 — we use NA only for partialH which we don't
/// exercise in the MVP).
fn expand_v(v_free: &[f64], npm_tot: usize, fix0: &[u8], istop: i32) -> Vec<f64> {
    let mut full = vec![0.0_f64; npm_tot * (npm_tot + 1) / 2];
    // Build the free-index list.
    let free_idx: Vec<usize> = (0..npm_tot).filter(|&k| fix0[k] == 0).collect();
    let _m = free_idx.len();
    // Pack from full(m,m) → full(npm_tot, npm_tot): full[i_free, j_free] = v_free[i,j]
    for (ii, &i) in free_idx.iter().enumerate() {
        for (jj, &j) in free_idx.iter().enumerate().skip(ii) {
            let src = jj * (jj + 1) / 2 + ii; // packed index in free block
            let dst = j * (j + 1) / 2 + i; // packed index in full layout
            full[dst] = if istop == 4 { f64::NAN } else { v_free[src] };
        }
    }
    full
}
