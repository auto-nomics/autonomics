//! Post-fit quantities: posterior class probabilities, empirical-Bayes random
//! effects, subject-specific predictions, and out-of-sample trajectory
//! prediction (`predictY`). Faithful port of `hetmixlin.f90::postprob` +
//! `hetmixlin.f90::residuals` and `R/predictY.hlme.R`.

use crate::data::LongData;
use crate::likelihood::invert_pd;
use crate::spec::{ModelSpec, ParamLayout};
use serde::{Deserialize, Serialize};

/// Posterior classification + predictions for every subject.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PosteriorResult {
    /// `ns × ng` posterior class probabilities P(class=g | Y_i), row-major.
    pub ppi: Vec<f64>,
    /// `ns` most-probable class assignments (1-indexed).
    pub class: Vec<usize>,
    /// `nobs × ng` class-conditional marginal means `pred_m_g`, row-major.
    pub pred_m_g: Vec<f64>,
    /// `nobs × ng` class-conditional subject-specific means `pred_ss_g`
    /// (only differs from `pred_m_g` when random effects are present),
    /// row-major.
    pub pred_ss_g: Vec<f64>,
    /// `nobs` marginal residuals `Y - pred_m` (weighted by π for ng>1).
    pub resid_m: Vec<f64>,
    /// `nobs` subject-specific residuals `Y - pred_ss`.
    pub resid_ss: Vec<f64>,
    /// `ns * nea` overall (π-weighted) empirical-Bayes RE predictions
    /// (empty when no random effects).
    pub pred_re: Vec<f64>,
    /// `ns * ng * nea` per-class RE predictions (empty when no random effects).
    pub class_pred_re: Vec<f64>,
}

/// Compute the posterior classification and residuals at the converged `best`.
///
/// `best` must be the *full* parameter vector (posfix re-inserted).
pub fn compute_posterior<L>(ll: &L, best: &[f64]) -> PosteriorResult
where
    L: PosteriorLike,
{
    ll.compute_posterior(best)
}

/// Trait abstraction so the posterior logic can be tested independently.
pub trait PosteriorLike {
    fn compute_posterior(&self, best: &[f64]) -> PosteriorResult;
    fn layout(&self) -> &ParamLayout;
    fn data(&self) -> &LongData;
    fn spec(&self) -> &ModelSpec;
}

const LOG_2PI: f64 = 1.8378770664093453;

impl<'a> PosteriorLike for crate::likelihood::HlmeLikelihood<'a> {
    fn compute_posterior(&self, best: &[f64]) -> PosteriorResult {
        let L = &self.layout;
        let data = self.data;
        let spec = self.spec;
        let ng = L.ng;
        let nea = L.nea;
        let nobs = data.nobs;
        let nwg_active = spec.nwg && ng > 1;
        let ut = L.cholesky_ut(best, spec.idiag);

        let mut ppi = vec![0.0; data.ns * ng];
        let mut pred_m_g = vec![0.0; nobs * ng];
        let mut pred_ss_g = vec![0.0; nobs * ng];
        let mut resid_m = vec![0.0; nobs];
        let mut resid_ss = vec![0.0; nobs];
        let mut pred_re = vec![0.0; data.ns * nea];
        let mut class_pred_re = vec![0.0; data.ns * ng * nea];

        let mut row = 0_usize;
        for i in 0..data.ns {
            let ni = data.nmes[i];
            if ni == 0 {
                continue;
            }
            let y1 = data.y_subject(i).to_vec();

            let zi = build_zi(data, spec, L, i, ni);
            let corr = build_corr(data, spec, L, best, i, ni);
            let pi = class_probs(spec, data, L, best, i);

            // Per-class decomposition (V_i, logdet, Valea) and densities.
            let mut fi = vec![0.0_f64; ng];
            // Cache per-class (vi_inv, valea) for RE prediction reuse.
            let mut class_vi: Vec<Vec<f64>> = vec![Vec::new(); ng];
            let mut class_valea: Vec<Vec<f64>> = vec![Vec::new(); ng];

            // Shared decomposition for non-nwg case.
            let shared = if !nwg_active && nea > 0 {
                Some(decompose_subject(&zi, &ut, nea, &corr, ni))
            } else {
                None
            };

            for g in 1..=ng {
                let (vi_inv, valea) = if nwg_active && nea > 0 {
                    let ut_g = L.scale_ut(&ut, best, g, true);
                    let (vi, _, val) = decompose_subject(&zi, &ut_g, nea, &corr, ni);
                    (vi, val)
                } else if let Some((vi, _det, val)) = &shared {
                    (vi.clone(), val.clone())
                } else {
                    // nea == 0: V_i = Corr_i, no random effects.
                    let (vi, _) = invert_pd(&corr, ni).unwrap_or_else(|| {
                        let mut eye = vec![0.0; ni * ni];
                        for j in 0..ni { eye[j * ni + j] = 1.0; }
                        (eye, 0.0)
                    });
                    (vi, Vec::new())
                };
                let det = if let Some((_, d, _)) = &shared { *d } else { 0.0 };
                class_vi[g - 1] = vi_inv.clone();
                class_valea[g - 1] = valea.clone();

                let mu = class_mean(spec, data, L, best, i, ni, g);
                let y2: Vec<f64> = (0..ni).map(|j| y1[j] - mu[j]).collect();
                let y3 = matvec(&vi_inv, &y2, ni);
                let y4 = (0..ni).map(|j| y2[j] * y3[j]).sum::<f64>();

                let effective_det = if nea > 0 || nwg_active {
                    if let Some((_, d, _)) = &shared {
                        *d
                    } else {
                        // nwg_active with nea>0: recompute det.
                        let ut_g = L.scale_ut(&ut, best, g, true);
                        let (_, d, _) = decompose_subject(&zi, &ut_g, nea, &corr, ni);
                        d
                    }
                } else {
                    det
                };
                fi[g - 1] = (-0.5 * (ni as f64 * LOG_2PI + effective_det + y4)).exp();

                for j in 0..ni {
                    pred_m_g[(row + j) * ng + (g - 1)] = mu[j];
                }

                // Subject-specific prediction.
                let pred_ss = if nea > 0 {
                    let ut_g = if nwg_active {
                        L.scale_ut(&ut, best, g, true)
                    } else {
                        ut.clone()
                    };
                    subject_specific_pred(&zi, &ut_g, &vi_inv, &y2, &mu, nea, ni)
                } else {
                    mu.clone()
                };
                for j in 0..ni {
                    pred_ss_g[(row + j) * ng + (g - 1)] = pred_ss[j];
                }
            }

            // Posterior class probabilities.
            let f_total: f64 = (0..ng).map(|g| pi[g] * fi[g]).sum();
            let ppi_i: Vec<f64> = if f_total > 0.0 {
                (0..ng).map(|g| pi[g] * fi[g] / f_total).collect()
            } else {
                vec![1.0 / ng as f64; ng]
            };
            for g in 0..ng {
                ppi[i * ng + g] = ppi_i[g];
            }

            // π-weighted residuals + RE predictions.
            for g in 0..ng {
                let mu = class_mean(spec, data, L, best, i, ni, g + 1);
                let pred_ss = pred_ss_g.iter()
                    .skip(row * ng + g)
                    .step_by(ng)
                    .take(ni)
                    .copied()
                    .collect::<Vec<_>>();
                for j in 0..ni {
                    resid_m[row + j] += pi[g] * (y1[j] - mu[j]);
                    resid_ss[row + j] += ppi_i[g] * (y1[j] - pred_ss[j]);
                }
                // Empirical-Bayes RE: b̂_g = Valea · V^{-1} · (Y - mu)
                if nea > 0 {
                    let y2: Vec<f64> = (0..ni).map(|j| y1[j] - mu[j]).collect();
                    let err1 = matvec(&class_vi[g], &y2, ni);
                    let mut err2 = vec![0.0; nea];
                    for k in 0..nea {
                        for j in 0..ni {
                            err2[k] += class_valea[g][k * ni + j] * err1[j];
                        }
                    }
                    for k in 0..nea {
                        pred_re[i * nea + k] += ppi_i[g] * err2[k];
                        class_pred_re[i * ng * nea + g * nea + k] = err2[k];
                    }
                }
            }

            row += ni;
        }

        let class: Vec<usize> = (0..data.ns)
            .map(|i| {
                let mut best_g = 1;
                let mut best_p = ppi[i * ng];
                for g in 1..ng {
                    if ppi[i * ng + g] > best_p {
                        best_p = ppi[i * ng + g];
                        best_g = g + 1;
                    }
                }
                best_g
            })
            .collect();

        PosteriorResult {
            ppi,
            class,
            pred_m_g,
            pred_ss_g,
            resid_m,
            resid_ss,
            pred_re,
            class_pred_re,
        }
    }

    fn layout(&self) -> &ParamLayout {
        &self.layout
    }
    fn data(&self) -> &LongData {
        self.data
    }
    fn spec(&self) -> &ModelSpec {
        self.spec
    }
}

// ---- builders ----------------------------------------------------------

fn build_zi(data: &LongData, spec: &ModelSpec, L: &ParamLayout, i: usize, ni: usize) -> Vec<f64> {
    let nea = L.nea;
    let mut zi = vec![0.0; ni * nea];
    if nea == 0 {
        return zi;
    }
    let mut l = 0;
    for k in 0..L.nv {
        if spec.idea[k] == 1 {
            for j in 0..ni {
                zi[j * nea + l] = data.x_at(i, j, k);
            }
            l += 1;
        }
    }
    zi
}

fn build_corr(data: &LongData, spec: &ModelSpec, L: &ParamLayout, best: &[f64], i: usize, ni: usize) -> Vec<f64> {
    let mut corr = vec![0.0; ni * ni];
    let mut tcor = vec![0.0; ni];
    if L.ncor > 0 {
        for k in 0..L.nv {
            if spec.idcor[k] == 1 {
                for j in 0..ni {
                    tcor[j] = data.x_at(i, j, k);
                }
            }
        }
    }
    let sigma_e = best[L.i_stderr];
    for j1 in 0..ni {
        for j2 in 0..ni {
            let mut c = 0.0;
            if j1 == j2 {
                c += sigma_e * sigma_e;
            }
            if L.ncor == 1 {
                let sbm = best[L.i_ncor];
                c += sbm * sbm * tcor[j1].min(tcor[j2]);
            } else if L.ncor == 2 {
                let rho = best[L.i_ncor];
                let sigma_ar = best[L.i_ncor + 1];
                c += sigma_ar * sigma_ar * (-rho * (tcor[j1] - tcor[j2]).abs()).exp();
            }
            corr[j1 * ni + j2] = c;
        }
    }
    corr
}

/// Returns `(V^{-1}, log|V|, Valea)` where `Valea = Ut · (Z·Ut)'` (nea×ni).
fn decompose_subject(zi: &[f64], ut: &[f64], nea: usize, corr: &[f64], ni: usize) -> (Vec<f64>, f64, Vec<f64>) {
    // P = Z · Ut (ni × nea)
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
    // VC = P P' + Corr
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
    let (vi_inv, det) = invert_pd(&vc, ni).unwrap_or_else(|| {
        let mut eye = vec![0.0; ni * ni];
        for j in 0..ni { eye[j * ni + j] = 1.0; }
        (eye, 0.0)
    });
    // Valea = Ut · P'  (nea × ni)
    let mut valea = vec![0.0; nea * ni];
    for k in 0..nea {
        for j in 0..ni {
            let mut s = 0.0;
            for l in 0..nea {
                s += ut[k * nea + l] * p[j * nea + l];
            }
            valea[k * ni + j] = s;
        }
    }
    (vi_inv, det, valea)
}

fn subject_specific_pred(zi: &[f64], ut: &[f64], vi_inv: &[f64], y2: &[f64], mu: &[f64], nea: usize, ni: usize) -> Vec<f64> {
    let err1 = matvec(vi_inv, y2, ni);
    // CovDev = Z B Z' = P P'
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
    let mut pred = mu.to_vec();
    for j in 0..ni {
        let mut s = 0.0;
        for k in 0..ni {
            let mut covdev_jk = 0.0;
            for l in 0..nea {
                covdev_jk += p[j * nea + l] * p[k * nea + l];
            }
            s += covdev_jk * err1[k];
        }
        pred[j] += s;
    }
    pred
}

fn class_probs(spec: &ModelSpec, data: &LongData, L: &ParamLayout, best: &[f64], i: usize) -> Vec<f64> {
    let ng = L.ng;
    let mut pi = vec![0.0; ng];
    if data.prior[i] != 0 {
        pi[(data.prior[i] - 1) as usize] = 1.0;
        return pi;
    }
    if L.nprob > 0 {
        let mut xprob = vec![0.0; L.nvarprob];
        let mut l = 0;
        for k in 0..L.nv {
            if spec.idprob[k] == 1 {
                xprob[l] = data.x_at(i, 0, k);
                l += 1;
            }
        }
        let mut temp = 0.0;
        for g in 1..ng {
            let mut eta = 0.0;
            for k in 0..L.nvarprob {
                eta += best[k * (ng - 1) + (g - 1)] * xprob[k];
            }
            pi[g - 1] = eta.exp();
            temp += eta.exp();
        }
        pi[ng - 1] = 1.0 / (1.0 + temp);
        for g in 1..ng {
            pi[g - 1] *= pi[ng - 1];
        }
    } else {
        for v in pi.iter_mut() { *v = 1.0; }
    }
    for g in 0..ng {
        pi[g] *= data.pprior[i * ng + g];
    }
    pi
}

fn class_mean(spec: &ModelSpec, data: &LongData, L: &ParamLayout, best: &[f64], i: usize, ni: usize, class1: usize) -> Vec<f64> {
    let mut mu = vec![0.0; ni];
    let mut nmoins = 0_isize;
    for k in 0..L.nv {
        match spec.idg[k] {
            1 => {
                let beta = best[L.nprob + nmoins as usize];
                for j in 0..ni { mu[j] += beta * data.x_at(i, j, k); }
                nmoins += 1;
            }
            2 => {
                let beta = best[L.nprob + nmoins as usize + (class1 - 1)];
                for j in 0..ni { mu[j] += beta * data.x_at(i, j, k); }
                nmoins += L.ng as isize;
            }
            _ => {}
        }
    }
    mu
}

fn matvec(a: &[f64], x: &[f64], ni: usize) -> Vec<f64> {
    let mut y = vec![0.0; ni];
    for i in 0..ni {
        let mut s = 0.0;
        for j in 0..ni { s += a[i * ni + j] * x[j]; }
        y[i] = s;
    }
    y
}

// =====================================================================
// predictY — out-of-sample class-conditional predicted mean trajectories.
// Port of R/predictY.hlme.R (marginal-mean-only branch, draws=FALSE).
// =====================================================================

/// Predicted class-conditional mean trajectories for `newdata`.
///
/// `newdata_x` is a row-major `n_new × nv` matrix using the same column
/// ordering as the original fit's design matrix. Returns `ng × n_new`
/// predicted means (class-major: result[g*n_new + r]).
pub fn predict_y(spec: &ModelSpec, L: &ParamLayout, best: &[f64], newdata_x: &[f64], n_new: usize) -> Vec<f64> {
    let ng = L.ng;
    let mut out = vec![0.0; ng * n_new];
    for g in 1..=ng {
        for r in 0..n_new {
            let mut mu = 0.0;
            let mut nmoins = 0_isize;
            for k in 0..L.nv {
                match spec.idg[k] {
                    1 => {
                        let beta = best[L.nprob + nmoins as usize];
                        mu += beta * newdata_x[r * L.nv + k];
                        nmoins += 1;
                    }
                    2 => {
                        let beta = best[L.nprob + nmoins as usize + (g - 1)];
                        mu += beta * newdata_x[r * L.nv + k];
                        nmoins += L.ng as isize;
                    }
                    _ => {}
                }
            }
            out[(g - 1) * n_new + r] = mu;
        }
    }
    out
}
