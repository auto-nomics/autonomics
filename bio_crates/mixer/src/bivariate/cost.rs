//! Bivariate MiXeR 的 cost function（解析 Gaussian 近似）。
//!
//! clean-room 复刻原版 `bgmg_calculator_unified.cc::calc_unified_bivariate_cost_gaussian`。
//! 与 univariate 不同，bivariate 不用 2 分量矩匹配，而是每个 tag 直接拟合
//! 一个 2D 高斯，协方差由 β 的三个 2 阶矩 E[b1²]、E[b2²]、E[b1·b2] 经 LD 传播得到。
//!
//! 简化（与原版 `.cost()` 调用一致，r2min=0 下成立）：
//!   sig2_zeroC = [1,1]、sig2_zeroL = [0,0]、rho_zeroL = 0 → 无 below-r2min 项。

use std::f64::consts::PI;

use crate::bivariate::data::BivariateData;
use crate::bivariate::params::BivariateParams;

/// pdf 下溢兜底（原版 kMinTagPdf = 1e-100）。
const K_MIN_TAG_PDF: f64 = 1e-100;
/// f32 最小正次正规数，原版 `gaussian2_pdf` 末尾 `+ numeric_limits<float>::min()`。
const F32_MIN_POS: f64 = 1.4012984643248171e-45;

/// 零均值 2D 高斯密度，显式 2×2 求逆。
///
/// 协方差 `S = [[a11,a12],[a12,a22]]`，`|S| = a11·a22 − a12²`，
/// `S⁻¹ = (1/|S|)·[[a22,−a12],[−a12,a11]]`，故指数分子为
/// `a22·z1² − 2·a12·z1·z2 + a11·z2²`。
///
/// 返回 `(2π)⁻¹·|S|⁻¹ᐟ²·exp(−½ zᵀ S⁻¹ z) + F32_MIN_POS`（末尾加项防止 log(0)，
/// 与原版一致）。
fn gaussian2_pdf(z1: f64, z2: f64, a11: f64, a12: f64, a22: f64) -> f64 {
    let dt = a11 * a22 - a12 * a12;
    if !(dt > 0.0) {
        // 非正定协方差（参数退化）：交给调用方的下溢兜底。
        return 0.0;
    }
    let log_exp = -0.5 * (a22 * z1 * z1 + a11 * z2 * z2 - 2.0 * a12 * z1 * z2) / dt;
    let log_dt = -0.5 * dt.ln();
    let log_pi = -(2.0 * PI).ln();
    (log_pi + log_dt + log_exp).exp() + F32_MIN_POS
}

/// Bivariate 负对数似然（Gaussian 近似）。
///
/// 步骤（见模块文档与原版 l.1000-1132）：
/// 1. 全局先验矩（参数常数，循环外算一次）：
///    - Eb20 = (pi[0]+pi[2])·sig2_beta[0]        = E[β1²]
///    - Eb02 = (pi[1]+pi[2])·sig2_beta[1]        = E[β2²]
///    - Eb11 = pi[2]·rho_beta·√(sig2_beta[0]·sig2_beta[1])  = E[β1·β2]（仅共有贡献）
/// 2. 逐 tag 做 LD 传播，累加 Ed20/Ed02/Ed11。
/// 3. 组装 2×2 协方差并求 2D 高斯密度，加权累加 −log pdf。
pub fn bivariate_cost_gaussian(data: &BivariateData, p: &BivariateParams) -> f64 {
    // 1. 先验矩
    let eb20 = (p.pi[0] + p.pi[2]) * p.sig2_beta[0];
    let eb02 = (p.pi[1] + p.pi[2]) * p.sig2_beta[1];
    let eb11 = p.pi[2] * p.rho_beta * (p.sig2_beta[0] * p.sig2_beta[1]).sqrt();

    // null 协方差（sig2_zeroL=0、rho_zeroL=0）：
    let sz11 = p.sig2_zero[0];
    let sz22 = p.sig2_zero[1];
    let sz12 = p.rho_zero * (p.sig2_zero[0] * p.sig2_zero[1]).sqrt();

    let mut cost = 0.0;
    for &tag in &data.tags {
        let j = tag as usize;
        let n1j = data.n1[j];
        let n2j = data.n2[j];

        // 2. LD 传播：a1=n1·h·r²、a2=n2·h·r²（sig2_zeroC=1）
        let mut ed20 = 0.0;
        let mut ed02 = 0.0;
        let mut ed11 = 0.0;
        let (cols, r2s) = data.ld.row(j);
        for (k, &s) in cols.iter().enumerate() {
            let hi = data.h[s as usize];
            let a1 = n1j * hi * r2s[k];
            let a2 = n2j * hi * r2s[k];
            ed20 += a1 * eb20;
            ed02 += a2 * eb02;
            // Edelta11 用 √(a1·a2)·Eb11（原版的 √(a2ij1·a2ij2) 技巧）
            ed11 += (a1 * a2).sqrt() * eb11;
        }

        // 两 trait 都无 LD 信号 → 跳过（原版 l.1077）
        if ed20 == 0.0 && ed02 == 0.0 {
            continue;
        }

        // 3. 组装协方差 + 2D 高斯密度
        let a11 = ed20 + sz11;
        let a22 = ed02 + sz22;
        let a12 = ed11 + sz12;

        let mut pdf = gaussian2_pdf(data.z1[j], data.z2[j], a11, a12, a22);
        if !(pdf > 0.0) {
            pdf = K_MIN_TAG_PDF;
        }
        let mut inc = -pdf.ln() * data.weights[j];
        if !inc.is_finite() {
            inc = -K_MIN_TAG_PDF.ln() * data.weights[j];
        }
        cost += inc;
    }
    cost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian2_independent_standard() {
        // 独立标准正态：(0,0) 处密度 = (1/√(2π))² = 1/(2π) ≈ 0.15915
        let p = gaussian2_pdf(0.0, 0.0, 1.0, 0.0, 1.0);
        assert!((p - 1.0 / (2.0 * PI)).abs() < 1e-9);
    }

    #[test]
    fn gaussian2_correlated_peak() {
        // 强相关 a12→a11 时峰值升高（|S| 减小）
        let indep = gaussian2_pdf(0.0, 0.0, 1.0, 0.0, 1.0);
        let corr = gaussian2_pdf(0.0, 0.0, 1.0, 0.5, 1.0);
        assert!(corr > indep);
    }

    #[test]
    fn gaussian2_degenerate_returns_zero() {
        // 非正定（dt=0）→ 0
        assert_eq!(gaussian2_pdf(0.0, 0.0, 1.0, 1.0, 1.0), 0.0);
    }

    #[test]
    fn cost_no_ld_is_zero() {
        // 所有 tag 无 LD（Ed20=Ed02=0）→ 全跳过 → cost=0
        let data = BivariateData::new(
            vec![1.0, 2.0],
            vec![0.5, 1.0],
            vec![100.0, 100.0],
            vec![100.0, 100.0],
            vec![0.5, 0.4],
            &[],
        );
        let p = BivariateParams {
            pi: [0.001, 0.002, 0.0005],
            sig2_beta: [0.04, 0.05],
            sig2_zero: [1.0, 1.0],
            rho_beta: 0.5,
            rho_zero: 0.1,
        };
        assert_eq!(bivariate_cost_gaussian(&data, &p), 0.0);
    }

    #[test]
    fn cost_with_ld_finite_positive() {
        let triples = vec![(0, 1, 0.5)];
        let data = BivariateData::new(
            vec![1.0, 0.8],
            vec![0.5, 0.6],
            vec![100.0, 100.0],
            vec![100.0, 100.0],
            vec![0.5, 0.4],
            &triples,
        );
        let p = BivariateParams {
            pi: [0.001, 0.002, 0.0005],
            sig2_beta: [0.04, 0.05],
            sig2_zero: [1.0, 1.0],
            rho_beta: 0.5,
            rho_zero: 0.1,
        };
        let c = bivariate_cost_gaussian(&data, &p);
        assert!(c.is_finite());
        assert!(c > 0.0);
    }
}
