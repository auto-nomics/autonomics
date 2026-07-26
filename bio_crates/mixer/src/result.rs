//! fit1 的输出结果。

use crate::data::ChromData;
use crate::params::UnivariateParams;

/// 捕获 90% 遗传力所需的 causal 变异比例（原版 NCKoef，针对单高斯+MAF 模型标定）。
const NC_COEF_P9: f64 = 0.319;

/// univariate MiXeR 的自由参数数（pi, sig2_beta, sig2_zero），用于 AIC/BIC。
const COST_DF: i64 = 3;

#[derive(Debug, Clone)]
pub struct FitResult {
    /// 拟合得到的最优参数
    pub params: UnivariateParams,
    /// 最优参数处的负对数似然（cost = −logL）
    pub loglike: f64,
    /// SNP 遗传力 h² = σ²_β · π · Σ_s h_s
    pub h2: f64,
    /// causal 变异总数 nc = π · num_snps
    pub nc: f64,
    /// 捕获 90% h² 的 causal 变异数 nc@p9 = π · num_snps · 0.319
    /// 这里的0.319是Univariate MiXeR原C++实现内标定的常数，意思是：效果最大的那31.9%
    /// 的因果变异，就足以解释90%的遗传力
    pub nc_p9: f64,
    /// AIC = 2·cost_df + 2·cost（值越小越好；正值表示数据支持 MiXeR 而非更简单模型）
    pub aic: f64,
    /// BIC = log(Σweights)·cost_df + 2·cost
    pub bic: f64,
}

impl FitResult {
    /// 从拟合参数 + 数据 + loglike 派生完整结果。
    pub fn derive(data: &ChromData, params: UnivariateParams, loglike: f64) -> Self {
        // totalhet = Σ_s h_s（杂合度之和）
        let totalhet: f64 = data.h.iter().sum();
        let num_snps = data.n_snp() as f64;
        let sum_weights: f64 = data.weights.iter().sum();

        let h2 = params.sig2_beta * params.pi * totalhet;
        let nc = params.pi * num_snps;
        let nc_p9 = nc * NC_COEF_P9;
        let aic = 2.0 * COST_DF as f64 + 2.0 * loglike;
        let bic = sum_weights.ln() * COST_DF as f64 + 2.0 * loglike;

        Self {
            params,
            loglike,
            h2,
            nc,
            nc_p9,
            aic,
            bic,
        }
    }
}
