//! Rust port of `marqLevAlg::mla` — the Marquardt-Levenberg optimizer used by
//! `lcmm::hlme`.
//!
//! Faithful port of `marqLevAlg/R/marqLevAlg.R` (v2.0.8) plus its helper files
//! `deriva.R`, `func1.R`, `ghg.R`, `searpas.R`, `valfpa.R`. Packed-matrix
//! Fortran subroutines `dsinv`/`dchole` are replaced with the equivalent
//! Cholesky-based linear algebra in [`crate::likelihood::invert_pd`] and a
//! damped-Cholesky solve, which gives the same result to machine precision.
//!
//! ## Convergence criteria (Philipps et al. 2021, R Journal 13(2))
//! * `ca` (epsa) — parameter stability: `sum(delta²)`
//! * `cb` (epsb) — objective stability: `|rl_new − rl_old|`
//! * `dd` (epsd) — relative distance to maximum: `g' H⁻¹ g / m`
//!
//! Optimization stops when all three fall below their thresholds.

use crate::likelihood::invert_pd;
use crate::LcmmError;

/// Controls for [`marq_lev_alg`].
#[derive(Clone, Debug)]
pub struct MlaControl {
    pub maxiter: usize,
    pub epsa: f64,
    pub epsb: f64,
    pub epsd: f64,
    /// We always maximize (`minimize=false` in lcmm usage).
    pub minimize: bool,
    pub blinding: bool,
    pub multiple_try: i32,
    pub verbose: bool,
    /// Indices (1-based) into the free-parameter vector dropped from the
    /// Hessian when computing the RDM (matches R `partialH`). Empty for hlme.
    pub partialH: Vec<usize>,
}

/// Optimization result.
#[derive(Clone, Debug)]
pub struct MlaResult {
    pub best: Vec<f64>,
    /// Packed upper-triangular variance-covariance (length m*(m+1)/2).
    pub v: Vec<f64>,
    pub fn_value: f64,
    pub istop: i32,
    pub niter: usize,
    pub gconv: [f64; 3],
}

/// Objective trait: the optimizer only needs to evaluate `f(b)`.
pub trait Objective {
    fn eval(&self, b: &[f64]) -> f64;
}

impl<F: Fn(&[f64]) -> f64> Objective for F {
    fn eval(&self, b: &[f64]) -> f64 {
        self(b)
    }
}

/// Run Marquardt-Levenberg maximization. Mirrors `marqLevAlg.R` line-for-line.
pub fn marq_lev_alg<O: Objective>(
    obj: &O,
    b0: &[f64],
    ctrl: &MlaControl,
) -> Result<MlaResult, LcmmError> {
    let m = b0.len();
    // For maximize (minimize=false) we evaluate loglik directly (sign=+1).
    // deriva computes V = −∂²f and grad = ∇f, and we solve V·delta = grad,
    // giving the Newton step delta = V⁻¹·grad = (−H)⁻¹·∇f = −H⁻¹·∇f.  For
    // minimize (sign=−1) the same formula applies to −f.
    let sign: f64 = if ctrl.minimize { -1.0 } else { 1.0 };
    let f = |b: &[f64]| -> f64 { sign * obj.eval(b) };

    let nfmax = m * (m + 1) / 2;
    let th = 1e-5_f64;
    let ep = 1e-20_f64;

    let mut b = b0.to_vec();
    let mut ca = ctrl.epsa + 1.0;
    let mut cb = ctrl.epsb + 1.0;
    let mut dd = ctrl.epsd + 1.0;
    let mut rl1 = -1e10_f64;
    let mut ni = 0_usize;
    let mut istop = 0_i32;
    let mut da = 1e-2_f64;
    let dm = 5.0_f64;
    let mut old_dd = dd;
    let mut old_b = b.clone();
    let mut old_rl: f64 = 0.0;
    let mut old_ca = 1.0_f64;
    let mut old_cb = 1.0_f64;
    let gonflencountmax = 10_i32;
    let mut v_final: Vec<f64> = vec![0.0; m * (m + 1) / 2];

    loop {
        // Infinite-parameter guard (Fortran/R lines 244–258).
        if b.iter().any(|&x| !x.is_finite()) {
            if ctrl.verbose {
                eprintln!(
                    "mla: infinite parameters. last b={:?}", old_b.iter().take(8).collect::<Vec<_>>()
                );
            }
            istop = 4;
            rl1 = -1e9;
            break;
        }

        // Numerical derivatives → packed Hessian (upper) + gradient.
        let (mut v_packed, mut rl) = deriva(&f, &b, m);
        // rl is the *maximization* objective (sign already applied).

        // multipleTry on the first iteration if rl is not finite.
        if ctrl.multiple_try > 1 && ni == 0 && !rl.is_finite() {
            let mut kk = 0;
            while kk < ctrl.multiple_try && !rl.is_finite() {
                kk += 1;
                for x in b.iter_mut() {
                    *x *= 0.5;
                }
                let (v2, r2) = deriva(&f, &b, m);
                v_final = v2[..m * (m + 1) / 2].to_vec();
                v_packed = v2;
                rl = r2;
            }
        }

        // Finite b but infinite function value → abort.
        if b.iter().all(|&x| x.is_finite()) && !rl.is_finite() {
            if ctrl.verbose {
                eprintln!("mla: infinite function value with finite parameters");
            }
            istop = 4;
            rl1 = -1e9;
            break;
        }

        rl1 = rl;
        dd = 0.0;

        // Invert the Hessian (packed upper) to compute RDM. If singular, dd is
        // set above threshold to force another iteration.
        let fu_int: Vec<f64> = v_packed[..nfmax].to_vec();
        let (hinv_packed, ier) = match invert_pd(&sym_to_full(&fu_int, m), m) {
            Some((full_inv, _logdet)) => (pack_upper(&full_inv, m), 0_i32),
            None => (fu_int.clone(), -1_i32),
        };

        if ier != -1 {
            let grad = &v_packed[nfmax..nfmax + m];
            dd = ghg(&hinv_packed, grad, m) / m as f64;
            if dd.is_nan() {
                dd = ctrl.epsd + 1.0;
            }
        } else {
            dd = ctrl.epsd + 1.0;
        }

        if ctrl.verbose {
            eprintln!(
                "mla iter {ni}: rl={rl:.6} ca={ca:.3e} cb={cb:.3e} dd={dd:.3e}"
            );
        }

        old_b = b.clone();
        old_rl = rl;
        old_ca = ca;
        old_cb = cb;
        if dd <= old_dd {
            old_dd = dd;
        }
        if ca < ctrl.epsa && cb < ctrl.epsb && dd < ctrl.epsd {
            v_final = hinv_packed.clone();
            break;
        }

        // ---- Compute Marquardt-damped step --------------------------------
        let _tr: f64 = (0..m).map(|i| v_packed[i * (i + 1) / 2 / 1 + 0].abs()).sum::<f64>() / m as f64;
        // NB: Fortran `tr = sum(|v[ii]|)/m` where ii = i*(i+1)/2 (1-indexed).
        // In 0-indexed packed: ii0 = i*(i+1)/2 for i in 0..m (diagonal entries).
        let tr: f64 = (0..m).map(|i| v_packed[i * (i + 1) / 2].abs()).sum::<f64>() / m as f64;

        let mut ncount = 0_i32;
        let mut ga = 0.01_f64;
        let delta;
        let mut damped_packed_final: Vec<f64> = Vec::new();

        // dchole-equivalent: add Marquardt damping to diagonal, factor, solve.
        let mut idpos;
        let mut damped_packed;
        loop {
            damped_packed = v_packed[..nfmax].to_vec();
            for i in 0..m {
                let ii = i * (i + 1) / 2;
                let diag = damped_packed[ii];
                if diag != 0.0 {
                    damped_packed[ii] = diag + da * ((1.0 - ga) * diag.abs() + ga * tr);
                } else {
                    damped_packed[ii] = da * ga * tr;
                }
            }
            // Solve (H + D) · delta = -grad via Cholesky.
            match solve_damped(&damped_packed, &v_packed[nfmax..nfmax + m], m) {
                Some(d) => {
                    idpos = 0;
                    delta = d;
                    break;
                }
                None => {
                    idpos = 1;
                }
            }
            ncount += 1;
            if ncount <= 3 || ga >= 1.0 {
                da *= dm;
            } else {
                ga *= dm;
                if ga > 1.0 {
                    ga = 1.0;
                }
            }
            if ncount >= gonflencountmax {
                // Damping exhausted: the Hessian is too ill-conditioned to
                // produce a usable step. R's marqLevAlg sets istop=3 (partial
                // H) in this situation. We keep the current b and break.
                istop = 3;
                v_final = damped_packed.clone();
                damped_packed_final = damped_packed.clone();
                delta = vec![0.0; m];
                idpos = 0;
                break;
            }
        }
        let _ = idpos;

        // istop=3: damping exhausted (partial H), break out of main loop.
        if istop == 3 {
            break;
        }

        // ---- Trial step ---------------------------------------------------
        let b1: Vec<f64> = (0..m).map(|i| b[i] + delta[i]).collect();
        let mut rl_new = f(&b1);
        if ctrl.blinding && rl_new.is_nan() {
            rl_new = -500_000.0;
        } else if !ctrl.blinding && rl_new.is_nan() {
            istop = 4;
            rl1 = -1e9;
            break;
        }

        if rl1 < rl_new {
            // Improvement: accept, shrink da.
            if da < th {
                da = th;
            } else {
                da /= dm + 2.0;
            }
            let prev_rl = rl1;
            cb = (prev_rl - rl_new).abs();
            ca = (0..m).map(|i| delta[i] * delta[i]).sum();
            b = b1;
            ni += 1;
            rl1 = rl_new;
            if ni >= ctrl.maxiter {
                istop = 2;
                v_final = damped_packed;
                break;
            }
        } else {
            // No improvement: line-search along delta.
            let maxt = delta.iter().fold(0.0_f64, |a, &x| a.max(x.abs()));
            let vw = if maxt == 0.0 { th } else { th / maxt };
            let step = (1.5_f64).ln();
            let (fi, vw_opt) = searpas(&f, vw, step, &b, &delta);
            let rl_ls = -fi; // valfpa negates funcpa
            if rl_ls == -1e9 {
                istop = 4;
                break;
            }
            let delta_ls: Vec<f64> = (0..m).map(|i| vw_opt * delta[i]).collect();
            let prev_rl = rl1;
            cb = (prev_rl - rl_ls).abs();
            ca = (0..m).map(|i| delta_ls[i] * delta_ls[i]).sum();
            b = (0..m).map(|i| b[i] + delta_ls[i]).collect();
            ni += 1;
            rl1 = rl_ls;
            da = (dm - 3.0) * da;
            if ni >= ctrl.maxiter {
                istop = 2;
                v_final = damped_packed;
                break;
            }
        }
        // Suppress unused-warning while keeping the loop readable.
        let _ = (nfmax, ep, old_ca, old_cb);
    }

    if !(2..=4).contains(&istop) {
        istop = 1;
    }
    let fn_value = sign * rl1;

    Ok(MlaResult {
        best: b,
        v: v_final,
        fn_value,
        istop,
        niter: ni,
        gconv: [old_ca, old_cb, old_dd],
    })
}

// ---- Helpers (faithful ports of the marqLevAlg helper files) ------------

/// Numerical first + second derivatives. Mirrors `deriva.R` sequential branch.
/// Returns `(v, rl)` where `v` has length `m*(m+3)/2`:
/// `v[0..m*(m+1)/2]` = packed upper Hessian, `v[m*(m+1)/2..]` = gradient.
fn deriva<F: Fn(&[f64]) -> f64>(f: &F, b: &[f64], m: usize) -> (Vec<f64>, f64) {
    let rl = f(b);
    let m1 = m * (m + 1) / 2;
    let mut v = vec![0.0; m * (m + 3) / 2];
    let mut fcith = vec![0.0; m];
    let mut fcith2 = vec![0.0; m];

    for i in 0..m {
        let th = step_size(b[i]);
        let mut bh = b.to_vec();
        bh[i] += th;
        fcith[i] = f(&bh);
        bh[i] -= 2.0 * th;
        fcith2[i] = f(&bh);
    }

    let mut k = 0;
    for i in 0..m {
        let thn = -step_size(b[i]);
        v[m1 + i] = -(fcith[i] - fcith2[i]) / (2.0 * thn);
        for j in 0..=i {
            let thi = step_size(b[i]);
            let thj = step_size(b[j]);
            let th = thi * thj;
            let mut bh = b.to_vec();
            bh[i] += thi;
            bh[j] += thj;
            let temp = f(&bh);
            v[k] = -(temp - fcith[j] - fcith[i] + rl) / th;
            k += 1;
        }
    }
    (v, rl)
}

#[inline]
fn step_size(x: f64) -> f64 {
    (1e-7_f64).max(1e-4 * x.abs())
}

/// Relative-distance-to-maximum numerator: `g' H^{-1} g` using packed H^{-1}.
fn ghg(hinv_packed: &[f64], grad: &[f64], m: usize) -> f64 {
    // hinv_packed is the packed upper-tri inverse Hessian (length m*(m+1)/2).
    // g' Hinv g = sum_{i,j} grad[i] * Hinv[i,j] * grad[j]
    let mut s = 0.0;
    for i in 0..m {
        for j in 0..m {
            let (a, b_) = if i <= j { (i, j) } else { (j, i) };
            let idx = b_ * (b_ + 1) / 2 + a;
            s += grad[i] * hinv_packed[idx] * grad[j];
        }
    }
    s
}

/// Brent-style line search along the delta direction. Mirrors `searpas.R` +
/// `valfpa.R`. `f` is the (already sign-adjusted) maximization objective.
/// Returns `(fi, vw)` where `fi` is the *negated* objective at the optimum
/// (so `rl = -fi`), matching R's convention.
fn searpas<F: Fn(&[f64]) -> f64>(
    f: &F,
    vw0: f64,
    step: f64,
    b: &[f64],
    delta: &[f64],
) -> (f64, f64) {
    let m = b.len();
    let valfpa = |vw: f64, b: &[f64], delta: &[f64]| -> f64 {
        if vw.is_nan() {
            return -2e9;
        }
        let bk: Vec<f64> = (0..m).map(|i| b[i] + vw.exp() * delta[i]).collect();
        -f(&bk)
    };

    let vlw1 = vw0.ln();
    let mut vlw2 = vlw1 + step;
    let mut fi1 = valfpa(vlw1, b, delta);
    let mut fi2 = valfpa(vlw2, b, delta);

    if !fi1.is_finite() || !fi2.is_finite() {
        // R stops with an error; we return a sentinel that triggers istop=4.
        return (-1e9, vw0);
    }

    let goto50 = |step: f64, vlw2: f64, fi1: f64, fi2: f64, fi3: f64| -> (f64, f64) {
        let vm = vlw2 - (step * (fi1 - fi3)) / (2.0 * (fi1 - 2.0 * fi2 + fi3));
        let fim = valfpa(vm, b, delta);
        (vm, fim)
    };

    if fi2 >= fi1 {
        let _vlw3 = vlw2;
        vlw2 = vlw1;
        let fi3 = fi2;
        fi2 = fi1;
        let nstep = -step;
        let vlw1n = vlw2 + nstep;
        let fi1n = valfpa(vlw1n, b, delta);
        let (vm, fim) = goto50(nstep, vlw2, fi1n, fi2, fi3);
        let (vm_out, fim_out) = if fim.is_nan() || fim > fi2 {
            (vlw2, if fim.is_nan() { 1e10 } else { fim })
        } else {
            (vm, fim)
        };
        let vw = vm_out.exp();
        let fim_final = if fim_out <= fi2 { fim_out } else { fi2 };
        (fim_final, vw)
    } else {
        // March forward until fi1 > fi2.
        let vlw_prev = vlw1;
        let mut vlw1c = vlw2;
        vlw2 = vlw_prev;
        let fim = fi1;
        fi1 = fi2;
        fi2 = fim;
        let mut fi3 = 0.0;
        let mut vlw3 = 0.0;
        let mut out_vm = vlw2;
        let mut out_fim = fi2;
        for _ in 0..40 {
            vlw3 = vlw2;
            vlw2 = vlw1c;
            fi3 = fi2;
            fi2 = fi1;
            vlw1c = vlw2 + step;
            fi1 = valfpa(vlw1c, b, delta);
            if fi1 > fi2 {
                let (vm, fim) = goto50(step, vlw2, fi1, fi2, fi3);
                out_vm = vm;
                out_fim = if fim.is_nan() || fim <= fi2 { fim } else { fi2 };
                if out_fim.is_nan() {
                    out_fim = fi2;
                    out_vm = vlw2;
                }
                break;
            }
            if fi1 == fi2 {
                out_vm = vlw2;
                out_fim = fi2;
                break;
            }
        }
        let vw = out_vm.exp();
        let _ = (vlw_prev, fi1);
        (out_fim, vw)
    }
}

/// Solve `(V + D) · x = grad` where `V+D` is SPD packed upper-tri.
///
/// `deriva` computes `V = −Hessian_of_f` and `grad = ∇f`.  The Newton step is
/// `delta = V⁻¹ · grad` because `−H⁻¹·∇f = (−H)⁻¹·∇f = V⁻¹·∇f`.
fn solve_damped(hd_packed: &[f64], grad: &[f64], m: usize) -> Option<Vec<f64>> {
    let rhs: Vec<f64> = grad.to_vec();
    // Cholesky factor the damped Hessian.
    let full = sym_to_full(hd_packed, m);
    let mut l = vec![0.0; m * m];
    for j in 0..m {
        let mut d = full[j * m + j];
        for k in 0..j {
            d -= l[j * m + k] * l[j * m + k];
        }
        if d <= 0.0 {
            return None;
        }
        let dj = d.sqrt();
        l[j * m + j] = dj;
        let inv = 1.0 / dj;
        for i in (j + 1)..m {
            let mut s = full[i * m + j];
            for k in 0..j {
                s -= l[i * m + k] * l[j * m + k];
            }
            l[i * m + j] = s * inv;
        }
    }
    // Forward solve L y = rhs.
    let mut y = vec![0.0; m];
    for i in 0..m {
        let mut s = rhs[i];
        for k in 0..i {
            s -= l[i * m + k] * y[k];
        }
        y[i] = s / l[i * m + i];
    }
    // Back solve L' x = y.
    let mut x = vec![0.0; m];
    for i in (0..m).rev() {
        let mut s = y[i];
        for k in (i + 1)..m {
            s -= l[k * m + i] * x[k];
        }
        x[i] = s / l[i * m + i];
    }
    Some(x)
}

/// Expand packed upper-tri (length m*(m+1)/2, column-major packing matching
/// Fortran `(j-1)*j/2+i` 1-indexed → `(j)*(j+1)/2+i` ... note: 0-indexed
/// `(j)*(j+1)/2 + i` for i≤j) into a full row-major symmetric matrix.
pub(crate) fn sym_to_full(packed: &[f64], m: usize) -> Vec<f64> {
    let mut full = vec![0.0; m * m];
    for j in 0..m {
        for i in 0..=j {
            let v = packed[j * (j + 1) / 2 + i];
            full[i * m + j] = v;
            full[j * m + i] = v;
        }
    }
    full
}

/// Pack a full row-major symmetric matrix into upper-tri column-major form.
pub(crate) fn pack_upper(full: &[f64], m: usize) -> Vec<f64> {
    let mut packed = vec![0.0; m * (m + 1) / 2];
    for j in 0..m {
        for i in 0..=j {
            packed[j * (j + 1) / 2 + i] = full[i * m + j];
        }
    }
    packed
}
