//! Bivariate 参数化：在固定 univariate 约束下，把自由参数映到无约束空间。
//!
//! 复刻原版 `bivar_mixer/utils.py` 的两个核心参数化：
//!   - `natural_axis`（3D）：自由维度 = [rho_beta, rho_zero, pi12/max_pi12]，
//!     供 diffevo-fast / neldermead-fast 使用。
//!   - `constRG_constRHOZERO`（1D）：固定 rg 与 rho_zero，仅扫 pi12，
//!     供 brute1-fast / brent1-fast 使用。
//!
//! 约定：pi[0]=仅trait1、pi[1]=仅trait2、pi[2]=共有（pi12）。
//! univariate 约束里的 `pi` 是该 trait 的总 causal 比例（pi_only + pi12）。

use crate::bivariate::params::{BivariateParams, UnivariateConstraint};
use crate::bivariate::transforms::{arctanh_tanh, logistic_bounded, logit_bounded};

/// 固定 univariate 约束 c1/c2，自由调节 (rho_beta, rho_zero, pi12)。
pub struct NaturalAxis {
    pub c1: UnivariateConstraint,
    pub c2: UnivariateConstraint,
    /// pi12 的上限 = min(c1.pi, c2.pi)。
    pub max_pi12: f64,
}

impl NaturalAxis {
    pub fn new(c1: UnivariateConstraint, c2: UnivariateConstraint) -> Self {
        Self {
            max_pi12: c1.pi.min(c2.pi),
            c1,
            c2,
        }
    }

    /// params → 无约束向量 [atanh(rho_beta), atanh(rho_zero), logit(pi12/max_pi12)]。
    pub fn params_to_vec(&self, p: &BivariateParams) -> [f64; 3] {
        [
            arctanh_tanh(p.rho_beta, false),
            arctanh_tanh(p.rho_zero, false),
            logit_bounded(p.pi[2] / self.max_pi12),
        ]
    }

    /// 无约束向量 → params。
    pub fn vec_to_params(&self, x: &[f64]) -> BivariateParams {
        let rho_beta = arctanh_tanh(x[0], true);
        let rho_zero = arctanh_tanh(x[1], true);
        let pi12 = self.max_pi12 * logistic_bounded(x[2]);
        BivariateParams {
            pi: [self.c1.pi - pi12, self.c2.pi - pi12, pi12],
            sig2_beta: [self.c1.sig2_beta, self.c2.sig2_beta],
            sig2_zero: [self.c1.sig2_zero, self.c2.sig2_zero],
            rho_beta,
            rho_zero,
        }
    }

    /// diffevo-fast 的搜索边界（无约束空间）。
    ///
    /// 左端：pi12=0.05·max_pi12、rho_beta=−0.95、rho_zero=−0.95；
    /// 右端：pi12=0.95·max_pi12、rho_beta=0.95、rho_zero=0.95。
    pub fn de_bounds(&self) -> [(f64, f64); 3] {
        let left = self.params_to_vec(&BivariateParams {
            pi: [self.c1.pi - 0.05 * self.max_pi12, self.c2.pi - 0.05 * self.max_pi12, 0.05 * self.max_pi12],
            sig2_beta: [self.c1.sig2_beta, self.c2.sig2_beta],
            sig2_zero: [self.c1.sig2_zero, self.c2.sig2_zero],
            rho_beta: -0.95,
            rho_zero: -0.95,
        });
        let right = self.params_to_vec(&BivariateParams {
            pi: [self.c1.pi - 0.95 * self.max_pi12, self.c2.pi - 0.95 * self.max_pi12, 0.95 * self.max_pi12],
            sig2_beta: [self.c1.sig2_beta, self.c2.sig2_beta],
            sig2_zero: [self.c1.sig2_zero, self.c2.sig2_zero],
            rho_beta: 0.95,
            rho_zero: 0.95,
        });
        [(left[0], right[0]), (left[1], right[1]), (left[2], right[2])]
    }
}

/// 固定 rg 与 rho_zero，仅 1D 调节 pi12。供 brute1/brent1。
pub struct ConstRgRhoZero {
    pub c1: UnivariateConstraint,
    pub c2: UnivariateConstraint,
    pub const_rg: f64,
    pub const_rho_zero: f64,
    pub max_pi12: f64,
    /// pi12 下限 = |rg|·√(c1.pi·c2.pi)，保证 |rho_beta|≤1。
    pub min_pi12: f64,
}

impl ConstRgRhoZero {
    pub fn new(
        c1: UnivariateConstraint,
        c2: UnivariateConstraint,
        const_rg: f64,
        const_rho_zero: f64,
    ) -> Self {
        let max_pi12 = c1.pi.min(c2.pi);
        let min_pi12 = const_rg.abs() * (c1.pi * c2.pi).sqrt();
        Self {
            c1,
            c2,
            const_rg,
            const_rho_zero,
            max_pi12,
            min_pi12,
        }
    }

    /// params → 1D 无约束坐标 = logit((pi12 − min_pi12)/(max_pi12 − min_pi12))。
    pub fn params_to_vec(&self, p: &BivariateParams) -> [f64; 1] {
        let frac = (p.pi[2] - self.min_pi12) / (self.max_pi12 - self.min_pi12);
        [logit_bounded(frac)]
    }

    /// 1D 坐标 → params。rho_beta 由 rg/pi12 反推：rho_beta = rg·√(c1.pi·c2.pi)/pi12。
    pub fn vec_to_params(&self, x: &[f64]) -> BivariateParams {
        let pi12 = self.min_pi12 + logistic_bounded(x[0]) * (self.max_pi12 - self.min_pi12);
        let rho_beta = self.const_rg * (self.c1.pi * self.c2.pi).sqrt() / pi12;
        BivariateParams {
            pi: [self.c1.pi - pi12, self.c2.pi - pi12, pi12],
            sig2_beta: [self.c1.sig2_beta, self.c2.sig2_beta],
            sig2_zero: [self.c1.sig2_zero, self.c2.sig2_zero],
            rho_beta,
            rho_zero: self.const_rho_zero,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctr() -> (UnivariateConstraint, UnivariateConstraint) {
        (
            UnivariateConstraint { pi: 0.0015, sig2_beta: 0.04, sig2_zero: 1.0 },
            UnivariateConstraint { pi: 0.0025, sig2_beta: 0.05, sig2_zero: 0.99 },
        )
    }

    #[test]
    fn natural_axis_roundtrip() {
        let (c1, c2) = ctr();
        let na = NaturalAxis::new(c1, c2);
        let p = BivariateParams {
            pi: [0.001, 0.002, 0.0005],
            sig2_beta: [0.04, 0.05],
            sig2_zero: [1.0, 0.99],
            rho_beta: 0.3,
            rho_zero: -0.2,
        };
        let x = na.params_to_vec(&p);
        let q = na.vec_to_params(&x);
        assert!((q.rho_beta - 0.3).abs() < 1e-9);
        assert!((q.rho_zero - (-0.2)).abs() < 1e-9);
        assert!((q.pi[2] - 0.0005).abs() < 1e-9);
        // pi[0] = c1.pi - pi12
        assert!((q.pi[0] - (0.0015 - 0.0005)).abs() < 1e-9);
    }

    #[test]
    fn const_rg_roundtrip_preserves_rg() {
        let (c1, c2) = ctr();
        let rg = 0.4;
        let pr = ConstRgRhoZero::new(c1, c2, rg, 0.1);
        let p = BivariateParams {
            pi: [0.0, 0.0, 0.0008],
            sig2_beta: [0.04, 0.05],
            sig2_zero: [1.0, 0.99],
            rho_beta: 0.0,
            rho_zero: 0.1,
        };
        let x = pr.params_to_vec(&p);
        let q = pr.vec_to_params(&x);
        assert!((q.rg() - rg).abs() < 1e-9, "rg {} vs {}", q.rg(), rg);
        assert!((q.rho_zero - 0.1).abs() < 1e-9);
    }
}
