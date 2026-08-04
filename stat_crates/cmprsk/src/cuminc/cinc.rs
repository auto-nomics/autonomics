//! `subroutine cinc` — cumulative incidence estimate and its variance.
//!
//! Transliterated from `reference/cmprsk/src/cincsub.f`.
//!
//! The output is deliberately shaped for step-function plotting: both corners
//! of every jump appear, so `x` starts at `(0, 0)` and each failure time of the
//! cause of interest contributes a pair `(t, F(t-))`, `(t, F(t))`. A final point
//! extends the curve out to the largest follow-up time. The arrays therefore
//! have length `2·nfc + 2` where `nfc` is the number of distinct failure times
//! of that cause — the same layout R hands back in
//! `cuminc()[[k]]$time / $est / $var`.

/// Cumulative incidence curve as returned by `cinc`.
#[derive(Debug, Clone, Default)]
pub struct CincOut {
    /// Times (step-function corners).
    pub x: Vec<f64>,
    /// Cumulative incidence estimates.
    pub f: Vec<f64>,
    /// Aalen-type variance estimates.
    pub v: Vec<f64>,
}

/// Estimate the cumulative incidence function for one cause in one group.
///
/// * `y` — failure/censoring times, **ascending**.
/// * `ic` — `1` if the case failed from any cause, `0` if censored.
/// * `icc` — `1` if the case failed from the cause of interest.
pub fn cinc(y: &[f64], ic: &[u8], icc: &[u8]) -> CincOut {
    let n = y.len();
    if n == 0 {
        return CincOut::default();
    }
    let nfc = {
        let mut c = 0usize;
        let mut last = f64::NEG_INFINITY;
        for i in 0..n {
            if icc[i] == 1 && (c == 0 || y[i] != last) {
                c += 1;
                last = y[i];
            }
        }
        c
    };
    let cap = 2 * nfc + 2;
    let mut x = vec![0.0_f64; cap];
    let mut f = vec![0.0_f64; cap];
    let mut v = vec![0.0_f64; cap];

    let mut fk = 1.0_f64;
    let (mut v1, mut v2, mut v3) = (0.0_f64, 0.0_f64, 0.0_f64);
    // Fortran is 1-based; `lcnt` is kept 1-based and `-1` applied at access.
    let mut lcnt = 1usize;
    let mut l = 1usize;
    let mut ll = 1usize;
    let mut rs = n as f64;
    let mut ty = y[0];

    loop {
        // ── label 10: advance to the end of the current tie block ───────────
        loop {
            l += 1;
            if l > n {
                break;
            }
            if y[l - 1] != ty {
                break;
            }
        }
        // ── label 60 ────────────────────────────────────────────────────────
        l -= 1;
        let mut nd1 = 0i64;
        let mut nd2 = 0i64;
        for i in ll..=l {
            nd1 += i64::from(icc[i - 1]);
            nd2 += i64::from(ic[i - 1]) - i64::from(icc[i - 1]);
        }
        let nd = nd1 + nd2;

        if nd != 0 {
            let fkn = fk * (rs - nd as f64) / rs;
            if nd1 > 0 {
                lcnt += 2;
                f[lcnt - 2] = f[lcnt - 3];
                f[lcnt - 1] = f[lcnt - 2] + fk * nd1 as f64 / rs;
            }
            // variance contribution from competing failures
            if nd2 > 0 && fkn > 0.0 {
                let t5 = if nd2 > 1 {
                    1.0 - (nd2 as f64 - 1.0) / (rs - 1.0)
                } else {
                    1.0
                };
                let t6 = fk * fk * t5 * nd2 as f64 / (rs * rs);
                let t3 = 1.0 / fkn;
                let t4 = f[lcnt - 1] / fkn;
                v1 += t4 * t4 * t6;
                v2 += t3 * t4 * t6;
                v3 += t3 * t3 * t6;
            }
            // ── label 30 ────────────────────────────────────────────────────
            if nd1 > 0 {
                let t5 = if nd1 > 1 {
                    1.0 - (nd1 as f64 - 1.0) / (rs - 1.0)
                } else {
                    1.0
                };
                let t6 = fk * fk * t5 * nd1 as f64 / (rs * rs);
                let t3 = if fkn > 0.0 { 1.0 / fkn } else { 0.0 };
                let t4 = 1.0 + t3 * f[lcnt - 1];
                v1 += t4 * t4 * t6;
                v2 += t3 * t4 * t6;
                v3 += t3 * t3 * t6;
                let t2 = f[lcnt - 1];
                x[lcnt - 2] = y[l - 1];
                x[lcnt - 1] = y[l - 1];
                v[lcnt - 2] = v[lcnt - 3];
                v[lcnt - 1] = v1 + t2 * t2 * v3 - 2.0 * t2 * v2;
            }
            // ── label 35 ────────────────────────────────────────────────────
            fk = fkn;
        }

        // ── label 40 ────────────────────────────────────────────────────────
        rs = (n - l) as f64;
        l += 1;
        if l > n {
            break;
        }
        ll = l;
        ty = y[l - 1];
    }

    // ── label 50 ────────────────────────────────────────────────────────────
    lcnt += 1;
    x[lcnt - 1] = y[n - 1];
    f[lcnt - 1] = f[lcnt - 2];
    v[lcnt - 1] = v[lcnt - 2];

    x.truncate(lcnt);
    f.truncate(lcnt);
    v.truncate(lcnt);
    CincOut { x, f, v }
}

/// `subroutine tpoi` — for each requested time, the index into a `cinc` curve
/// whose value should be reported. `0` means "beyond the end of the curve" and
/// maps to `NA` in R.
///
/// Returned indices are **1-based**, matching the Fortran (and R's use of them
/// as subscripts).
pub fn tpoi(x: &[f64], tp5: &[f64]) -> Vec<usize> {
    let n = x.len();
    let ntp = tp5.len();
    let mut ind = vec![0usize; ntp];
    if n == 0 || ntp == 0 {
        return ind;
    }
    let mut l = ntp as isize;

    // ── loop 10: trim requested times past the end of the curve ─────────────
    for i in (1..=ntp).rev() {
        if x[n - 1] >= tp5[i - 1] {
            break; // go to 9
        }
        ind[(l - 1) as usize] = 0;
        l -= 1;
    }
    // ── label 9 ─────────────────────────────────────────────────────────────
    if l <= 0 {
        return ind;
    }
    if x[n - 1] == tp5[(l - 1) as usize] {
        ind[(l - 1) as usize] = n;
        l -= 1;
    }
    let mut k = n as isize - 1;

    loop {
        // ── label 11 ────────────────────────────────────────────────────────
        if l <= 0 {
            return ind;
        }
        let trip = k.max(0);
        let mut matched = false;
        for _ in 0..trip {
            if k >= 1 && x[(k - 1) as usize] <= tp5[(l - 1) as usize] {
                ind[(l - 1) as usize] = (k + 1) as usize;
                l -= 1;
                matched = true;
                break; // go to 11
            }
            k -= 1;
        }
        if matched {
            continue;
        }
        for i in 1..=l {
            ind[(i - 1) as usize] = 0;
        }
        return ind;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_events_gives_flat_curve() {
        let out = cinc(&[1.0, 2.0, 3.0], &[0, 0, 0], &[0, 0, 0]);
        assert_eq!(out.x, vec![0.0, 3.0]);
        assert_eq!(out.f, vec![0.0, 0.0]);
    }

    #[test]
    fn single_event_doubles_the_corner() {
        // one failure of the cause at t = 2 out of 3 at risk
        let out = cinc(&[1.0, 2.0, 3.0], &[0, 1, 0], &[0, 1, 0]);
        assert_eq!(out.x, vec![0.0, 2.0, 2.0, 3.0]);
        assert!((out.f[2] - 0.5).abs() < 1e-12); // fk = 1, nd1 = 1, rs = 2
        assert_eq!(out.f[1], 0.0);
    }
}
