//! 参数边界变换，clean-room 复刻原版 `common/utils.py` 的 `_xxx_bounded` 系列。
//!
//! 把有物理约束的参数映到 (−∞,+∞) 供无约束优化器使用。所有变换都做 epsval
//! 裁剪，避免 ±0、±1、±inf 处的数值发散。`epsval = f64::EPSILON`，与
//! numpy `finfo(float).eps` 一致。

/// 与 numpy `finfo(float).eps` 一致。
pub const EPSVAL: f64 = f64::EPSILON; // 2.220446049250313e-16
pub const MAXVAL: f64 = f64::MAX;

/// log 变换：[0,+∞) → (−∞,+∞)，裁剪到 [epsval, maxval]。
pub fn log_bounded(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let x = x.clamp(EPSVAL, MAXVAL);
    x.ln()
}

/// exp 变换：(−∞,+∞) → [0,+∞)，结果裁剪到 [epsval, maxval]。
pub fn exp_bounded(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let y = x.exp();
    y.clamp(EPSVAL, MAXVAL)
}

/// logit 变换：(0,1) → (−∞,+∞)。先裁剪 x 到 [epsval, 1−epsval]，再 log(x/(1−x))。
pub fn logit_bounded(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let x = x.clamp(EPSVAL, 1.0 - EPSVAL);
    log_bounded(x / (1.0 - x))
}

/// logistic 变换：(−∞,+∞) → (0,1)。= exp_bounded(x)/(1+exp_bounded(x))，再裁剪。
pub fn logistic_bounded(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let e = exp_bounded(x);
    let y = e / (1.0 + e);
    y.clamp(EPSVAL, 1.0 - EPSVAL)
}

/// [−1,1] ↔ (−∞,+∞) 的"安全 atanh/tanh"。
///
/// 正向（invflag=false）：`0.5·logit(0.5x+0.5)`。性质 atanh(x)~x 在 x≈0 附近成立，
/// 且用 logit 的裁剪避免 ±1 处发散。
/// 逆向（invflag=true）：`2·logistic(2x)−1`。
pub fn arctanh_tanh(x: f64, invflag: bool) -> f64 {
    if invflag {
        2.0 * logistic_bounded(2.0 * x) - 1.0
    } else {
        0.5 * logit_bounded(0.5 * x + 0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_logistic_roundtrip() {
        for &p in &[0.01, 0.1, 0.5, 0.9, 0.99] {
            let y = logit_bounded(p);
            let q = logistic_bounded(y);
            assert!((q - p).abs() < 1e-9, "{p} -> {y} -> {q}");
        }
    }

    #[test]
    fn arctanh_tanh_roundtrip() {
        for &r in &[0.0, 0.1, -0.5, 0.9, -0.95] {
            let y = arctanh_tanh(r, false);
            let r2 = arctanh_tanh(y, true);
            assert!((r2 - r).abs() < 1e-9, "{r} -> {y} -> {r2}");
        }
    }

    #[test]
    fn arctanh_small_is_identity() {
        // atanh(x) ~ x for small x
        assert!(arctanh_tanh(0.01, false).abs() < 0.011);
    }

    #[test]
    fn boundary_clamps_no_nan() {
        assert!(logit_bounded(0.0).is_finite());
        assert!(logit_bounded(1.0).is_finite());
        assert!(arctanh_tanh(1.0, false).is_finite());
        assert!(arctanh_tanh(-1.0, false).is_finite());
        assert!(logistic_bounded(1000.0) < 1.0);
        assert!(logistic_bounded(-1000.0) > 0.0);
    }
}
