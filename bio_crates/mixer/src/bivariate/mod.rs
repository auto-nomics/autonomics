//! Bivariate MiXeR（双变量混合模型）。
//!
//! 联合两个 GWAS trait 估计共享/特异 causal 变异与遗传相关。每个 tag 拟合
//! 单个 2D 高斯，协方差由 β 的 2 阶矩经 LD 传播得到（区别于 univariate 的
//! 2 分量矩匹配）。fit2 在固定 univariate 约束下优化 3 个自由参数
//! （rho_beta、rho_zero、pi12），fit_sequence 与原版对齐：
//! `diffevo-fast → neldermead-fast → brute1-fast → brent1-fast`。
//!
//! 详见各子模块文档与 `MIGRATION_PLAN.md`。

pub mod cost;
pub mod data;
pub mod fit;
pub mod optimizers;
pub mod parametrize;
pub mod params;
pub mod result;
pub mod sampling;
pub mod transforms;

pub use cost::bivariate_cost_gaussian;
pub use data::BivariateData;
pub use fit::{Fit2Config, fit2};
pub use params::{BivariateParams, UnivariateConstraint};
pub use result::BivariateFitResult;
pub use sampling::bivariate_cost_sampling;
