//! Faithful Rust port of `hetmixlin.f90::funcpa` — the hlme log-likelihood.
//!
//! The arithmetic matches the Fortran line-for-line, including the unusual
//! 2π/log-determinant convention (`vrais -= nmes·log(2π)` once per subject,
//! `funcpa = vrais/2` at the end), so that gradient/Hessian finite-differences
//! evaluated by the optimizer reproduce the R/Fortran trajectory.

use crate::data::LongData;
use crate::optimizer::Objective;
use crate::spec::{ModelSpec, ParamLayout};

/// ln(2π) — precomputed because `f64::ln` is not `const`.
const LOG_2PI: f64 = 1.8378770664093453;
const EPS: f64 = 1e-20;

/// Bundled likelihood evaluator: owns the data + spec + posfix mask so the
/// optimizer can call `eval_full(&b)` with just the free parameter vector.
pub struct HlmeLikelihood<'a> {
    pub data: &'a LongData,
    pub spec: &'a ModelSpec,
    pub layout: ParamLayout,
    pub fix0: Vec<u8>,
    pub bfix: Vec<f64>,
}

impl<'a> HlmeLikelihood<'a> {
    pub fn new(
        data: &'a LongData,
        spec: &'a ModelSpec,
        fix0: &[u8],
        bfix: &[f64],
    ) -> Self {
        Self {
            data,
            spec,
            layout: spec.layout(),
            fix0: fix0.to_vec(),
            bfix: bfix.to_vec(),
        }
    }

    /// Expand the free parameter vector `b_free` into the full layout by
    /// splicing in `bfix` at posfix positions. Mirrors Fortran `funcpa` lines
    /// 123–138.
    fn expand_b(&self, b_free: &[f64]) -> Vec<f64> {
        let npm_tot = self.layout.npm;
        let mut b1 = vec![0.0; npm_tot];
        let mut l = 0; // free index
        let mut m = 0; // fixed index
        for k in 0..npm_tot {
            if self.fix0[k] == 0 {
                b1[k] = b_free[l];
                l += 1;
            } else {
                b1[k] = self.bfix[m];
                m += 1;
            }
        }
        b1
    }

    /// Evaluate the log-likelihood at the *free* parameter vector `b_free`.
    /// Returns `-1e9` on singular covariance (matches Fortran's `funcpa=-1.d9`).
    pub fn eval_full(&self, b_free: &[f64]) -> f64 {
        let b1 = self.expand_b(b_free);
        self.funcpa(&b1)
    }
}

impl<'a> Objective for HlmeLikelihood<'a> {
    fn eval(&self, b: &[f64]) -> f64 {
        self.eval_full(b)
    }
}

impl<'a> HlmeLikelihood<'a> {

    /// The hlme log-likelihood at the full parameter vector `b1` (length NPM).
    /// Direct port of `hetmixlin.f90::funcpa`.
    pub fn funcpa(&self, b1: &[f64]) -> f64 {
        let L = &self.layout;
        let data = self.data;
        let ng = L.ng;
        let nea = L.nea;
        let nwg_active = self.spec.nwg && ng > 1;

        // Build Ut once (class-shared Cholesky of B); for nwg it is rescaled
        // per-class inside the loop.
        let ut = L.cholesky_ut(b1, self.spec.idiag);
        let n = nea;

        let mut vrais: f64 = 0.0;

        for i in 0..data.ns {
            let ni = data.nmes[i];
            if ni == 0 {
                continue;
            }

            // --- Build Zi (ni × nea) from idea==1 covariate columns -----------
            let mut zi = vec![0.0; ni * n];
            if n > 0 {
                let mut l = 0;
                for k in 0..L.nv {
                    if self.spec.idea[k] == 1 {
                        for j in 0..ni {
                            zi[j * n + l] = data.x_at(i, j, k);
                        }
                        l += 1;
                    }
                }
            }

            // --- Build Corr_i (ni × ni) from cor time variable ----------------
            let mut corr = vec![0.0; ni * ni];
            let mut tcor = vec![0.0; ni];
            if L.ncor > 0 {
                for k in 0..L.nv {
                    if self.spec.idcor[k] == 1 {
                        for j in 0..ni {
                            tcor[j] = data.x_at(i, j, k);
                        }
                    }
                }
            }
            let stderr = b1[L.i_stderr];
            let sigma_e_sq = stderr * stderr;
            for j1 in 0..ni {
                for j2 in 0..ni {
                    let mut c = 0.0;
                    if j1 == j2 {
                        c += sigma_e_sq;
                    }
                    if L.ncor == 1 {
                        // BM: sigma_bm^2 * min(t_j1, t_j2)
                        let sbm = b1[L.i_ncor];
                        c += sbm * sbm * tcor[j1].min(tcor[j2]);
                    } else if L.ncor == 2 {
                        // AR(1): sigma_ar^2 * exp(-rho * |dt|)
                        let rho = b1[L.i_ncor];
                        let sigma_ar = b1[L.i_ncor + 1];
                        c += sigma_ar * sigma_ar
                            * (-rho * (tcor[j1] - tcor[j2]).abs()).exp();
                    }
                    corr[j1 * ni + j2] = c;
                }
            }

            // --- Y_i ----------------------------------------------------------
            let y1 = data.y_subject(i).to_vec();

            // Pre-decompose V_i = Zi Ut Ut' Zi' + Corr when nwg is inactive
            // (single shared V across classes). Returns (Vi, logdet) or None.
            let shared_v: Option<(Vec<f64>, f64)> = if !nwg_active {
                let ut1 = if ng == 1 { ut.clone() } else { ut.clone() };
                Self::build_v_and_invert(&zi, &ut1, n, &corr, ni)
            } else {
                None
            };

            // -1/2 * n_i * log(2π)  (Fortran does this once per subject, before
            // the class loop; the final /2 makes it the standard Gaussian term)
            vrais -= ni as f64 * LOG_2PI;

            if ng == 1 {
                // ----- ng=1 path (Fortran lines 272–299) ----------------------
                let (vi_inv, det) = match &shared_v {
                    Some(t) => t.clone(),
                    None => {
                        let ut1 = ut.clone();
                        match Self::build_v_and_invert(&zi, &ut1, n, &corr, ni) {
                            Some(t) => t,
                            None => return -1e9,
                        }
                    }
                };

                // mu = X0_i · b0 (idg != 0 covars)
                let mu = self.class_mean(b1, i, ni, 1); // g=1 (1-indexed for consistency)
                let y2: Vec<f64> = (0..ni).map(|j| y1[j] - mu[j]).collect();
                let y3 = matvec(&vi_inv, &y2, ni);
                let y4 = (0..ni).map(|j| y2[j] * y3[j]).sum::<f64>();

                vrais -= det;
                vrais -= y4;
            } else {
                // ----- ng>1 path (Fortran lines 302–454) ----------------------
                // Class-membership probabilities pi[g].
                let pi = self.class_probs(b1, i);

                // Per-class V_i (only recompute when nwg rescales Ut).
                let mut expo: f64 = 0.0;
                for g in 1..=ng {
                    let (vi_inv, det) = if nwg_active {
                        let ut1 = L.scale_ut(&ut, b1, g, true);
                        match Self::build_v_and_invert(&zi, &ut1, n, &corr, ni) {
                            Some(t) => t,
                            None => return -1e9,
                        }
                    } else {
                        shared_v.clone().unwrap()
                    };

                    let mu = self.class_mean(b1, i, ni, g);
                    let y2: Vec<f64> = (0..ni).map(|j| y1[j] - mu[j]).collect();
                    let y3 = matvec(&vi_inv, &y2, ni);
                    let y4 = (0..ni).map(|j| y2[j] * y3[j]).sum::<f64>();

                    let fi = (-0.5 * (det + y4)).exp();
                    expo += pi[g - 1] * fi;
                }
                if expo <= 0.0 {
                    return -1e9;
                }
                vrais += 2.0 * expo.ln();
            }
        }

        vrais / 2.0
    }

    /// Build V = Z·Ut·Ut'·Z' + Corr (ni×ni, row-major), invert via Cholesky,
    /// return (V^{-1}, log|V|). None on singular.
    fn build_v_and_invert(
        zi: &[f64],
        ut: &[f64],
        nea: usize,
        corr: &[f64],
        ni: usize,
    ) -> Option<(Vec<f64>, f64)> {
        // P = Z · Ut    (ni × nea)
        let mut p = vec![0.0; ni * nea];
        for j in 0..ni {
            for l in 0..nea {
                let mut s = 0.0;
                for k in 0..nea {
                    s += zi[j * nea + k] * ut[k * nea + l];
                }
                p[j * nea + l] = s;
            }
        }
        // VC = P · P' + Corr
        let mut vc = vec![0.0; ni * ni];
        for j in 0..ni {
            for k in 0..ni {
                let mut s = corr[j * ni + k];
                for l in 0..nea {
                    s += p[j * nea + l] * p[k * nea + l];
                }
                vc[j * ni + k] = s;
            }
        }
        invert_pd(&vc, ni)
    }

    /// Class-membership probabilities `pi[g]` for subject `i`.
    /// Mirrors Fortran lines 303–348: multinomial logit if any classmb covars,
    /// else uniform 1/ng; then multiplied by pprior; overridden by `prior`.
    fn class_probs(&self, b1: &[f64], i: usize) -> Vec<f64> {
        let L = &self.layout;
        let ng = L.ng;
        let data = self.data;
        let mut pi = vec![0.0; ng];

        if data.prior[i] != 0 {
            pi[(data.prior[i] - 1) as usize] = 1.0;
            return pi;
        }

        if L.nprob > 0 {
            // Xprob = classmb covariates at subject's first observation.
            let mut xprob = vec![0.0; L.nvarprob];
            let mut l = 0;
            for k in 0..L.nv {
                if self.spec.idprob[k] == 1 {
                    xprob[l] = data.x_at(i, 0, k);
                    l += 1;
                }
            }
            // eta_g = sum_k b1[(k-1)*(ng-1)+g] * xprob[k],  g=1..ng-1
            // (Fortran 1-indexed: bprob(k) = b1((k-1)*(ng-1)+g))
            let mut temp = 0.0;
            for g in 1..ng {
                let mut eta = 0.0;
                for k in 0..L.nvarprob {
                    let bprob = b1[k * (ng - 1) + (g - 1)];
                    eta += bprob * xprob[k];
                }
                pi[g - 1] = eta.exp();
                temp += eta.exp();
            }
            pi[ng - 1] = 1.0 / (1.0 + temp);
            for g in 1..ng {
                pi[g - 1] *= pi[ng - 1];
            }
        } else {
            for v in &mut pi {
                *v = 1.0;
            }
        }

        // Multiply by pprior (default all 1.0).
        for g in 0..ng {
            pi[g] *= data.pprior[i * ng + g];
        }
        pi
    }

    /// Class-conditional mean mu_g = X0 · b0_common + X2 · b2_g.
    /// Mirrors Fortran lines 350–386 (`nmoins` accumulation).
    /// `class1` is 1-indexed.
    fn class_mean(&self, b1: &[f64], i: usize, ni: usize, class1: usize) -> Vec<f64> {
        let L = &self.layout;
        let data = self.data;
        let mut mu = vec![0.0; ni];
        let mut nmoins = 0_isize;
        for k in 0..L.nv {
            match self.spec.idg[k] {
                1 => {
                    // overall fixed effect
                    let beta = b1[L.nprob + nmoins as usize];
                    for j in 0..ni {
                        mu[j] += beta * data.x_at(i, j, k);
                    }
                    nmoins += 1;
                }
                2 => {
                    // class-specific: ng entries per covariate
                    let beta = b1[L.nprob + nmoins as usize + (class1 - 1)];
                    for j in 0..ni {
                        mu[j] += beta * data.x_at(i, j, k);
                    }
                    nmoins += L.ng as isize;
                }
                _ => {}
            }
        }
        mu
    }
}

// ---- Linear-algebra helpers (small dense SPD operations) ---------------

/// `y = A · x` for ni×ni row-major `A`.
fn matvec(a: &[f64], x: &[f64], ni: usize) -> Vec<f64> {
    let mut y = vec![0.0; ni];
    for i in 0..ni {
        let mut s = 0.0;
        for j in 0..ni {
            s += a[i * ni + j] * x[j];
        }
        y[i] = s;
    }
    y
}

/// Invert a positive-definite ni×ni matrix via Cholesky, returning
/// `(V^{-1} row-major, log|V|)`. `None` if not PD.
///
/// This replaces Fortran `DSINV` (which does the same: Cholesky factorize,
/// log-det from the factor, invert by back-substitution). The numerical
/// result agrees to machine precision.
pub(crate) fn invert_pd(vc: &[f64], ni: usize) -> Option<(Vec<f64>, f64)> {
    // Cholesky: L·L' = A, lower-triangular L.
    let mut l = vec![0.0; ni * ni];
    let mut log_det = 0.0;
    for j in 0..ni {
        let mut d = vc[j * ni + j];
        for k in 0..j {
            d -= l[j * ni + k] * l[j * ni + k];
        }
        if d <= EPS {
            return None;
        }
        let dj = d.sqrt();
        l[j * ni + j] = dj;
        log_det += dj.ln();
        let inv_dj = 1.0 / dj;
        for i in (j + 1)..ni {
            let mut s = vc[i * ni + j];
            for k in 0..j {
                s -= l[i * ni + k] * l[j * ni + k];
            }
            l[i * ni + j] = s * inv_dj;
        }
    }
    log_det *= 2.0;

    // Invert L (lower-tri), then V^{-1} = L^{-T} · L^{-1}.
    // Linv lower-triangular.
    let mut linv = vec![0.0; ni * ni];
    for i in 0..ni {
        linv[i * ni + i] = 1.0 / l[i * ni + i];
        for j in 0..i {
            let mut s = 0.0;
            for k in j..i {
                s += l[i * ni + k] * linv[k * ni + j];
            }
            linv[i * ni + j] = -s / l[i * ni + i];
        }
    }
    // Vinv = Linv' · Linv (only need row-major full)
    let mut vinv = vec![0.0; ni * ni];
    for i in 0..ni {
        for j in 0..ni {
            let mut s = 0.0;
            for k in 0..ni {
                // Linv' [i,k] = Linv[k,i]
                s += linv[k * ni + i] * linv[k * ni + j];
            }
            vinv[i * ni + j] = s;
        }
    }
    Some((vinv, log_det))
}

/// Free function wrapper for cross-validation tests.
pub fn loglik_hlme(
    b: &[f64],
    data: &LongData,
    spec: &ModelSpec,
) -> f64 {
    let layout = spec.layout();
    debug_assert_eq!(b.len(), layout.npm);
    let fix0 = vec![0u8; layout.npm];
    let ll = HlmeLikelihood::new(data, spec, &fix0, &[]);
    ll.funcpa(b)
}
