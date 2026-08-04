//! Gray's (1988) k-sample test for comparing cumulative incidence functions.
//!
//! Transliterated from `reference/cmprsk/src/crstm.f` (`crstm` + `crst`).
//!
//! `crst` computes, for one stratum, the score vector `s` (length `ng-1`) and
//! its covariance `v` (packed lower triangle, `l = i(i-1)/2 + j`). `crstm` sums
//! those over strata and unpacks the result into a symmetric matrix. The test
//! statistic is then `s' V⁻¹ s`, asymptotically `χ²(ng-1)`.
//!
//! The routine carries *pairs* of running quantities: `skmm`/`skm` are the
//! left- and right-continuous Kaplan–Meier estimates of overall survival within
//! each group, and `f1m`/`f1` the left- and right-continuous cumulative
//! incidence estimates. Mixing them up silently changes the variance, so the
//! update order at label 92 is preserved exactly.

/// Score vector and its (unpacked, symmetric) covariance matrix.
#[derive(Debug, Clone)]
pub struct CrstmOut {
    /// Scores for the first `ng - 1` groups.
    pub s: Vec<f64>,
    /// Estimated covariance of `s`, `(ng-1) × (ng-1)`.
    pub vs: Vec<Vec<f64>>,
}

/// Packed index used by the Fortran: `l = i(i-1)/2 + j` for `j <= i`, 1-based.
#[inline]
fn pk(i: usize, j: usize) -> usize {
    i * (i - 1) / 2 + j - 1
}

/// `subroutine crst` — one stratum.
///
/// * `y` — times, ascending.
/// * `m` — `0` censored, `1` failure from the cause of interest, `2` other.
/// * `ig` — group index, **1-based**, in `1..=ng`.
#[allow(clippy::needless_range_loop)]
fn crst(y: &[f64], m: &[u8], ig: &[usize], ng: usize, rho: f64) -> (Vec<f64>, Vec<f64>) {
    let n = y.len();
    let ng1 = ng - 1;
    let nv = ng * ng1 / 2;

    let mut s = vec![0.0_f64; ng1];
    let mut v = vec![0.0_f64; nv];
    if n == 0 {
        return (s, v);
    }

    // rs(j): risk-set size in group j at the current failure time.
    let mut rs = vec![0i64; ng + 1];
    for i in 0..n {
        rs[ig[i]] += 1;
    }

    let mut f1m = vec![0.0_f64; ng + 1];
    let mut f1 = vec![0.0_f64; ng + 1];
    let mut skmm = vec![1.0_f64; ng + 1];
    let mut skm = vec![1.0_f64; ng + 1];
    let mut v3 = vec![0.0_f64; ng + 1];
    // v2(ng1, ng) and c(ng, ng), both 1-based.
    let mut v2 = vec![vec![0.0_f64; ng + 1]; ng1 + 1];
    let mut cmat = vec![vec![0.0_f64; ng + 1]; ng + 1];
    let mut a = vec![vec![0.0_f64; ng + 1]; ng + 1];
    // d(0:2, ng)
    let mut d = [vec![0i64; ng + 1], vec![0i64; ng + 1], vec![0i64; ng + 1]];

    let mut fm = 0.0_f64;
    let mut f = 0.0_f64;

    let mut ll = 1usize;
    let mut lu;

    loop {
        // ── label 50: find the tie block [ll, lu] ───────────────────────────
        lu = ll;
        loop {
            lu += 1;
            if lu > n {
                break;
            }
            if y[lu - 1] > y[ll - 1] {
                break;
            }
        }
        lu -= 1;

        let mut nd1 = 0i64;
        let mut nd2 = 0i64;
        for k in 0..3 {
            for item in d[k].iter_mut() {
                *item = 0;
            }
        }
        for i in ll..=lu {
            let j = ig[i - 1];
            let k = m[i - 1] as usize;
            d[k][j] += 1;
        }
        for i in 1..=ng {
            nd1 += d[1][i];
            nd2 += d[2][i];
        }

        if nd1 != 0 || nd2 != 0 {
            let mut tr = 0.0_f64;
            let mut tq = 0.0_f64;
            for i in 1..=ng {
                if rs[i] <= 0 {
                    continue;
                }
                let td = (d[1][i] + d[2][i]) as f64;
                let rsi = rs[i] as f64;
                // skmm left-continuous, skm right-continuous KM
                skm[i] = skmm[i] * (rsi - td) / rsi;
                // f1m left-continuous, f1 right-continuous cuminc
                f1[i] = f1m[i] + (skmm[i] * d[1][i] as f64) / rsi;
                tr += rsi / skmm[i];
                tq += rsi * (1.0 - f1m[i]) / skmm[i];
            }
            f = fm + nd1 as f64 / tr;
            let fb = (1.0 - fm).powf(rho);

            // ── loop 66: build a(i,j) and accumulate c(i,j) ─────────────────
            for i in 1..=ng {
                for j in i..=ng {
                    a[i][j] = 0.0;
                }
                if rs[i] <= 0 {
                    continue;
                }
                let t1 = rs[i] as f64 / skmm[i];
                a[i][i] = fb * t1 * (1.0 - t1 / tr);
                if a[i][i] != 0.0 {
                    cmat[i][i] += a[i][i] * nd1 as f64 / (tr * (1.0 - fm));
                }
                for j in (i + 1)..=ng {
                    if rs[j] <= 0 {
                        continue;
                    }
                    a[i][j] = -fb * t1 * rs[j] as f64 / (skmm[j] * tr);
                    if a[i][j] != 0.0 {
                        cmat[i][j] += a[i][j] * nd1 as f64 / (tr * (1.0 - fm));
                    }
                }
            }
            for i in 2..=ng {
                for j in 1..i {
                    a[i][j] = a[j][i];
                    cmat[i][j] = cmat[j][i];
                }
            }

            // ── loop 74: scores ────────────────────────────────────────────
            for i in 1..=ng1 {
                if rs[i] <= 0 {
                    continue;
                }
                s[i - 1] += fb
                    * (d[1][i] as f64
                        - nd1 as f64 * rs[i] as f64 * (1.0 - f1m[i]) / (skmm[i] * tq));
            }

            // ── loop 72: variance from failures of the cause of interest ───
            if nd1 > 0 {
                for k in 1..=ng {
                    if rs[k] <= 0 {
                        continue;
                    }
                    let t4 = if skm[k] > 0.0 {
                        1.0 - (1.0 - f) / skm[k]
                    } else {
                        1.0
                    };
                    let t5 = if nd1 > 1 {
                        1.0 - (nd1 - 1) as f64 / (tr * skmm[k] - 1.0)
                    } else {
                        1.0
                    };
                    let t3 = t5 * skmm[k] * nd1 as f64 / (tr * rs[k] as f64);
                    v3[k] += t4 * t4 * t3;
                    for i in 1..=ng1 {
                        let t1 = a[i][k] - t4 * cmat[i][k];
                        v2[i][k] += t1 * t4 * t3;
                        for j in 1..=i {
                            let t2 = a[j][k] - t4 * cmat[j][k];
                            v[pk(i, j)] += t1 * t2 * t3;
                        }
                    }
                }
            }

            // ── loop 82: variance from competing failures ──────────────────
            if nd2 != 0 {
                for k in 1..=ng {
                    if skm[k] <= 0.0 || d[2][k] <= 0 {
                        continue;
                    }
                    let t4 = (1.0 - f) / skm[k];
                    let t5 = if d[2][k] > 1 {
                        1.0 - (d[2][k] as f64 - 1.0) / (rs[k] as f64 - 1.0)
                    } else {
                        1.0
                    };
                    let t6 = rs[k] as f64;
                    let t3 = t5 * (skmm[k] * skmm[k] * d[2][k] as f64) / (t6 * t6);
                    v3[k] += t4 * t4 * t3;
                    for i in 1..=ng1 {
                        let t1 = t4 * cmat[i][k];
                        v2[i][k] -= t1 * t4 * t3;
                        for j in 1..=i {
                            let t2 = t4 * cmat[j][k];
                            v[pk(i, j)] += t1 * t2 * t3;
                        }
                    }
                }
            }
        }

        // ── label 90 ────────────────────────────────────────────────────────
        if lu >= n {
            break;
        }
        for i in ll..=lu {
            rs[ig[i - 1]] -= 1;
        }
        fm = f;
        f1m[1..].copy_from_slice(&f1[1..]);
        skmm[1..].copy_from_slice(&skm[1..]);
        ll = lu + 1;
    }

    // ── label 30: fold the censoring-martingale terms into v ────────────────
    for i in 1..=ng1 {
        for j in 1..=i {
            let l = pk(i, j);
            for k in 1..=ng {
                v[l] += cmat[i][k] * cmat[j][k] * v3[k];
                v[l] += cmat[i][k] * v2[j][k];
                v[l] += cmat[j][k] * v2[i][k];
            }
        }
    }

    (s, v)
}

/// `subroutine crstm` — sum [`crst`] over strata and unpack the covariance.
///
/// * `ig` — group index, 1-based in `1..=ng`.
/// * `ist` — stratum index, 1-based in `1..=nst`.
pub fn crstm(
    y: &[f64],
    m: &[u8],
    ig: &[usize],
    ist: &[usize],
    nst: usize,
    ng: usize,
    rho: f64,
) -> CrstmOut {
    let ng1 = ng - 1;
    let nv = ng * ng1 / 2;
    let mut s = vec![0.0_f64; ng1];
    let mut v = vec![0.0_f64; nv];

    for ks in 1..=nst {
        let mut ys = Vec::new();
        let mut ms = Vec::new();
        let mut igs = Vec::new();
        for i in 0..y.len() {
            if ist[i] != ks {
                continue;
            }
            ys.push(y[i]);
            ms.push(m[i]);
            igs.push(ig[i]);
        }
        let (st, vt) = crst(&ys, &ms, &igs, ng, rho);
        for i in 0..ng1 {
            s[i] += st[i];
        }
        for l in 0..nv {
            v[l] += vt[l];
        }
    }

    let mut vs = vec![vec![0.0_f64; ng1]; ng1];
    for i in 1..=ng1 {
        for j in 1..=i {
            let val = v[pk(i, j)];
            vs[i - 1][j - 1] = val;
            vs[j - 1][i - 1] = val;
        }
    }

    CrstmOut { s, vs }
}
