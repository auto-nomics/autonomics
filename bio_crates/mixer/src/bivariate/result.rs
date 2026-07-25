//! Bivariate fit2 的输出结果与派生量。
//!
//! 复刻原版 `bivar_mixer/utils.py::_calculate_bivariate_uncertainty_funcs` 的
//! point_estimate 公式（fit2 不做 uncertainty sampling，只写 point_estimate）。

use crate::bivariate::data::BivariateData;
use crate::bivariate::params::BivariateParams;

/// 捕获 90% h² 所需的 causal 比例（原版 NCKoef = 0.319）。
const NC_COEF_P9: f64 = 0.319;
/// 自由度（含 univariate 6 + bivariate 3），原版 `enhance_optimize_result` 写死。
pub const COST_DF: i64 = 9;

#[derive(Debug, Clone)]
pub struct BivariateFitResult {
    pub params: BivariateParams,
    /// 最优参数处的负对数似然。
    pub loglike: f64,
    /// 残差相关。
    pub rho_zero: f64,
    /// 因子相关。
    pub rho_beta: f64,
    /// 模型隐含遗传相关 rg。
    pub rg: f64,
    /// Dice 相似系数 = 2·pi12 / (pi1 + pi2 + 2·pi12)。
    pub dice: f64,
    pub h2_t1: f64,
    pub h2_t2: f64,
    pub pi1: f64,
    pub pi2: f64,
    pub pi12: f64,
    pub pi1u: f64,
    pub pi2u: f64,
    pub nc12: f64,
    pub nc12_p9: f64,
    pub aic: f64,
    pub bic: f64,
}

impl BivariateFitResult {
    pub fn derive(data: &BivariateData, params: BivariateParams, loglike: f64) -> Self {
        let totalhet: f64 = data.h.iter().sum();
        let num_snps = data.n_snp() as f64;
        let sum_weights: f64 = data.weights.iter().sum();

        let pi1 = params.pi[0];
        let pi2 = params.pi[1];
        let pi12 = params.pi[2];
        let pi1u = pi1 + pi12;
        let pi2u = pi2 + pi12;

        let rg = params.rg();
        let dice = 2.0 * pi12 / (pi1 + pi2 + 2.0 * pi12);
        let h2_t1 = params.sig2_beta[0] * pi1u * totalhet;
        let h2_t2 = params.sig2_beta[1] * pi2u * totalhet;
        let nc12 = num_snps * pi12;
        let nc12_p9 = NC_COEF_P9 * nc12;
        let aic = 2.0 * COST_DF as f64 + 2.0 * loglike;
        let bic = sum_weights.ln() * COST_DF as f64 + 2.0 * loglike;

        Self {
            rho_beta: params.rho_beta,
            rho_zero: params.rho_zero,
            rg,
            dice,
            h2_t1,
            h2_t2,
            pi1,
            pi2,
            pi12,
            pi1u,
            pi2u,
            nc12,
            nc12_p9,
            aic,
            bic,
            params,
            loglike,
        }
    }
}
