//! fit1 的输出结果。

use crate::params::UnivariateParams;

#[derive(Debug, Clone)]
pub struct FitResult {
    /// 拟合得到的最优参数
    pub params: UnivariateParams,
    /// 最优参数处的负对数似然（cost）
    pub loglike: f64,
}
