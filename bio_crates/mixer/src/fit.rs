//! Univariate MiXeR 的 fit1 入口：跑优化器，返回拟合结果。

use crate::cost::univariate_cost_gaussian;
use crate::data::ChromData;
use crate::optimizer::nelder_mead;
use crate::parametrize;
use crate::result::FitResult;

/// fit1 的配置（超参数）。
#[derive(Debug, Clone)]
pub struct FitConfig {
    /// 起始点（无约束空间）
    pub x0: [f64; 3],
    /// 初始单纯形边长
    pub step: f64,
    /// 收敛阈值
    pub tol: f64,
    /// 最大迭代数
    pub max_iter: usize,
}

impl Default for FitConfig {
    fn default() -> Self {
        // 起点对应 π≈0.01, σ²_β≈0.001, sig2_zero≈1.0 的无约束值
        Self {
            x0: [0.0, -7.0, -4.6],
            step: 0.5,
            tol: 1e-7,
            max_iter: 2000,
        }
    }
}

/// 跑单次 Nelder-Mead 拟合，返回最优参数与 cost。
pub fn fit1(data: &ChromData, cfg: &FitConfig) -> FitResult {
    let x_best = nelder_mead(data, cfg.x0, cfg.step, cfg.tol, cfg.max_iter);
    let params = parametrize::from_unconstrained(x_best);
    let loglike = univariate_cost_gaussian(data, &params);
    FitResult { params, loglike }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::ChromData;

    #[test]
    fn fit1_runs_and_returns_valid_params() {
        let triples = vec![(0, 1, 0.5), (1, 0, 0.3), (2, 1, 0.4)];
        let data = ChromData::new(
            vec![1.2, 0.8, 1.5],
            vec![100.0, 100.0, 100.0],
            vec![0.5, 0.4, 0.45],
            &triples,
        );
        let result = fit1(&data, &FitConfig::default());

        // 参数在物理约束内
        assert!(result.params.pi > 0.0 && result.params.pi < 1.0);
        assert!(result.params.sig2_beta > 0.0);
        assert!(result.params.sig2_zero > 0.0);
        // loglike 有限
        assert!(result.loglike.is_finite());
        println!(
            "拟合: pi={}, sig2_beta={}, sig2_zero={}, cost={}",
            result.params.pi, result.params.sig2_beta, result.params.sig2_zero, result.loglike
        );
    }
}
