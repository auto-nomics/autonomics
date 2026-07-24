/// Univariate MiXeR 的 3 个自由参数
///
/// 模型假设每个 SNP 要么是 causal（以概率 π），要么不是。
///   - causal SNP 的效应量 ~ N(0, sig2_beta)
///   - 非 causal SNP 的 z-score ~ N(0, sig2_zero)（不是标准正态，
///     因为人群分层、样本亲缘等会让方差 > 1）
///
/// 三个参数一起决定：有多少 causal 变异（polygenicity）、
/// 每个的效应多大（discoverability）、以及有多少截断膨胀（inflation）。
#[derive(Debug, Clone)]
pub struct UnivariateParams {
    /// causal SNP 比例（polygenicity） (0,1)
    pub pi: f64,
    /// 因果效应量方差 (discoverability) >0
    pub sig2_beta: f64,
    /// Null 分量方差膨胀因子（等价于 LDSC intercept），> 0
    pub sig2_zero: f64,
}

impl UnivariateParams {
    pub fn new(pi: f64, sig2_beta: f64, sig2_zero: f64) -> Self {
        Self {
            pi,
            sig2_beta,
            sig2_zero,
        }
    }
}
