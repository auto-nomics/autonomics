//! Kaplan–Meier estimate of the **censoring** distribution, evaluated at `t-`.
//!
//! This module reproduces `cmprsk.R:66-82` bit-for-bit:
//!
//! ```r
//! for (k in 1:ncg) {
//!   u <- survfit(Surv(ftime, cenind) ~ 1, subset = cengroup == k)
//!   u <- approx(c(min(0, u$time) - 10 * .Machine$double.eps,
//!                 c(u$time, max(u$time) * (1 + 10 * .Machine$double.eps))),
//!               c(1, u$surv, 0),
//!               xout = ftime * (1 - 100 * .Machine$double.eps),
//!               method = 'constant', f = 0, rule = 2)
//!   uuu[k, 1:length(u$y)] <- u$y
//! }
//! ```
//!
//! Two details matter and are easy to get wrong:
//!
//! * the *event* for this KM fit is **being censored** (`cenind == 1`), because
//!   `crr` reweights competing-cause risk-set contributions by the censoring
//!   survivor function `G(t)`;
//! * every group's curve is evaluated at **all** `n` failure times, not just the
//!   times of that group's own members — hence the `ncg × n` result.
//!
//! The `(1 - 100ε)` shift on the query points, combined with `method="constant"`
//! and `f=0`, is what makes the lookup return `G(t-)` (left-continuous) rather
//! than `G(t)`.

/// `.Machine$double.eps` in R.
pub const DBL_EPS: f64 = f64::EPSILON;

/// A step function stored as knot/value pairs, evaluated the way R's
/// `approx(method = "constant", f = 0, rule = 2)` does.
#[derive(Debug, Clone)]
pub struct StepFn {
    /// Knot positions, ascending.
    pub x: Vec<f64>,
    /// Value taken on `[x[j], x[j+1])`.
    pub y: Vec<f64>,
}

impl StepFn {
    /// Evaluate at `q`.
    ///
    /// * `q <= x[0]` → `y[0]` (rule = 2 clamps below)
    /// * `q >= x[last]` → `y[last]` (rule = 2 clamps above)
    /// * otherwise the value of the last knot at or before `q` (`f = 0`)
    pub fn eval(&self, q: f64) -> f64 {
        let n = self.x.len();
        if n == 0 {
            return f64::NAN;
        }
        if q <= self.x[0] {
            return self.y[0];
        }
        if q >= self.x[n - 1] {
            return self.y[n - 1];
        }
        // Largest j with x[j] <= q. `partition_point` gives the count of
        // elements satisfying the predicate, so it is that index + 1.
        let j = self.x.partition_point(|&xv| xv <= q) - 1;
        self.y[j]
    }
}

/// Kaplan–Meier survivor curve for the censoring distribution within one group.
///
/// `times` must be ascending; `cenind[i] == 1` marks a censored observation
/// (which is the *event* for this fit). Returns `(distinct_times, surv)` — the
/// `time` / `surv` components of `survival::survfit`, which reports one row per
/// distinct observed time whether or not an event occurred there.
fn km_censoring(times: &[f64], cenind: &[u8]) -> (Vec<f64>, Vec<f64>) {
    let m = times.len();
    let mut ut: Vec<f64> = Vec::new();
    let mut surv: Vec<f64> = Vec::new();
    let mut s = 1.0_f64;
    let mut n_risk = m as f64;

    let mut i = 0usize;
    while i < m {
        let t = times[i];
        let mut j = i;
        let mut d = 0.0_f64; // censoring "events" at t
        while j < m && times[j] == t {
            if cenind[j] == 1 {
                d += 1.0;
            }
            j += 1;
        }
        let n_at = (j - i) as f64;
        if d > 0.0 {
            s *= (n_risk - d) / n_risk;
        }
        ut.push(t);
        surv.push(s);
        n_risk -= n_at;
        i = j;
    }
    (ut, surv)
}

/// Build the `ncg × n` matrix `uuu` of censoring-survivor values `G_k(t_i-)`.
///
/// * `ftime` — all failure/censoring times, **ascending** (the caller sorts).
/// * `cenind` — `1` if the observation is censored, `0` otherwise.
/// * `cengroup` — 0-based censoring-group index for each observation.
/// * `ncg` — number of censoring groups.
pub fn censoring_weights(
    ftime: &[f64],
    cenind: &[u8],
    cengroup: &[usize],
    ncg: usize,
) -> Vec<Vec<f64>> {
    let n = ftime.len();
    let query: Vec<f64> = ftime.iter().map(|&t| t * (1.0 - 100.0 * DBL_EPS)).collect();

    let mut uuu = vec![vec![0.0_f64; n]; ncg];
    for (k, row) in uuu.iter_mut().enumerate() {
        // Members of group k, already in ascending-time order.
        let mut gt: Vec<f64> = Vec::new();
        let mut gc: Vec<u8> = Vec::new();
        for i in 0..n {
            if cengroup[i] == k {
                gt.push(ftime[i]);
                gc.push(cenind[i]);
            }
        }
        if gt.is_empty() {
            // R would error out of survfit; defensively leave the row at zero.
            continue;
        }
        let (ut, surv) = km_censoring(&gt, &gc);

        // Knot vector: [min(0, t1) - 10ε, t1 .. tm, tm * (1 + 10ε)]
        let t_min = ut[0];
        let t_max = ut[ut.len() - 1];
        let mut x = Vec::with_capacity(ut.len() + 2);
        let mut y = Vec::with_capacity(ut.len() + 2);
        x.push(t_min.min(0.0) - 10.0 * DBL_EPS);
        y.push(1.0);
        for (t, s) in ut.iter().zip(surv.iter()) {
            x.push(*t);
            y.push(*s);
        }
        x.push(t_max * (1.0 + 10.0 * DBL_EPS));
        y.push(0.0);

        let step = StepFn { x, y };
        for (i, q) in query.iter().enumerate() {
            row[i] = step.eval(*q);
        }
    }
    uuu
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_fn_is_left_continuous_at_knots() {
        let f = StepFn {
            x: vec![0.0, 1.0, 2.0],
            y: vec![1.0, 0.5, 0.0],
        };
        assert_eq!(f.eval(-1.0), 1.0); // rule = 2 below
        assert_eq!(f.eval(0.5), 1.0);
        assert_eq!(f.eval(1.0), 0.5); // f = 0 → value at the knot
        assert_eq!(f.eval(1.9), 0.5);
        assert_eq!(f.eval(5.0), 0.0); // rule = 2 above
    }

    #[test]
    fn km_matches_hand_computation() {
        // times 1,2,3,4 with censoring events at 2 and 4.
        let (ut, s) = km_censoring(&[1.0, 2.0, 3.0, 4.0], &[0, 1, 0, 1]);
        assert_eq!(ut, vec![1.0, 2.0, 3.0, 4.0]);
        assert!((s[0] - 1.0).abs() < 1e-15);
        assert!((s[1] - 2.0 / 3.0).abs() < 1e-15); // 1 * (3-1)/3
        assert!((s[2] - 2.0 / 3.0).abs() < 1e-15);
        assert!((s[3] - 0.0).abs() < 1e-15); // (1-1)/1
    }

    #[test]
    fn weights_evaluate_at_t_minus() {
        // Single group; G jumps down at t = 2, so the weight *at* t = 2 must
        // still be the pre-jump value 1.0 (left continuity).
        let ftime = [1.0, 2.0, 3.0, 4.0];
        let cenind = [0u8, 1, 0, 1];
        let w = censoring_weights(&ftime, &cenind, &[0, 0, 0, 0], 1);
        assert!((w[0][1] - 1.0).abs() < 1e-15);
        assert!((w[0][2] - 2.0 / 3.0).abs() < 1e-15);
    }
}
