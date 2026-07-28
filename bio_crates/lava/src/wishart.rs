//! Simulation-based inference — faithful port of `R/wishart.R` and the
//! non-central Wishart sampler inside `matrixsampling::rwishart`.
//!
//! The non-central Wishart draw for `rwishart(n, nu, Sigma, Theta)` has
//! `E[W] = nu·Sigma + Theta`. For integer `nu` (always the case in LAVA, where
//! `nu = K`, the PC count) and `p ≥ 2` the matrixsampling branches are:
//!   - `nu > 2p−1`:  `W = (Θʰ+ΣʰZ)(Θʰ+ΣʰZ)ᵀ + Σʰ·B·Bᵀ·Σʰ`
//!   - `nu == p`:    `W = (Θʰ+ΣʰZ)(Θʰ+ΣʰZ)ᵀ`
//!   - else:         `W = (Θʰ+ΣʰZ)(Θʰ+ΣʰZ)ᵀ + Σʰ·Y·Yᵀ·Σʰ`
//!
//! with `Σʰ, Θʰ` the symmetric matrix square roots (eigendecomposition),
//! `Z` a p×p standard-normal matrix, `B` a lower-triangular Bartlett factor
//! (`diagₖ = √χ²(nu−p−k+1)`, lower off-diagonal N(0,1)), and `Y` a
//! p×(nu−p) standard-normal matrix.

use faer::{Mat, MatRef, Side};
use rand::Rng;
use rand_distr::{ChiSquared, Distribution, Normal};

use crate::stats::{pnorm, pnorm_sf};

/// Symmetric matrix square root via eigendecomposition (`U·diag(√λ)·Uᵀ`),
/// matching `matrixsampling::matrixroot` (symmetric = TRUE).
pub fn symroot(m: MatRef<f64>) -> Mat<f64> {
    let n = m.nrows();
    let e = m
        .self_adjoint_eigen(Side::Lower)
        .expect("symroot: eigen failed");
    let s = e.S();
    let u = e.U();
    let sv = s.column_vector();
    // U · diag(√λ) · Uᵀ, computed as U · (diag·Uᵀ):
    // tmp[i][j] = sqrt(λ_i) * U[j][i]
    let mut tmp = Mat::zeros(n, n);
    for i in 0..n {
        let sl = sv[i].max(0.0).sqrt();
        for j in 0..n {
            tmp[(i, j)] = sl * u[(j, i)];
        }
    }
    u.as_ref() * &tmp
}

#[derive(Debug, Clone, Copy)]
enum Branch {
    /// nu > 2p−1
    Full,
    /// nu == p
    EqP,
    /// integer nu, p < nu < 2p−1
    Mid,
}

/// Precomputed context for sampling `W ~ W_p(nu, Sigma, Theta)`.
pub struct WishartCtx {
    pub nu: usize,
    pub p: usize,
    sigma_root: Mat<f64>,
    theta_root: Mat<f64>,
    branch: Branch,
    /// nu − p (extra df for the central part; 0 when nu == p)
    nu_minus_p: usize,
}

impl WishartCtx {
    pub fn new(nu: usize, sigma: MatRef<f64>, theta: MatRef<f64>) -> Self {
        let p = sigma.nrows();
        let sigma_root = symroot(sigma);
        let theta_root = symroot(theta);
        let branch = if nu > 2 * p - 1 {
            Branch::Full
        } else if nu == p {
            Branch::EqP
        } else {
            Branch::Mid
        };
        Self {
            nu,
            p,
            sigma_root,
            theta_root,
            branch,
            nu_minus_p: nu.saturating_sub(p),
        }
    }

    /// Produce one draw into the provided `out` (p×p).
    pub fn draw_into<R: Rng>(&self, rng: &mut R, out: &mut Mat<f64>) {
        let p = self.p;
        let n01 = Normal::new(0.0, 1.0).unwrap();
        let randn = |r: &mut R| -> f64 { n01.sample(r) };
        // Z: p×p N(0,1)
        let mut z = Mat::zeros(p, p);
        for i in 0..p {
            for j in 0..p {
                z[(i, j)] = randn(rng);
            }
        }
        // A = theta_root + sigma_root·Z  (p×p)
        let mut a = Mat::zeros(p, p);
        let sz = &self.sigma_root * &z;
        for i in 0..p {
            for j in 0..p {
                a[(i, j)] = self.theta_root[(i, j)] + sz[(i, j)];
            }
        }
        // W = A·Aᵀ
        for i in 0..p {
            for j in 0..p {
                let mut s = 0.0;
                for k in 0..p {
                    s += a[(i, k)] * a[(j, k)];
                }
                out[(i, j)] = s;
            }
        }
        // add central part for Full / Mid branches
        match self.branch {
            Branch::EqP => {}
            Branch::Full => {
                // B: lower-tri Bartlett, df = nu-p; diag[k]=sqrt(chisq(nu-p-k+1))
                let mut b = Mat::zeros(p, p);
                for k in 0..p {
                    let df = (self.nu_minus_p - k) as f64;
                    let chi = sample_chisq(rng, df);
                    b[(k, k)] = chi.max(0.0).sqrt();
                    for r in (k + 1)..p {
                        b[(r, k)] = randn(rng);
                    }
                }
                // central = sigma_root · B · Bᵀ · sigma_root
                // SB = sigma_root·B
                let mut sb = Mat::zeros(p, p);
                for i in 0..p {
                    for j in 0..p {
                        let mut s = 0.0;
                        for k in 0..p {
                            s += self.sigma_root[(i, k)] * b[(k, j)];
                        }
                        sb[(i, j)] = s;
                    }
                }
                for i in 0..p {
                    for j in 0..p {
                        let mut s = 0.0;
                        for k in 0..p {
                            s += sb[(i, k)] * sb[(j, k)];
                        }
                        out[(i, j)] += s;
                    }
                }
            }
            Branch::Mid => {
                // Y: p×(nu-p) N(0,1); central = sigma_root·Y·Yᵀ·sigma_root
                let q = self.nu_minus_p;
                // SY = sigma_root·Y  (p×q)
                let mut sy = Mat::zeros(p, q);
                for i in 0..p {
                    for j in 0..q {
                        let y = randn(rng);
                        let mut s = 0.0;
                        for k in 0..p {
                            s += self.sigma_root[(i, k)] * y;
                        }
                        sy[(i, j)] = s;
                    }
                }
                for i in 0..p {
                    for j in 0..p {
                        let mut s = 0.0;
                        for k in 0..q {
                            s += sy[(i, k)] * sy[(j, k)];
                        }
                        out[(i, j)] += s;
                    }
                }
            }
        }
    }
}

fn sample_chisq<R: Rng>(rng: &mut R, df: f64) -> f64 {
    if df <= 0.0 {
        return 0.0;
    }
    let d = ChiSquared::new(df).unwrap_or_else(|_| ChiSquared::new(1.0).unwrap());
    d.sample(rng)
}

/// `conditional.norm(obs, means, sds)`: two-tailed normal tail probability,
/// averaged over draws. Faithful port. Returns the mean of
/// `P(N(m,s) > |obs|) + P(N(m,s) < -|obs|)` with `na.rm`.
pub fn conditional_norm<R: Rng, F: FnMut(&mut R) -> Option<(f64, f64)>>(
    obs: f64,
    n_iter: usize,
    mut mean_sd: F,
    rng: &mut R,
) -> f64 {
    let obs = obs.abs();
    let mut sum = 0.0;
    let mut count = 0usize;
    for _ in 0..n_iter {
        let (m, sd) = match mean_sd(rng) {
            Some(v) => v,
            None => continue,
        };
        if sd.is_nan() || sd <= 0.0 || m.is_nan() {
            continue;
        }
        let z_hi = (obs - m) / sd;
        let z_lo = (-obs - m) / sd;
        let prob = pnorm_sf(z_hi) + pnorm(z_lo);
        sum += prob;
        count += 1;
    }
    if count == 0 {
        f64::NAN
    } else {
        sum / count as f64
    }
}

// ---------------------------------------------------------------------------
// bivariate.integral
// ---------------------------------------------------------------------------

/// Per-draw statistic for the bivariate integral: returns `(m, v)` or `None`
/// when `v <= 0`. `draw` is the 3×3 Wishart draw (0-indexed).
fn bivar_cond_stats(
    draw: &Mat<f64>,
    k: f64,
    sig_xy: f64,
    sig_xys: f64,
    var_y: f64,
) -> Option<(f64, f64)> {
    let m = draw[(1, 2)] + draw[(0, 2)] + sig_xys * (draw[(0, 1)] + draw[(0, 0)]);
    let m = m / k - sig_xy;
    let v = var_y * (draw[(1, 1)] + 2.0 * draw[(0, 1)] + draw[(0, 0)]) / (k * k);
    if v <= 0.0 { None } else { Some((m, v.sqrt())) }
}

/// `bivariate.integral` (single direction, add.reverse = FALSE). `omega`,
/// `sigma` are 2×2.
pub fn bivariate_integral_one<R: Rng>(
    k: usize,
    omega: &Mat<f64>,
    sigma: &Mat<f64>,
    n_iter: usize,
    rng: &mut R,
) -> f64 {
    // omega.null = diag(diag(omega))
    let mut omega_null = Mat::zeros(2, 2);
    omega_null[(0, 0)] = omega[(0, 0)];
    omega_null[(1, 1)] = omega[(1, 1)];
    // sig.use: 3×3, [0,0]=sigma[0,0]
    let mut sig_use = Mat::zeros(3, 3);
    sig_use[(0, 0)] = sigma[(0, 0)];
    // theta: 3×3, theta[1..3,1..3] = omega.null * K
    let mut theta = Mat::zeros(3, 3);
    let kf = k as f64;
    theta[(1, 1)] = omega_null[(0, 0)] * kf;
    theta[(1, 2)] = omega_null[(0, 1)] * kf;
    theta[(2, 1)] = omega_null[(1, 0)] * kf;
    theta[(2, 2)] = omega_null[(1, 1)] * kf;

    let sig_xy = sigma[(0, 1)];
    let sig_xys = sig_xy / sigma[(0, 0)];
    let var_y = sigma[(1, 1)] - sigma[(0, 1)].powi(2) / sigma[(0, 0)];

    let ctx = WishartCtx::new(k, sig_use.as_ref(), theta.as_ref());
    let mut draw = Mat::zeros(3, 3);
    let obs = omega[(0, 1)];
    conditional_norm(
        obs,
        n_iter,
        |rng| {
            ctx.draw_into(rng, &mut draw);
            bivar_cond_stats(&draw, kf, sig_xy, sig_xys, var_y)
        },
        rng,
    )
}

/// `bivariate.integral` with add.reverse (both directions averaged).
pub fn bivariate_integral<R: Rng>(
    k: usize,
    omega: &Mat<f64>,
    sigma: &Mat<f64>,
    n_iter: usize,
    rng: &mut R,
) -> f64 {
    let half = n_iter / 2;
    let p1 = bivariate_integral_one(k, omega, sigma, half.max(1), rng);
    // reversed: omega[2:1,2:1] = swap rows/cols 0,1
    let mut omega_r = Mat::zeros(2, 2);
    let mut sigma_r = Mat::zeros(2, 2);
    for i in 0..2 {
        for j in 0..2 {
            omega_r[(i, j)] = omega[(1 - i, 1 - j)];
            sigma_r[(i, j)] = sigma[(1 - i, 1 - j)];
        }
    }
    let p2 = bivariate_integral_one(k, &omega_r, &sigma_r, half.max(1), rng);
    (p1 + p2) / 2.0
}

// ---------------------------------------------------------------------------
// multivariate.integral  (multiple regression p-values)
// ---------------------------------------------------------------------------

fn solve_spd(a: MatRef<f64>) -> Option<Mat<f64>> {
    use faer::linalg::solvers::{DenseSolveCore, Llt};
    let llt = Llt::new(a, Side::Lower).ok()?;
    Some(llt.inverse())
}

fn solve_mat(a: MatRef<f64>, b: MatRef<f64>) -> Option<Mat<f64>> {
    use faer::linalg::solvers::{Llt, Solve};
    if a.nrows() != a.ncols() {
        return None;
    }
    let llt = Llt::new(a, Side::Lower).ok()?;
    Some(llt.solve(&b.to_owned()))
}

/// `multivariate.integral`. Returns per-predictor p-values (length Px = P−1).
pub fn multivariate_integral<R: Rng>(
    k: usize,
    omega: &Mat<f64>,
    sigma: &Mat<f64>,
    n_iter: usize,
    rng: &mut R,
) -> Vec<f64> {
    let p = omega.nrows();
    let px = p - 1;
    let kf = k as f64;
    // omega.x = omega[0..Px, 0..Px]; omega.xy = omega[0..Px, P-1]
    let omega_x = omega.submatrix(0, 0, px, px);
    let omega_xy: Vec<f64> = (0..px).map(|i| omega[(i, p - 1)]).collect();
    // sig.xys = solve(sigma[0..Px,0..Px]) · sigma[0..Px, P-1] / K
    let sig_xx = sigma.submatrix(0, 0, px, px);
    let b = Mat::from_fn(px, 1, |i, _| sigma[(i, p - 1)]);
    let sig_xys_mat = match solve_mat(sig_xx.as_ref(), b.as_ref()) {
        Some(m) => m,
        None => return vec![f64::NAN; px],
    };
    let sig_xys: Vec<f64> = (0..px).map(|i| sig_xys_mat[(i, 0)] / kf).collect();
    // var.y = (sigma[P,P] - sigma[P,0..Px]·inv·sigma[0..Px,P]) / K²
    let srow = Mat::from_fn(1, px, |_, j| sigma[(p - 1, j)]);
    let scol = Mat::from_fn(px, 1, |i, _| sigma[(i, p - 1)]);
    let mid = match solve_mat(sig_xx.as_ref(), scol.as_ref()) {
        Some(m) => m,
        None => return vec![f64::NAN; px],
    };
    let mut dot = 0.0;
    for i in 0..px {
        dot += srow[(0, i)] * mid[(i, 0)];
    }
    let var_y = (sigma[(p - 1, p - 1)] - dot) / (kf * kf);
    let _ = srow;

    // sigma.use (P+Px)×(P+Px): [0..Px,0..Px] = sigma[0..Px,0..Px]
    let dim = p + px;
    let mut sigma_use = Mat::zeros(dim, dim);
    for i in 0..px {
        for j in 0..px {
            sigma_use[(i, j)] = sigma[(i, j)];
        }
    }
    // theta: theta[Px..2Px, Px..2Px] = K*omega.x ; theta[dim-1, dim-1] = K
    let mut theta = Mat::zeros(dim, dim);
    for i in 0..px {
        for j in 0..px {
            theta[(px + i, px + j)] = kf * omega_x[(i, j)];
        }
    }
    theta[(dim - 1, dim - 1)] = kf;

    let ctx = WishartCtx::new(k, sigma_use.as_ref(), theta.as_ref());
    // gamma.ss.obs = diag(sqrt(diag(omega.x))) · inv(omega.x) · omega.xy
    let omega_x_inv = match solve_spd(omega_x.as_ref()) {
        Some(m) => m,
        None => return vec![f64::NAN; px],
    };
    let mut gamma_ss_obs = vec![0.0; px];
    for i in 0..px {
        let mut s = 0.0;
        for j in 0..px {
            s += omega_x[(i, i)].sqrt() * omega_x_inv[(i, j)] * omega_xy[j];
        }
        gamma_ss_obs[i] = s;
    }

    // precompute, per index, gamma.null and tau.null
    struct NullModel {
        gamma_null: Vec<f64>, // length px, 0 at `index`
        tau_null_sqrt: f64,
        index: usize,
    }
    let mut nulls: Vec<NullModel> = Vec::with_capacity(px);
    for index in 0..px {
        // sub omega.x without row/col `index`
        let mut sub = Mat::zeros(px - 1, px - 1);
        let mut xy_sub = Vec::with_capacity(px - 1);
        let mut a = 0;
        for i in 0..px {
            if i == index {
                continue;
            }
            xy_sub.push(omega_xy[i]);
            let mut bcol = 0;
            for j in 0..px {
                if j == index {
                    continue;
                }
                sub[(a, bcol)] = omega_x[(i, j)];
                bcol += 1;
            }
            a += 1;
        }
        let sub_inv = match solve_spd(sub.as_ref()) {
            Some(m) => m,
            None => {
                nulls.push(NullModel {
                    gamma_null: vec![f64::NAN; px],
                    tau_null_sqrt: 0.0,
                    index,
                });
                continue;
            }
        };
        // gamma.null[-index] = sub_inv · xy_sub
        let mut gn = vec![0.0; px];
        for i in 0..(px - 1) {
            let mut s = 0.0;
            for j in 0..(px - 1) {
                s += sub_inv[(i, j)] * xy_sub[j];
            }
            // map back to original index
            let orig = if i < index { i } else { i + 1 };
            gn[orig] = s;
        }
        // tau.null = omega[P,P] - xy_sub · sub_inv · xy_sub
        let mut q = 0.0;
        for i in 0..(px - 1) {
            let mut s = 0.0;
            for j in 0..(px - 1) {
                s += sub_inv[(i, j)] * xy_sub[j];
            }
            q += xy_sub[i] * s;
        }
        let tau = omega[(p - 1, p - 1)] - q;
        let tau_sqrt = if tau > 0.0 { tau.sqrt() } else { 0.0 };
        nulls.push(NullModel {
            gamma_null: gn,
            tau_null_sqrt: tau_sqrt,
            index,
        });
    }

    // accumulators for conditional.norm per predictor
    // We accumulate sum and count of prob for each predictor, and need per-draw
    // (M_index, sd_index). Do it inline.
    let mut draw = Mat::zeros(dim, dim);
    let mut prob_sum = vec![0.0f64; px];
    let mut prob_cnt = vec![0usize; px];

    for _ in 0..n_iter {
        ctx.draw_into(rng, &mut draw);
        // i.eps = 0..Px ; i.delta = Px..2Px ; i.y = 2Px (= dim-1)
        // dtd.x[i,j] = draw[eps,delta] sums
        let mut dtd_x = Mat::zeros(px, px);
        for i in 0..px {
            for j in 0..px {
                dtd_x[(i, j)] =
                    draw[(i, j)] + draw[(i, px + j)] + draw[(px + i, j)] + draw[(px + i, px + j)];
            }
        }
        // omega.x.draw = dtd.x/K - sigma[0..Px,0..Px]
        let mut ox = Mat::zeros(px, px);
        for i in 0..px {
            for j in 0..px {
                ox[(i, j)] = dtd_x[(i, j)] / kf - sigma[(i, j)];
            }
        }
        let ox_inv = match solve_spd(ox.as_ref()) {
            Some(m) => m,
            None => continue,
        };
        // O.x = diag(sqrt(diag(ox))) · ox_inv
        let mut ox_diag = vec![0.0f64; px];
        for i in 0..px {
            ox_diag[i] = if ox[(i, i)] > 0.0 {
                ox[(i, i)].sqrt()
            } else {
                0.0
            };
        }
        let mut o_x = Mat::zeros(px, px);
        for i in 0..px {
            for j in 0..px {
                o_x[(i, j)] = ox_diag[i] * ox_inv[(i, j)];
            }
        }
        // sds = sqrt(diag(var.y · O.x · dtd.x · t(O.x)))
        let mut od = Mat::zeros(px, px);
        // temp1 = O.x · dtd.x
        let mut t1 = Mat::zeros(px, px);
        for i in 0..px {
            for j in 0..px {
                let mut s = 0.0;
                for k in 0..px {
                    s += o_x[(i, k)] * dtd_x[(k, j)];
                }
                t1[(i, j)] = s;
            }
        }
        // diag of t1 · t(O.x) = t1 · o_xᵀ ; diag[i] = sum_k t1[i,k]*o_x[i,k]
        for i in 0..px {
            let mut s = 0.0;
            for k in 0..px {
                s += t1[(i, k)] * o_x[(i, k)];
            }
            od[(i, i)] = (var_y * s).max(0.0).sqrt();
        }
        // C1 = (draw[delta,delta]+draw[delta,eps]) · t(O.x) / K   (Px×Px)
        let mut dd_de = Mat::zeros(px, px);
        for i in 0..px {
            for j in 0..px {
                dd_de[(i, j)] = draw[(px + i, px + j)] + draw[(px + i, j)];
            }
        }
        let mut c1 = Mat::zeros(px, px);
        for i in 0..px {
            for j in 0..px {
                let mut s = 0.0;
                for k in 0..px {
                    // dd_de[i,k] * t(O.x)[k,j] = dd_de[i,k]*o_x[j,k]
                    s += dd_de[(i, k)] * o_x[(j, k)];
                }
                c1[(i, j)] = s / kf;
            }
        }
        // C2 = O.x · (draw[delta,y]+draw[eps,y]) / K   (Px)
        let mut c2 = vec![0.0f64; px];
        for i in 0..px {
            let mut s = 0.0;
            for k in 0..px {
                s += o_x[(i, k)] * (draw[(px + k, dim - 1)] + draw[(k, dim - 1)]);
            }
            c2[i] = s / kf;
        }
        // C3 = O.x · ((draw[delta,eps]+draw[eps,eps])·sig.xys - sigma[0..Px,P])  (Px)
        let mut c3 = vec![0.0f64; px];
        for i in 0..px {
            let mut val = 0.0;
            for k in 0..px {
                let mut vk = 0.0;
                for l in 0..px {
                    vk += (draw[(px + k, l)] + draw[(k, l)]) * sig_xys[l];
                }
                vk -= sigma[(k, p - 1)];
                val += o_x[(i, k)] * vk;
            }
            c3[i] = val;
        }

        // per predictor: M = gamma.null·C1[row block index] + tau_null_sqrt·C2[index] + C3[index]
        // C1[1:Px+(index-1)*Px, ] in R = rows (1+(index-1)*Px) .. (Px+(index-1)*Px) → 0-indexed rows (index-1)*Px .. index*Px-1; i.e. block column `index`?
        // R: C1[1:Px+(index-1)*Px, ] selects rows 1+(index-1)*Px .. Px+(index-1)*Px of C1 (Px rows) = the (index)-th Px-row block. Since C1 is Px×Px and stored col-major in R... Actually C1 is Px×Px; 1:Px+(index-1)*Px are row indices [1..Px] shifted by (index-1)*Px. For a Px×Px matrix that exceeds Px rows unless interpreted via vector layout. The R code treats C1 param as a flat vector (param[1:(Px^2),] = C1 column-major). So C1[1:Px+(index-1)*Px, ] = column `index` of C1 (1-indexed) in column-major. So it's C1[:, index] (0-indexed column index).
        for nm in &nulls {
            let index = nm.index;
            // M[j] = sum over k: gamma_null[k] * C1[k, index]   (C1 column `index`)
            //      + tau_null_sqrt * C2[index]   + C3[index]  (but these are per-draw vectors; M is a vector over draws)
            // Actually M is the mean vector for THIS draw: M = gamma.null·C1[:,index] + tau_null_sqrt·C2[index] + C3[index]
            // gamma.null·C1[:,index] is a scalar? No: gamma.null is 1×Px, C1[:,index] is Px×1 → scalar (dot).
            // Wait in R: M = gamma.null %*% C1[1:Px+(index-1)*Px,] + tau.null.sqrt*C2[index,] + C3[index,]
            // gamma.null (1×Px) %*% C1_col (Px×n_iter) → (1×n_iter). So M is per-draw scalar = dot(gamma.null, C1[:,index for this draw]).
            // For a single draw, C1 is Px×1 (just this draw's column). So M_scalar = dot(gamma_null, c1[:,index]) + tau_null_sqrt*c2[index] + c3[index].
            let mut m = 0.0;
            for k in 0..px {
                m += nm.gamma_null[k] * c1[(k, index)];
            }
            m += nm.tau_null_sqrt * c2[index] + c3[index];
            let sd = od[(index, index)];
            if sd.is_nan() || sd <= 0.0 || m.is_nan() {
                continue;
            }
            let obs = gamma_ss_obs[index].abs();
            let z_hi = (obs - m) / sd;
            let z_lo = (-obs - m) / sd;
            let prob = pnorm_sf(z_hi) + pnorm(z_lo);
            prob_sum[index] += prob;
            prob_cnt[index] += 1;
        }
    }

    (0..px)
        .map(|i| {
            if prob_cnt[i] == 0 {
                f64::NAN
            } else {
                prob_sum[i] / prob_cnt[i] as f64
            }
        })
        .collect()
}

/// Adaptive p-value driver — `integral.p`. Runs `min.iter` draws, bumping to
/// 1e5 / 1e6 as the p-value falls below the `adap.thresh` thresholds.
pub fn integral_p<R: Rng, F: FnMut(&mut R, usize) -> f64>(
    min_iter: usize,
    adap_thresh: &[f64],
    rng: &mut R,
    mut func: F,
) -> f64 {
    let mut tot_iter: Vec<usize> = Vec::with_capacity(adap_thresh.len() + 1);
    let mut cur = min_iter;
    tot_iter.push(cur);
    for _ in adap_thresh {
        cur *= 10;
        tot_iter.push(cur);
    }
    let mut thresh = adap_thresh.to_vec();
    thresh.push(0.0);

    let mut p = 1.0;
    let mut curr_iter = 0usize;
    for i in 0..tot_iter.len() {
        let add = tot_iter[i].saturating_sub(curr_iter);
        let add_p = func(rng, add);
        p = (curr_iter as f64 * p + add as f64 * add_p) / tot_iter[i] as f64;
        curr_iter = tot_iter[i];
        if add_p.is_nan() || add_p >= thresh[i] {
            break;
        }
    }
    p
}

// ---------------------------------------------------------------------------
// pcov.integral  (partial-correlation p-value)
// ---------------------------------------------------------------------------

/// `pcov.integral` (single direction). `omega`, `sigma` are the full P×P
/// matrices; the phenotype pair is `(x, y)` and `z` are the conditioners. The
/// matrices are internally reordered to `[z..., x, y]`.
pub fn pcov_integral_one<R: Rng>(
    k: usize,
    omega: &Mat<f64>,
    sigma: &Mat<f64>,
    xy: (usize, usize),
    z: &[usize],
    n_iter: usize,
    rng: &mut R,
) -> f64 {
    let mut idx: Vec<usize> = z.to_vec();
    idx.push(xy.0);
    idx.push(xy.1);
    let mut omega = subm(omega, &idx);
    let sigma = subm(sigma, &idx);
    let p = idx.len(); // reordered dimension = |z| + 2
    let pw = p - 1;
    let pz = pw - 1;
    let kf = k as f64;

    // fit.x, fit.y (z = 0..pz, x = pw-1, y = p-1 in the reordered matrix)
    let zr: Vec<usize> = (0..pz).collect();
    let fit_x = regress_fit(&omega, pw - 1, &zr);
    let fit_y = regress_fit(&omega, p - 1, &zr);
    if omega[(pw - 1, pw - 1)] <= fit_x {
        omega[(pw - 1, pw - 1)] = fit_x / 0.99999;
    }
    let zw: Vec<usize> = (0..pw).collect(); // z + x
    let fit_xy = regress_fit(&omega, p - 1, &zw);
    if omega[(p - 1, p - 1)] <= fit_xy {
        omega[(p - 1, p - 1)] = fit_xy / 0.99999;
    }

    let var_y = {
        let inv = solve_spd(subm(&sigma, &zw).as_ref()).expect("sigma z+x singular");
        let srow = Mat::from_fn(1, pw, |_, j| sigma[(p - 1, zw[j])]);
        let mut dot = 0.0;
        for i in 0..pw {
            let mut s = 0.0;
            for j in 0..pw {
                s += inv[(i, j)] * sigma[(zw[j], p - 1)];
            }
            dot += srow[(0, i)] * s;
        }
        (sigma[(p - 1, p - 1)] - dot) / (kf * kf)
    };
    let sig_xys = {
        let _inv = solve_spd(subm(&sigma, &zw).as_ref()).expect("sigma z+x singular");
        let col = Mat::from_fn(pw, 1, |i, _| sigma[(zw[i], p - 1)]);
        let sol = solve_mat(subm(&sigma, &zw).as_ref(), col.as_ref()).expect("sigma z+x singular");
        (0..pw).map(|i| sol[(i, 0)] / kf).collect::<Vec<_>>()
    };

    // gamma parts
    let oz_inv = match solve_spd(subm(&omega, &zr).as_ref()) {
        Some(m) => m,
        None => return f64::NAN,
    };
    let gamma_x: Vec<f64> = (0..pz)
        .map(|i| {
            let mut s = 0.0;
            for j in 0..pz {
                s += oz_inv[(i, j)] * omega[(zr[j], pw - 1)];
            }
            s
        })
        .collect();
    let gamma_y: Vec<f64> = (0..pz)
        .map(|i| {
            let mut s = 0.0;
            for j in 0..pz {
                s += oz_inv[(i, j)] * omega[(zr[j], p - 1)];
            }
            s
        })
        .collect();
    // dw.dz.gamma = c(omega[z,y], omega[z,x]·inv·omega[z,y]) * K   (length Pz+1)
    let mut dw_dz_gamma = vec![0.0; pz + 1];
    for i in 0..pz {
        dw_dz_gamma[i] = omega[(zr[i], p - 1)] * kf;
    }
    {
        let mut s = 0.0;
        for i in 0..pz {
            s += omega[(zr[i], pw - 1)] * gamma_y[i];
        }
        dw_dz_gamma[pz] = s * kf;
    }
    let gamma_fit_x = fit_x * kf;

    // sigma.use (dim = P+Pw = 2p-1), sigma.use[0..Pw,0..Pw] = sigma[0..Pw,0..Pw]
    let dim = p + pw;
    let mut sigma_use = Mat::zeros(dim, dim);
    for i in 0..pw {
        for j in 0..pw {
            sigma_use[(i, j)] = sigma[(i, j)];
        }
    }
    // theta
    let mut theta = Mat::zeros(dim, dim);
    // z block at theta[pw .. pw+pz-1, same] = K·omega[z,z]
    for i in 0..pz {
        for j in 0..pz {
            theta[(pw + i, pw + j)] = kf * omega[(zr[i], zr[j])];
        }
    }
    // diag overrides: 1-indexed diag[2Pw]=K*(omega[x,x]-fit.x), diag[2Pw-1]=K*(omega[y,y]-fit.y)
    // 0-indexed positions (2Pw-1) and (2Pw-2)
    theta[(2 * pw - 1, 2 * pw - 1)] = kf * (omega[(pw - 1, pw - 1)] - fit_x);
    theta[(2 * pw - 2, 2 * pw - 2)] = kf * (omega[(p - 1, p - 1)] - fit_y);

    let ctx = WishartCtx::new(k, sigma_use.as_ref(), theta.as_ref());

    // pcov.obs
    let pcov_obs = {
        let mut s = 0.0;
        for i in 0..pz {
            s += omega[(zr[i], pw - 1)] * gamma_y[i];
        }
        omega[(pw - 1, p - 1)] - s
    };

    let mut draw = Mat::zeros(dim, dim);
    conditional_norm(
        pcov_obs,
        n_iter,
        |rng| {
            pcov_cond_stats(
                &mut draw,
                rng,
                &ctx,
                k,
                &sigma,
                &gamma_x,
                &gamma_y,
                gamma_fit_x,
                &dw_dz_gamma,
                &sig_xys,
                var_y,
                p,
            )
        },
        rng,
    )
}

#[allow(clippy::too_many_arguments)] // faithful port of R's pcov.integral per-draw stats
fn pcov_cond_stats<R: Rng>(
    draw: &mut Mat<f64>,
    rng: &mut R,
    ctx: &WishartCtx,
    k: usize,
    sigma: &Mat<f64>,
    gamma_x: &[f64],
    gamma_y: &[f64],
    gamma_fit_x: f64,
    dw_dz_gamma: &[f64],
    sig_xys: &[f64],
    var_y: f64,
    p: usize,
) -> Option<(f64, f64)> {
    let pw = p - 1;
    let pz = pw - 1;
    let kf = k as f64;
    let dim = ctx.p; // = 2p-1
    ctx.draw_into(rng, draw);
    // index sets (0-indexed): eps=0..pw, eps.z=0..pz, eps.x=pw-1, delta=pw..2pw, delta.z=pw..pw+pz, x=2pw-1, y=2pw-2
    let ieps = |a: usize| a; // 0..pw
    let ide = |a: usize| pw + a; // delta
    let ide_z = |a: usize| pw + a; // delta.z (a in 0..pz)
    let ieps_x = pw - 1;
    let ix = 2 * pw - 1;
    let iy = 2 * pw - 2;

    // dtd.w[a,b] for a,b in 0..pw
    let mut dtd_w = Mat::zeros(pw, pw);
    for a in 0..pw {
        for b in 0..pw {
            dtd_w[(a, b)] = draw[(ieps(a), ieps(b))]
                + draw[(ieps(a), ide(b))]
                + draw[(ide(a), ieps(b))]
                + draw[(ide(a), ide(b))];
        }
    }
    // dtd_w[pw-1,pw-1] += 2·dot(gamma_x, draw[delta.z, ieps_x]) + gamma_fit_x
    {
        let mut dot = 0.0;
        for kk in 0..pz {
            dot += gamma_x[kk] * draw[(ide_z(kk), ieps_x)];
        }
        dtd_w[(pw - 1, pw - 1)] += 2.0 * dot + gamma_fit_x;
    }
    // dtd_w[0..pz, pw-1] += (draw[delta.z,delta.z] + draw[eps.z,delta.z]) · gamma_x
    for a in 0..pz {
        let mut s = 0.0;
        for kk in 0..pz {
            s += (draw[(ide_z(a), ide_z(kk))] + draw[(ieps(a), ide_z(kk))]) * gamma_x[kk];
        }
        dtd_w[(a, pw - 1)] += s;
        dtd_w[(pw - 1, a)] = dtd_w[(a, pw - 1)];
    }
    // omega.w = dtd_w/K - sigma[0..pw,0..pw]
    let mut omega_w = Mat::zeros(pw, pw);
    for i in 0..pw {
        for j in 0..pw {
            omega_w[(i, j)] = dtd_w[(i, j)] / kf - sigma[(i, j)];
        }
    }
    // omega.z.inv = solve(omega.w[0..pz, 0..pz])
    let zr: Vec<usize> = (0..pz).collect();
    let oz_sub = subm(&omega_w, &zr);
    let oz_inv = solve_spd(oz_sub.as_ref())?;
    // b = [-(oz_inv·omega_w[z, pw-1]); 1]
    let mut b = vec![0.0; pw];
    for i in 0..pz {
        let mut s = 0.0;
        for j in 0..pz {
            s += oz_inv[(i, j)] * omega_w[(zr[j], pw - 1)];
        }
        b[i] = -s;
    }
    b[pw - 1] = 1.0;

    // dhw.dy[a] = dw_dz_gamma[a] + dot(draw[eps, delta.z] row a, gamma_y) + draw[eps_a, iy]  (length pw; dw_dz_gamma has pw entries)
    let mut dhw_dy = vec![0.0; pw];
    for a in 0..pw {
        let mut dot = 0.0;
        for kk in 0..pz {
            dot += draw[(ieps(a), ide_z(kk))] * gamma_y[kk];
        }
        dhw_dy[a] = dw_dz_gamma[a] + dot + draw[(ieps(a), iy)];
    }
    // dw.ew[a, col]: rows 0..pz = draw[delta.z_a, eps_col]; row pz = dot(gamma_x, draw[delta.z, eps_col]) + draw[ix, eps_col]
    let mut dw_ew = Mat::zeros(pw, pw);
    for col in 0..pw {
        for a in 0..pz {
            dw_ew[(a, col)] = draw[(ide_z(a), ieps(col))];
        }
        // row pz
        let mut s = 0.0;
        for kk in 0..pz {
            s += gamma_x[kk] * draw[(ide_z(kk), ieps(col))];
        }
        dw_ew[(pz, col)] = s + draw[(ix, ieps(col))];
    }
    // M[a] = dhw.dy[a]/K + sum_col (dw.ew[a,col]+draw[eps_a,eps_col])·sig_xys[col] - sigma[a, pw]
    let mut mm = vec![0.0; pw];
    for a in 0..pw {
        let mut s = dhw_dy[a] / kf;
        for col in 0..pw {
            s += (dw_ew[(a, col)] + draw[(ieps(a), ieps(col))]) * sig_xys[col];
        }
        s -= sigma[(a, pw)];
        mm[a] = s;
    }
    // V = b·dtd_w·b · var_y ;  Mstat = b·M
    let mut mstat = 0.0;
    for a in 0..pw {
        mstat += b[a] * mm[a];
    }
    // b·dtd_w·b
    let mut v = 0.0;
    for a in 0..pw {
        let mut row = 0.0;
        for c in 0..pw {
            row += dtd_w[(a, c)] * b[c];
        }
        v += b[a] * row;
    }
    v *= var_y;
    let sd = if v >= 0.0 { v.sqrt() } else { return None };
    let _ = dim;
    Some((mstat, sd))
}

fn subm(m: &Mat<f64>, idx: &[usize]) -> Mat<f64> {
    let n = idx.len();
    Mat::from_fn(n, n, |i, j| m[(idx[i], idx[j])])
}

fn regress_fit(omega: &Mat<f64>, y: usize, x: &[usize]) -> f64 {
    // omega[x,y]·inv(omega[x,x])·omega[x,y]
    let nx = x.len();
    let sub = Mat::from_fn(nx, nx, |i, j| omega[(x[i], x[j])]);
    let inv = match solve_spd(sub.as_ref()) {
        Some(m) => m,
        None => return f64::NAN,
    };
    let mut dot = 0.0;
    for i in 0..nx {
        let mut s = 0.0;
        for j in 0..nx {
            s += inv[(i, j)] * omega[(x[j], y)];
        }
        dot += omega[(x[i], y)] * s;
    }
    dot
}

/// `pcov.integral` with add.reverse (both xy orderings averaged).
pub fn pcov_integral<R: Rng>(
    k: usize,
    omega: &Mat<f64>,
    sigma: &Mat<f64>,
    xy: (usize, usize),
    z: &[usize],
    n_iter: usize,
    rng: &mut R,
) -> f64 {
    let half = (n_iter / 2).max(1);
    let p1 = pcov_integral_one(k, omega, sigma, xy, z, half, rng);
    let p2 = pcov_integral_one(k, omega, sigma, (xy.1, xy.0), z, half, rng);
    (p1 + p2) / 2.0
}
