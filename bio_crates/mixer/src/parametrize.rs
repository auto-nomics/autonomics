//! 参数化变换：把有物理约束的模型参数映射到无约束空间，供优化器使用。
//!
//! MiXeR 的三个自由参数都有约束（pi ∈ (0,1)，sig2_beta/sig2_zero > 0），
//! 而 Nelder-Mead / 差分进化这类无约束优化器需要在一个开的空间里搜索。
//! 解法是用一个单调的双射把参数映到 (-∞, +∞)：
//!   - 正数参数 (sig2_zero, sig2_beta)：用 log 变换
//!   - (0,1) 参数 (pi)：用 logit 变换
//!
//! 优化全程在无约束空间 `[x1, x2, x3]` 上进行；每次需要计算 cost 时，
//! 再用 `from_unconstrained` 反变换回真实参数喂给 cost function。

use crate::params::UnivariateParams;

/// 差分进化 (differential evolution) 的搜索边界。
///
/// 注意：这里的值是**原始参数空间**的上下界，不是无约束空间的。
/// 原版 `bivar_mixer/cli.py` 在调用 DE 前会先用 `to_unconstrained`
/// 把这些边界变换到无约束空间，再交给优化器。
///
/// 边界顺序与无约束向量一致：`[sig2_zero, sig2_beta, pi]`。
///
/// - `sig2_zero` ∈ [0.9, 2.5]：截断方差接近 1（类似 LDSC intercept，
///   健壮 GWAS 通常 1.0~1.1），下界不给太小避免跑到无意义的极端值。
/// - `sig2_beta` ∈ [5e-6, 5e-2]：因果效应量方差，量级很小。
/// - `pi` ∈ [5e-5, 5e-1]：causal SNP 比例。
pub const DE_BOUNDS: &[(f64, f64); 3] = &[(0.9, 2.5), (5e-6, 5e-2), (5e-5, 5e-1)];

/// logit 变换：(0,1) → (−∞,+∞)。
fn logit(p: f64) -> f64 {
    (p / (1.0 - p)).ln()
}

/// 把原始参数空间的 DE 边界转换到无约束空间（DE 实际搜索的边界）。
///
/// 顺序与 `DE_BOUNDS` 一致：`[sig2_zero, sig2_beta, pi]`，对应
/// `[log(·), log(·), logit(·)]`。原版 `cli.py` 就是把原始边界经
/// `params_to_vec` 变换后交给 scipy DE。
pub fn de_bounds_unconstrained() -> [(f64, f64); 3] {
    [
        (DE_BOUNDS[0].0.ln(), DE_BOUNDS[0].1.ln()),
        (DE_BOUNDS[1].0.ln(), DE_BOUNDS[1].1.ln()),
        (logit(DE_BOUNDS[2].0), logit(DE_BOUNDS[2].1)),
    ]
}

/// 模型参数 → 无约束空间向量 `[log(sig2_zero), log(sig2_beta), logit(pi)]`。
///
/// 返回固定长度 3 的数组，顺序为 `[sig2_zero, sig2_beta, pi]`（与 `DE_BOUNDS` 对齐）。
///
/// 提示：`logit(p) = ln(p / (1 - p))`。
pub fn to_unconstrained(params: &UnivariateParams) -> [f64; 3] {
    [
        params.sig2_zero.ln(),
        params.sig2_beta.ln(),
        (params.pi / (1.0 - params.pi)).ln(), // = logit(pi);
    ]
}

/// 无约束空间向量 → 模型参数（`to_unconstrained` 的逆）。
///
/// 提示：`logistic(x) = 1 / (1 + exp(-x))`，即 `logit` 的反函数。
/// 注意 `exp` 在输入很大时会溢出成 `inf`，可用 `x.min(700.0)` 钳制避免 NaN。
pub fn from_unconstrained(x: [f64; 3]) -> UnivariateParams {
    UnivariateParams {
        pi: 1.0 / (1.0 + (-x[2]).exp()),
        sig2_beta: x[1].exp(),
        sig2_zero: x[0].exp(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let p = UnivariateParams::new(0.01, 0.001, 1.2);
        let p2 = from_unconstrained(to_unconstrained(&p));
        assert!((p2.sig2_zero - p.sig2_zero).abs() < 1e-12);
        assert!((p2.sig2_beta - p.sig2_beta).abs() < 1e-12);
        assert!((p2.pi - p.pi).abs() < 1e-12);
    }

    #[test]
    fn extreme_unconstrained_no_nan() {
        // 极端无约束值不应产生 NaN/panic
        let p = from_unconstrained([-700.0, 700.0, 700.0]);
        assert!(p.sig2_zero.is_finite() || p.sig2_zero == 0.0);
        assert!(p.sig2_beta.is_finite());
        assert!(p.pi > 0.0 && p.pi <= 1.0);
    }
}
