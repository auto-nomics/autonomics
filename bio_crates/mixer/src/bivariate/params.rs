//! Bivariate MiXeR 参数结构。
//!
//! 模型把每个 SNP 分到 3 个互斥的因果成分：
//!   - 仅 trait1 causal（比例 pi[0]）
//!   - 仅 trait2 causal（比例 pi[1]）
//!   - 两个 trait 共有 causal（比例 pi[2]）
//! 共有成分内，两个 trait 的效应相关系数为 `rho_beta`。
//! 两个 trait 的残差（null）相关系数为 `rho_zero`。

/// 双变量模型的 9 个参数（fit2 输出）。
#[derive(Debug, Clone)]
pub struct BivariateParams {
    /// 3 个因果成分的比例：[仅trait1, 仅trait2, 共有]。
    /// 约束：每个 ≥0，三者之和 ≤ 1。
    pub pi: [f64; 3],
    /// 每个 trait 的因果效应方差：[trait1, trait2]。
    pub sig2_beta: [f64; 2],
    /// 每个 trait 的截断/残差方差（inflation）：[trait1, trait2]。
    pub sig2_zero: [f64; 2],
    /// 共有成分内的效应相关系数 ∈ [−1,1]。
    pub rho_beta: f64,
    /// 残差相关系数 ∈ [−1,1]。
    pub rho_zero: f64,
}

impl BivariateParams {
    /// 模型隐含的遗传相关 rg。
    ///
    /// `rg = rho_beta · pi12 / sqrt(pi1u · pi2u)`，
    /// 其中 pi1u = pi[0]+pi[2]、pi2u = pi[1]+pi[2]（各 trait 的总 causal 比例）。
    /// 这不是样本相关，是因子相关 rho_beta 经"共有负载比例"折减后的总体遗传相关。
    pub fn rg(&self) -> f64 {
        let pi1u = self.pi[0] + self.pi[2];
        let pi2u = self.pi[1] + self.pi[2];
        self.rho_beta * self.pi[2] / (pi1u * pi2u).sqrt()
    }
}

/// 来自 univariate fit1 的固定约束（fit2 全程不变）。
///
/// `pi` 是该 trait 的总 causal 比例（= pi_only + pi12）；
/// `sig2_beta`、`sig2_zero` 直接来自 fit1。
#[derive(Debug, Clone, Copy)]
pub struct UnivariateConstraint {
    pub pi: f64,
    pub sig2_beta: f64,
    pub sig2_zero: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rg_formula() {
        // pi=[0.001, 0.002, 0.0005], rho_beta=0.8
        let p = BivariateParams {
            pi: [0.001, 0.002, 0.0005],
            sig2_beta: [0.04, 0.05],
            sig2_zero: [1.0, 1.0],
            rho_beta: 0.8,
            rho_zero: 0.5,
        };
        let pi1u: f64 = 0.0015;
        let pi2u: f64 = 0.0025;
        let expect = 0.8 * 0.0005 / (pi1u * pi2u).sqrt();
        assert!((p.rg() - expect).abs() < 1e-12);
    }
}
