//! Bivariate fit2 入口：对齐原版 fit_sequence
//! `diffevo-fast → neldermead-fast → brute1-fast → brent1-fast`（全程 gaussian cost）。
//!
//! univariate 约束（来自两个 fit1）全程固定；自由参数 3 个：rho_beta、rho_zero、pi12。
//! brute1/brent1 固定 rg 与 rho_zero，沿 rg=const 曲线 1D 精修 pi12。

use crate::bivariate::cost::bivariate_cost_gaussian;
use crate::bivariate::data::BivariateData;
use crate::bivariate::optimizers::{brent1, brute1, differential_evolution, nelder_mead};
use crate::bivariate::parametrize::{ConstRgRhoZero, NaturalAxis};
use crate::bivariate::params::UnivariateConstraint;
use crate::bivariate::result::BivariateFitResult;
use crate::bivariate::sampling::bivariate_cost_sampling;
use crate::bivariate::transforms::logit_bounded;

/// fit2 配置，默认值对齐原版 `bivar_mixer/cli.py`。
#[derive(Debug, Clone)]
pub struct Fit2Config {
    /// diffevo-fast 重复次数（原版 `--diffevo-fast-repeats`，默认 20）。
    pub diffevo_repeats: usize,
    pub diffevo_popsize: usize,
    pub diffevo_max_gen: usize,
    pub diffevo_tol: f64,
    pub seed: u64,
    /// Nelder-Mead 初始单纯形边长。
    pub nm_step: f64,
    /// Nelder-Mead 收敛（原版 xatol=1e-4, fatol=1e-7）。
    pub nm_xatol: f64,
    pub nm_fatol: f64,
    pub nm_max_iter: usize,
    /// brute1 网格点数（原版 Ns=20）。
    pub brute_ns: usize,
    /// brent1 收敛（scipy 默认 tol=1.48e-8）。
    pub brent_xtol: f64,
    pub brent_max_iter: usize,
    /// brute1/brent1 是否用 sampling cost（true=打破 pi12/rho_beta 退化；diffevo/neldermead 仍 gaussian）。
    pub sampling: bool,
    /// sampling cost 的 Monte Carlo 配置数（原版 `--kmax`，默认 20000）。
    pub k_max: usize,
}

impl Default for Fit2Config {
    fn default() -> Self {
        Self {
            diffevo_repeats: 20,
            diffevo_popsize: 15,
            diffevo_max_gen: 1000,
            diffevo_tol: 0.01,
            seed: 123,
            nm_step: 0.5,
            nm_xatol: 1e-4,
            nm_fatol: 1e-7,
            nm_max_iter: 1200,
            brute_ns: 20,
            brent_xtol: 1.48e-8,
            brent_max_iter: 500,
            sampling: false,
            k_max: 20000,
        }
    }
}

/// 跑完整 fit2：DE×repeats → NM → brute1 → brent1，返回派生结果。
pub fn fit2(
    data: &BivariateData,
    c1: UnivariateConstraint,
    c2: UnivariateConstraint,
    cfg: &Fit2Config,
) -> BivariateFitResult {
    let na = NaturalAxis::new(c1, c2);
    let bounds = na.de_bounds();
    let cost_na = |x: &[f64]| bivariate_cost_gaussian(data, &na.vec_to_params(x));

    // 1. diffevo-fast × repeats，取最优
    let mut best_x: Option<Vec<f64>> = None;
    let mut best_cost = f64::INFINITY;
    for rep in 0..cfg.diffevo_repeats {
        let x = differential_evolution(
            &cost_na,
            &bounds,
            cfg.diffevo_popsize,
            cfg.diffevo_max_gen,
            cfg.diffevo_tol,
            cfg.seed + rep as u64,
        );
        let c = cost_na(&x);
        if c < best_cost {
            best_cost = c;
            best_x = Some(x);
        }
    }

    // 2. neldermead-fast（从 DE 最优点出发）
    let x0 = best_x.unwrap_or_else(|| vec![0.0, 0.0, 0.0]);
    let x_nm = nelder_mead(&cost_na, &x0, cfg.nm_step, cfg.nm_xatol, cfg.nm_fatol, cfg.nm_max_iter);
    let params_nm = na.vec_to_params(&x_nm);

    // 3. brute1-fast：固定 rg、rho_zero，"忘记" pi12，在 [min_pi12, max_pi12] 全程扫描。
    //    cost calculator：sampling=true 时用 MC 采样 cost（打破退化），否则 gaussian。
    let rg_const = params_nm.rg();
    let rho_zero_const = params_nm.rho_zero;
    let pr = ConstRgRhoZero::new(c1, c2, rg_const, rho_zero_const);
    let cost_pr = |x: &[f64]| {
        let params = pr.vec_to_params(x);
        if cfg.sampling {
            bivariate_cost_sampling(data, &params, cfg.k_max, cfg.seed)
        } else {
            bivariate_cost_gaussian(data, &params)
        }
    };

    // 1D logit 坐标作为 pi12 的函数（与 pr.params_to_vec 一致）
    let coord_of_pi12 = |pi12: f64| {
        let frac = (pi12 - pr.min_pi12) / (pr.max_pi12 - pr.min_pi12);
        logit_bounded(frac)
    };
    let lo = coord_of_pi12(0.99 * pr.min_pi12 + 0.01 * pr.max_pi12);
    let hi = coord_of_pi12(0.01 * pr.min_pi12 + 0.99 * pr.max_pi12);
    let (x_brute, _) = brute1(|x| cost_pr(&[x]), lo, hi, cfg.brute_ns);
    let params_brute = pr.vec_to_params(&[x_brute]);

    // 4. brent1-fast：括号 (min, brute结果, max)，parabolic 精修。
    let bracket_mid = coord_of_pi12(params_brute.pi[2]);
    let bracket_lo = coord_of_pi12(pr.min_pi12);
    let bracket_hi = coord_of_pi12(pr.max_pi12);
    let params_final = match brent1(
        |x| cost_pr(&[x]),
        bracket_lo,
        bracket_mid,
        bracket_hi,
        cfg.brent_xtol,
        cfg.brent_max_iter,
    ) {
        Some((xmin, _)) => pr.vec_to_params(&[xmin]),
        None => params_brute, // 括号无效（scipy 抛 ValueError）→ 回退 brute 结果
    };

    let loglike = if cfg.sampling {
        bivariate_cost_sampling(data, &params_final, cfg.k_max, cfg.seed)
    } else {
        bivariate_cost_gaussian(data, &params_final)
    };
    BivariateFitResult::derive(data, params_final, loglike)
}
