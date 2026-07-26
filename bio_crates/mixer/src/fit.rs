//! Univariate MiXeR 的 fit1 入口：对齐原版流水线（diffevo-fast ×20 → neldermead）。

use crate::data::UnivariateSufficient;
use crate::optimizer::{cost_of_vec, differential_evolution, nelder_mead};
use crate::parametrize;
use crate::result::FitResult;

/// fit1 的配置（超参数），默认值对齐原版 `bivar_mixer/cli.py`。
#[derive(Debug, Clone)]
pub struct FitConfig {
    /// 差分进化重复次数（原版 `--diffevo-fast-repeats`，默认 20）
    pub diffevo_repeats: usize,
    /// 差分进化种群倍数（scipy popsize，实际种群 = 倍数 × 维度）
    pub diffevo_popsize: usize,
    /// 差分进化最大代数
    pub diffevo_max_gen: usize,
    /// 差分进化收敛阈值（原版 `tol=0.01`）
    pub diffevo_tol: f64,
    /// 随机种子
    pub seed: u64,
    /// Nelder-Mead 初始单纯形边长
    pub nm_step: f64,
    /// Nelder-Mead 收敛阈值（原版 `fatol=1e-7, xatol=1e-4`）
    pub nm_tol: f64,
    /// Nelder-Mead 最大迭代数（原版 `maxiter=1200`）
    pub nm_max_iter: usize,
}

impl Default for FitConfig {
    fn default() -> Self {
        Self {
            diffevo_repeats: 20,
            diffevo_popsize: 15,
            diffevo_max_gen: 100,
            diffevo_tol: 0.01,
            seed: 1,
            nm_step: 0.5,
            nm_tol: 1e-7,
            nm_max_iter: 1200,
        }
    }
}

/// 跑完整 fit1：差分进化（×repeats）全局粗搜 → Nelder-Mead 精修。
///
/// 对齐原版 `apply_univariate_fit_sequence(['diffevo-fast', 'neldermead'])`：
/// 1. DE 重复 `diffevo_repeats` 次（每次种子不同），取 cost 最低者
/// 2. 从 DE 最优点出发，Nelder-Mead 精细收敛
pub fn fit1(data: &UnivariateSufficient, cfg: &FitConfig) -> FitResult {
    let bounds = parametrize::de_bounds_unconstrained();

    // 1. 差分进化 × repeats，取最优
    let mut best_x: Option<[f64; 3]> = None;
    let mut best_cost = f64::INFINITY;
    for rep in 0..cfg.diffevo_repeats {
        let x = differential_evolution(
            data,
            &bounds,
            cfg.diffevo_popsize,
            cfg.diffevo_max_gen,
            cfg.diffevo_tol,
            cfg.seed + rep as u64,
        );
        let c = cost_of_vec(data, x);
        if c < best_cost {
            best_cost = c;
            best_x = Some(x);
        }
    }

    // 2. Nelder-Mead 精修
    let x0 = best_x.unwrap_or([0.0, -7.0, -4.6]);
    let x_best = nelder_mead(data, x0, cfg.nm_step, cfg.nm_tol, cfg.nm_max_iter);
    let params = parametrize::from_unconstrained(x_best);
    let loglike = crate::cost::univariate_cost_sufficient(data, &params);
    FitResult::derive(data, params, loglike)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::ChromData;
    // 注意：fit1 的端到端验证由 `fit1_recovers_synthetic_params`（合成数据参数回收）
    // 和 tests/cross_validation.rs（真实 HM3 数据 vs 原版金标准）覆盖。

    #[test]
    fn fit1_recovers_synthetic_params() {
        use crate::params::UnivariateParams;
        use crate::simulate::simulate;

        // 真实参数（用来造数据）。选使得信号可辨的参数：
        //   E[δ²] ≈ 邻居数·(N·h·r²)·(π·σ²_β) = 5·(10000·0.4·0.5)·(0.05·0.002) ≈ 1.0
        //   即信号 std≈1，与噪声 std=1 相当，可被拟合识别。
        let true_params = UnivariateParams::new(0.05, 0.002, 1.0);

        // 1000 SNP，密集 LD（每个 SNP 与后 5 个邻居有 LD）
        let n = 1000;
        let h: Vec<f64> = (0..n).map(|i| 0.4 + 0.0001 * (i as f64)).collect();
        let nn: Vec<f64> = vec![10000.0; n];
        let triples: Vec<(u32, u32, f64)> = (0..n as u32)
            .flat_map(|i| (1..=5).map(move |k| (i, i + k, 0.5)))
            .filter(|&(_, b, _)| b < n as u32)
            .collect();
        let mut data = ChromData::new(vec![0.0; n], nn, h, &triples);

        // 用真实参数采样 z
        data.z = simulate(&data, &true_params, 42);

        // 压缩成充分统计量后拟合（fit1 不再直接吃 ChromData）。
        let suff = crate::data::UnivariateSufficient::from_chrom_data(&data);
        let result = fit1(&suff, &FitConfig::default());
        // cost_at_true 仍用逐邻居参考实现做独立交叉校验（ChromData 版 cost 保留）。
        let cost_at_true = crate::cost::univariate_cost_gaussian(&data, &true_params);

        println!(
            "真实:  pi={:.4}, sig2_beta={:.5}, sig2_zero={:.4}",
            true_params.pi, true_params.sig2_beta, true_params.sig2_zero
        );
        println!(
            "拟合:  pi={:.4}, sig2_beta={:.5}, sig2_zero={:.4}",
            result.params.pi, result.params.sig2_beta, result.params.sig2_zero
        );
        println!(
            "cost: 拟合={:.4}  真实参数处={:.4}",
            result.loglike, cost_at_true
        );
        println!(
            "派生: h2={:.4}, nc={:.1}, nc@p9={:.1}, AIC={:.2}, BIC={:.2}",
            result.h2, result.nc, result.nc_p9, result.aic, result.bic
        );

        // 拟合 cost 不应高于真实参数处的 cost
        assert!(result.loglike <= cost_at_true + 1e-6);
        assert!(result.params.sig2_zero > 0.0);
        assert!(result.params.sig2_beta > 0.0);
        assert!(result.params.pi > 0.0 && result.params.pi < 1.0);
        assert!(result.h2 >= 0.0);
        assert!(result.aic.is_finite());
    }
}
