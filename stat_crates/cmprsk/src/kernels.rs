//! Line-faithful transliteration of `reference/cmprsk/src/crr.f`.
//!
//! `needless_late_init` is allowed because the `let mut x; if … { continue }
//! … x = … else { x = … }` shape follows the Fortran branch/`go to` structure
//! verbatim; collapsing it into `let x = if …` would silently reorder the
//! control flow we are preserving bit-for-bit.
#![allow(clippy::needless_late_init)]
//!
//! The six routines below (`covt`, `crrfsv`, `crrf`, `crrvv`, `crrsr`,
//! `crrfit`) are ported statement-by-statement from Bob Gray's Fortran 77,
//! including its control flow. Several of the loops compute *running centred*
//! quantities whose intermediate state is reused in a way that does not match
//! the textbook formulae — for example `crrfsv`'s inner loop mutates `xbt` in
//! place after accumulating `xb`, and weights the outer-product update by
//! `xb1 * twt / xb1o`. Rewriting these "cleanly" changes the numbers, so the
//! structure is preserved verbatim and only index bases are converted.
//!
//! Fortran indices are 1-based; local control variables that participate in the
//! algorithm's logic (`iuc`, `itmp`, `ldf`, `lc`) are kept 1-based here so the
//! comparisons read the same as the source, and `-1` is applied at the point of
//! array access.
//!
//! Data layout: Fortran's `x(n,ncov)` is column-major, but every access is
//! `x(row, col)`, so the port stores covariates row-major as `x[i][j]`.

/// All inputs shared by the five kernels.
///
/// Mirrors the common argument prefix
/// `(t2, ici, n, x, ncov, np, x2, ncov2, tf, ndf, wt, ncg, icg)`.
pub struct CrrData<'a> {
    /// Event/censoring times, **ascending**.
    pub t2: &'a [f64],
    /// `1` = failure of the cause of interest, `2` = competing failure,
    /// `0` = censored.
    pub ici: &'a [u8],
    /// Fixed covariates, `n × ncov`.
    pub x: &'a [Vec<f64>],
    /// Number of fixed covariates (`np - npt` in the R driver).
    pub ncov: usize,
    /// Total number of parameters, `ncov + ncov2`.
    pub np: usize,
    /// Time-interacted covariates, `n × ncov2`.
    pub x2: &'a [Vec<f64>],
    /// Number of time-interacted covariates.
    pub ncov2: usize,
    /// Time functions evaluated at the unique failure times, `ndf × ncov2`.
    pub tf: &'a [Vec<f64>],
    /// Number of distinct type-1 failure times.
    pub ndf: usize,
    /// Censoring survivor weights `G_g(t_i-)`, `ncg × n`.
    pub wt: &'a [Vec<f64>],
    /// Number of censoring groups.
    pub ncg: usize,
    /// 0-based censoring-group index per observation.
    pub icg: &'a [usize],
}

impl CrrData<'_> {
    /// Number of observations.
    #[inline]
    pub fn n(&self) -> usize {
        self.t2.len()
    }

    /// `subroutine covt` — build the covariate vector for case `j` at the
    /// `k`-th distinct failure time and return `x'β`.
    ///
    /// `j` and `k` are **0-based**.
    #[inline]
    fn covt(&self, j: usize, k: usize, b: &[f64], xbt: &mut [f64]) -> f64 {
        let mut wk = 0.0_f64;
        for i in 0..self.ncov {
            xbt[i] = self.x[j][i];
            wk += xbt[i] * b[i];
        }
        for i in 0..self.ncov2 {
            xbt[self.ncov + i] = self.x2[j][i] * self.tf[k][i];
            wk += xbt[self.ncov + i] * b[self.ncov + i];
        }
        wk
    }
}

/// Result of [`crrfsv`]: the negated log pseudo-likelihood, its gradient and
/// its (positive-definite) second-derivative matrix.
///
/// `crr` runs a *minimiser*, so `lik` and `s` carry the opposite sign to the
/// log-likelihood and score reported by `crr()` — the R driver flips them
/// (`loglik = -z[[1]]`, `score = -z[[2]]`).
#[derive(Debug, Clone)]
pub struct Fsv {
    /// Negative log pseudo-likelihood.
    pub lik: f64,
    /// Gradient of `lik` (i.e. minus the score).
    pub s: Vec<f64>,
    /// Second-derivative matrix (the information), `np × np`.
    pub v: Vec<Vec<f64>>,
}

/// `subroutine crrfsv` — objective, gradient and Hessian in one pass.
pub fn crrfsv(d: &CrrData, b: &[f64]) -> Fsv {
    let n = d.n();
    let np = d.np;

    let mut lik = 0.0_f64;
    let mut s = vec![0.0_f64; np];
    let mut v = vec![vec![0.0_f64; np]; np];

    let mut xb = vec![0.0_f64; np];
    let mut xbt = vec![0.0_f64; np];
    let mut vt = vec![vec![0.0_f64; np]; np];

    let mut iuc = n; // 1-based
    let mut ldf = d.ndf + 1; // 1-based
    if n == 0 {
        return Fsv { lik, s, v };
    }

    loop {
        // ── label 98: find the next (largest remaining) failure time ────────
        let mut itmp = iuc;
        let mut cft = 0.0_f64;
        let mut found = false;
        for i in (1..=iuc).rev() {
            itmp = i;
            if d.ici[i - 1] == 1 {
                cft = d.t2[i - 1];
                found = true;
                break;
            }
        }
        let _unused = &itmp;
        if !found {
            return Fsv { lik, s, v }; // no more failures
        }

        // ── label 11 ────────────────────────────────────────────────────────
        iuc = itmp;
        ldf -= 1;
        let mut twf = 0.0_f64;
        let mut itmp = iuc;
        for i in (1..=iuc).rev() {
            if d.t2[i - 1] < cft {
                break; // go to 14
            }
            itmp = i;
            if d.ici[i - 1] == 1 {
                let wk = d.covt(i - 1, ldf - 1, b, &mut xbt);
                twf += 1.0;
                // minimisation → negate objective and scores
                lik -= wk;
                for j in 0..np {
                    s[j] -= xbt[j];
                }
            }
        }

        // ── label 14: sums over the risk set ────────────────────────────────
        iuc = itmp;
        let mut xb1 = 0.0_f64;
        let mut xb1o = xb1;
        for item in xb.iter_mut() {
            *item = 0.0;
        }
        for (i, row) in vt.iter_mut().enumerate() {
            for item in row.iter_mut().skip(i) {
                *item = 0.0;
            }
        }

        for i in 1..=n {
            let twt;
            if d.t2[i - 1] < cft {
                if d.ici[i - 1] <= 1 {
                    continue; // go to 15
                }
                let wk = d.covt(i - 1, ldf - 1, b, &mut xbt);
                let g = d.icg[i - 1];
                twt = wk.exp() * d.wt[g][iuc - 1] / d.wt[g][i - 1];
            } else {
                let wk = d.covt(i - 1, ldf - 1, b, &mut xbt);
                twt = wk.exp();
            }
            xb1 += twt;
            for j in 0..np {
                xb[j] += twt * xbt[j];
                xbt[j] -= xb[j] / xb1;
            }
            if xb1o > 0.0 {
                let twt2 = xb1 * twt / xb1o;
                for k in 0..np {
                    for j in k..np {
                        vt[k][j] += twt2 * xbt[k] * xbt[j];
                    }
                }
            }
            xb1o = xb1;
        }

        lik += twf * xb1.ln();
        let twt3 = twf / xb1;
        for i in 0..np {
            s[i] += twt3 * xb[i];
            for j in i..np {
                v[i][j] += twt3 * vt[i][j];
                v[j][i] = v[i][j];
            }
        }

        if iuc <= 1 {
            return Fsv { lik, s, v };
        }
        iuc -= 1;
    }
}

/// `subroutine crrf` — objective only (used by the backtracking line search).
pub fn crrf(d: &CrrData, b: &[f64]) -> f64 {
    let n = d.n();
    let np = d.np;
    let mut lik = 0.0_f64;
    let mut xbt = vec![0.0_f64; np];

    let mut iuc = n;
    let mut ldf = d.ndf + 1;
    if n == 0 {
        return lik;
    }

    loop {
        let mut itmp = iuc;
        let mut cft = 0.0_f64;
        let mut found = false;
        for i in (1..=iuc).rev() {
            itmp = i;
            if d.ici[i - 1] == 1 {
                cft = d.t2[i - 1];
                found = true;
                break;
            }
        }
        if !found {
            return lik;
        }

        iuc = itmp;
        ldf -= 1;
        let mut twf = 0.0_f64;
        let mut itmp = iuc;
        for i in (1..=iuc).rev() {
            if d.t2[i - 1] < cft {
                break;
            }
            itmp = i;
            if d.ici[i - 1] == 1 {
                let wk = d.covt(i - 1, ldf - 1, b, &mut xbt);
                twf += 1.0;
                lik -= wk;
            }
        }

        iuc = itmp;
        let mut xb1 = 0.0_f64;
        for i in 1..=n {
            let twt;
            if d.t2[i - 1] < cft {
                if d.ici[i - 1] <= 1 {
                    continue;
                }
                let wk = d.covt(i - 1, ldf - 1, b, &mut xbt);
                let g = d.icg[i - 1];
                twt = wk.exp() * d.wt[g][iuc - 1] / d.wt[g][i - 1];
            } else {
                let wk = d.covt(i - 1, ldf - 1, b, &mut xbt);
                twt = wk.exp();
            }
            xb1 += twt;
        }
        lik += twf * xb1.ln();

        if iuc <= 1 {
            return lik;
        }
        iuc -= 1;
    }
}

/// Result of [`crrvv`]: the two matrices of the sandwich estimator.
#[derive(Debug, Clone)]
pub struct Vv {
    /// `v` — the "bread": second derivatives of the log pseudo-likelihood.
    pub v: Vec<Vec<f64>>,
    /// `v2` — the "meat": empirical covariance of the influence functions
    /// `η_i + ψ_i`.
    pub v2: Vec<Vec<f64>>,
}

/// `subroutine crrvv` — robust variance components.
///
/// This is the most intricate of the kernels: it builds the risk-set sums
/// `xb(i, ·)` at every type-1 failure time, then walks the sample accumulating
/// the influence-function contributions `η_i` (`st(·,1)`) and the
/// censoring-martingale correction `ψ_i` (`st(·,2)`), the latter driven by the
/// `q(u)` integral in `qu` and the running `ss2` accumulator.
pub fn crrvv(d: &CrrData, b: &[f64]) -> Vv {
    let n = d.n();
    let np = d.np;
    let ncg = d.ncg;

    let mut v = vec![vec![0.0_f64; np]; np];
    let mut v2 = vec![vec![0.0_f64; np]; np];
    let mut vt = vec![vec![0.0_f64; np]; np];
    let mut xbt = vec![0.0_f64; np];
    // xb(n, 0:np) → column 0 holds the scalar sum, columns 1..np the vector.
    let mut xb = vec![vec![0.0_f64; np + 1]; n];
    let mut qu = vec![vec![0.0_f64; ncg]; np];
    let mut ss2 = vec![vec![0.0_f64; ncg]; np];
    let mut ss3 = vec![vec![0.0_f64; ncg]; np];
    let mut ss4 = vec![0.0_f64; ncg];
    let mut st1 = vec![0.0_f64; np];
    let mut st2 = vec![0.0_f64; np];
    let mut icrsk = vec![0_i64; ncg];

    if n == 0 {
        return Vv { v, v2 };
    }

    for i in 0..n {
        icrsk[d.icg[i]] += 1;
    }

    // ── loop 6: risk-set sums at each distinct type-1 failure time ──────────
    {
        let mut ldf = 0usize; // 1-based once incremented
        let mut cft = (-1.0_f64).min(d.t2[0] * (1.0 - 1.0e-5));
        for i in 1..=n {
            if d.ici[i - 1] != 1 {
                continue;
            }
            if d.t2[i - 1] > cft {
                cft = d.t2[i - 1];
                ldf += 1;
            }
            for j in 1..=n {
                let twt;
                if d.t2[j - 1] < d.t2[i - 1] {
                    if d.ici[j - 1] <= 1 {
                        continue;
                    }
                    let wk = d.covt(j - 1, ldf - 1, b, &mut xbt);
                    let g = d.icg[j - 1];
                    twt = wk.exp() * d.wt[g][i - 1] / d.wt[g][j - 1];
                } else {
                    let wk = d.covt(j - 1, ldf - 1, b, &mut xbt);
                    twt = wk.exp();
                }
                xb[i - 1][0] += twt;
                for k in 1..=np {
                    xb[i - 1][k] += twt * xbt[k - 1];
                }
            }
        }
    }

    // ── loop 10: influence functions ────────────────────────────────────────
    let mut lc = 1usize; // 1-based
    let mut ldf2 = 0usize;
    let mut cft2 = (-1.0_f64).min(d.t2[0] * (1.0 - 1.0e-5));

    for i in 1..=n {
        for item in st1.iter_mut() {
            *item = 0.0;
        }

        // ── loop 15: dΛ̂ portion of η_i ──────────────────────────────────
        {
            let mut ldf = 0usize;
            let mut cft = (-1.0_f64).min(d.t2[0] * (1.0 - 1.0e-5));
            for j in 1..=n {
                if d.ici[j - 1] != 1 {
                    continue;
                }
                if d.t2[j - 1] > cft {
                    cft = d.t2[j - 1];
                    ldf += 1;
                }
                let twt;
                if d.t2[j - 1] <= d.t2[i - 1] {
                    let wk = d.covt(i - 1, ldf - 1, b, &mut xbt);
                    twt = wk.exp();
                } else if d.t2[i - 1] < d.t2[j - 1] && d.ici[i - 1] > 1 {
                    let wk = d.covt(i - 1, ldf - 1, b, &mut xbt);
                    let g = d.icg[i - 1];
                    twt = wk.exp() * d.wt[g][j - 1] / d.wt[g][i - 1];
                } else {
                    continue; // go to 15
                }
                for k in 1..=np {
                    st1[k - 1] -= (xbt[k - 1] - xb[j - 1][k] / xb[j - 1][0]) * twt / xb[j - 1][0];
                }
            }
        }

        if d.ici[i - 1] == 1 {
            if d.t2[i - 1] > cft2 {
                cft2 = d.t2[i - 1];
                ldf2 += 1;
            }
            d.covt(i - 1, ldf2 - 1, b, &mut xbt);
            // dN_i portion of η_i
            for k in 1..=np {
                st1[k - 1] += xbt[k - 1] - xb[i - 1][k] / xb[i - 1][0];
            }
            // second derivatives
            for (j1, row) in vt.iter_mut().enumerate() {
                for item in row.iter_mut().skip(j1) {
                    *item = 0.0;
                }
            }
            for j in 1..=n {
                let twt;
                if d.t2[j - 1] < d.t2[i - 1] {
                    if d.ici[j - 1] <= 1 {
                        continue;
                    }
                    let wk = d.covt(j - 1, ldf2 - 1, b, &mut xbt);
                    let g = d.icg[j - 1];
                    twt = wk.exp() * d.wt[g][i - 1] / d.wt[g][j - 1];
                } else {
                    let wk = d.covt(j - 1, ldf2 - 1, b, &mut xbt);
                    twt = wk.exp();
                }
                for k in 1..=np {
                    xbt[k - 1] -= xb[i - 1][k] / xb[i - 1][0];
                }
                for k in 0..np {
                    for j2 in k..np {
                        vt[k][j2] += twt * xbt[k] * xbt[j2];
                    }
                }
            }
            for j1 in 0..np {
                for j2 in j1..np {
                    v[j1][j2] += vt[j1][j2] / xb[i - 1][0];
                }
            }
        }

        // ── q(u) recomputation, once per distinct time ──────────────────────
        let iflg = i == 1 || d.t2[i - 1] > d.t2[i - 2];
        if iflg {
            // do 40: is there a censored case among the ties at t2(i)?
            let mut goto39 = false;
            for j in i..=n {
                if d.t2[j - 1] > d.t2[i - 1] {
                    goto39 = true;
                    break;
                }
                if d.ici[j - 1] == 0 {
                    break; // go to 38
                }
            }

            if !goto39 {
                // ── label 38 ────────────────────────────────────────────────
                let mut ldf = ldf2;
                let mut cft = cft2;
                for row in qu.iter_mut() {
                    for item in row.iter_mut() {
                        *item = 0.0;
                    }
                }
                for j1 in lc..=n {
                    if d.ici[j1 - 1] != 1 {
                        continue;
                    }
                    if d.t2[j1 - 1] > cft {
                        cft = d.t2[j1 - 1];
                        ldf += 1;
                    }
                    for item in ss4.iter_mut() {
                        *item = 0.0;
                    }
                    for row in ss3.iter_mut() {
                        for item in row.iter_mut() {
                            *item = 0.0;
                        }
                    }
                    // only type-2 failures strictly before t2(i) contribute
                    for j2 in 1..=n {
                        if d.t2[j2 - 1] >= d.t2[i - 1] {
                            break; // go to 542
                        }
                        if d.ici[j2 - 1] <= 1 {
                            continue;
                        }
                        let wk = d.covt(j2 - 1, ldf - 1, b, &mut xbt);
                        let g = d.icg[j2 - 1];
                        let twt = wk.exp() * d.wt[g][j1 - 1] / d.wt[g][j2 - 1];
                        ss4[g] += twt;
                        for k in 0..np {
                            ss3[k][g] += xbt[k] * twt;
                        }
                    }
                    let g1 = d.icg[j1 - 1];
                    for k in 0..np {
                        qu[k][g1] += (ss3[k][g1] - xb[j1 - 1][k + 1] * ss4[g1] / xb[j1 - 1][0])
                            / xb[j1 - 1][0];
                    }
                }

                // ── loop 43: dΛ̂_c portion of ψ_i ──────────────────────────
                for j in i..=n {
                    if d.t2[j - 1] > d.t2[i - 1] {
                        break; // go to 39
                    }
                    if d.ici[j - 1] == 0 {
                        let g = d.icg[j - 1];
                        let ir = icrsk[g] as f64;
                        for k in 0..np {
                            ss2[k][g] -= qu[k][g] / (ir * ir);
                        }
                    }
                }
            }
        }

        // ── label 39: ψ_i ───────────────────────────────────────────────────
        let gi = d.icg[i - 1];
        for k in 0..np {
            st2[k] = ss2[k][gi];
        }
        if d.ici[i - 1] == 0 {
            let ir = icrsk[gi] as f64;
            for k in 0..np {
                st2[k] += qu[k][gi] / ir;
            }
        }
        for j1 in (0..np).rev() {
            st1[j1] += st2[j1];
            for j2 in j1..np {
                v2[j1][j2] += st1[j1] * st1[j2];
            }
        }

        if i < n && d.t2[i] > d.t2[i - 1] {
            for j in lc..=i {
                icrsk[d.icg[j - 1]] -= 1;
            }
            lc = i + 1;
        }
    }

    for j1 in 0..np.saturating_sub(1) {
        for j2 in (j1 + 1)..np {
            v[j2][j1] = v[j1][j2];
            v2[j2][j1] = v2[j1][j2];
        }
    }

    Vv { v, v2 }
}

/// `subroutine crrsr` — Schoenfeld-type score residuals.
///
/// Returns an `ndf × np` matrix: row `k` is the contribution to each score at
/// the `k`-th distinct failure time. (The R driver transposes the Fortran
/// `res(np, ndf)` into exactly this shape.)
pub fn crrsr(d: &CrrData, b: &[f64]) -> Vec<Vec<f64>> {
    let n = d.n();
    let np = d.np;
    let mut res = vec![vec![0.0_f64; np]; d.ndf];
    let mut xb = vec![0.0_f64; np];
    let mut xbt = vec![0.0_f64; np];

    let mut iuc = n;
    let mut ldf = d.ndf + 1;
    if n == 0 {
        return res;
    }

    loop {
        let mut itmp = iuc;
        let mut cft = 0.0_f64;
        let mut found = false;
        for i in (1..=iuc).rev() {
            itmp = i;
            if d.ici[i - 1] == 1 {
                cft = d.t2[i - 1];
                found = true;
                break;
            }
        }
        if !found {
            return res;
        }

        iuc = itmp;
        ldf -= 1;
        let mut twf = 0.0_f64;
        let mut itmp = iuc;
        for i in (1..=iuc).rev() {
            if d.t2[i - 1] < cft {
                break;
            }
            itmp = i;
            if d.ici[i - 1] == 1 {
                d.covt(i - 1, ldf - 1, b, &mut xbt);
                twf += 1.0;
                for j in 0..np {
                    res[ldf - 1][j] += xbt[j];
                }
            }
        }

        iuc = itmp;
        let mut xb1 = 0.0_f64;
        for item in xb.iter_mut() {
            *item = 0.0;
        }
        for i in 1..=n {
            let twt;
            if d.t2[i - 1] < cft {
                if d.ici[i - 1] <= 1 {
                    continue;
                }
                let wk = d.covt(i - 1, ldf - 1, b, &mut xbt);
                let g = d.icg[i - 1];
                twt = wk.exp() * d.wt[g][iuc - 1] / d.wt[g][i - 1];
            } else {
                let wk = d.covt(i - 1, ldf - 1, b, &mut xbt);
                twt = wk.exp();
            }
            xb1 += twt;
            for j in 0..np {
                xb[j] += twt * xbt[j];
            }
        }
        let twt = -twf / xb1;
        for i in 0..np {
            res[ldf - 1][i] += twt * xb[i];
        }

        if iuc <= 1 {
            return res;
        }
        iuc -= 1;
    }
}

/// `subroutine crrfit` — jumps in the Breslow-type estimate of the underlying
/// cumulative subdistribution hazard, one per distinct type-1 failure time.
pub fn crrfit(d: &CrrData, b: &[f64]) -> Vec<f64> {
    let n = d.n();
    let np = d.np;
    let mut res = vec![0.0_f64; d.ndf];
    let mut xbt = vec![0.0_f64; np];

    let mut iuc = 1usize;
    let mut ldf = 0usize;
    if n == 0 {
        return res;
    }

    loop {
        let mut itmp = iuc;
        let mut cft = 0.0_f64;
        let mut found = false;
        for i in iuc..=n {
            itmp = i;
            if d.ici[i - 1] == 1 {
                cft = d.t2[i - 1];
                found = true;
                break;
            }
        }
        if !found {
            return res;
        }

        iuc = itmp;
        ldf += 1;
        let mut twf = 0.0_f64;
        let mut itmp = iuc;
        for i in iuc..=n {
            if d.t2[i - 1] > cft {
                break;
            }
            itmp = i;
            if d.ici[i - 1] == 1 {
                twf += 1.0;
            }
        }

        iuc = itmp;
        let mut xb1 = 0.0_f64;
        for i in 1..=n {
            let twt;
            if d.t2[i - 1] < cft {
                if d.ici[i - 1] <= 1 {
                    continue;
                }
                let wk = d.covt(i - 1, ldf - 1, b, &mut xbt);
                let g = d.icg[i - 1];
                twt = wk.exp() * d.wt[g][iuc - 1] / d.wt[g][i - 1];
            } else {
                let wk = d.covt(i - 1, ldf - 1, b, &mut xbt);
                twt = wk.exp();
            }
            xb1 += twt;
        }
        res[ldf - 1] += twf / xb1;

        iuc += 1;
        if iuc > n {
            return res;
        }
    }
}
