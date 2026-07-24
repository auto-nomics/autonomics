//! Univariate MiXeR 的 cost function（解析 Gaussian 近似）。
//!
//! 给定参数 (π, σ²_β, σ²_zero) 和 GWAS 数据，返回负对数似然。
//! 实现 bgmg_calculator_unified.cc::calc_unified_univariate_cost_gaussian。

use std::f64::consts::PI;

use crate::{data::ChromData, params::UnivariateParams};

/// 防止 pdf 下溢成0导致log(−∞) 的最小值
const K_MIN_PDF: f64 = 1e-300;

/// 均值为 0、标准差为 std 的正态分布，在 x 处的密度值。
///
/// φ(x; 0, σ) = (1 / (σ·√(2π))) · exp(−x² / (2σ²))
///
/// `x` 是曲线上的坐标，调用时传入 GWAS z-score 的数值（z=β/SE）。
/// 注意指数里必须有 σ²（之前 bug 就是漏了它，导致 std 越小密度反而越大）。
fn gaussian_pdf(x: f64, std: f64) -> f64 {
    if std <= 0.0 {
        // 退化：零宽度分布。x≠0 处密度为 0，交给后续 pdf.max(K_MIN_PDF) 兜底。
        return 0.0;
    }
    let coeff = 1.0 / (std * (2.0 * PI).sqrt());
    let exponent = -x * x / (2.0 * std * std);
    coeff * exponent.exp()
}

/// spike-and-slab 先验的 2 阶矩 E[β²] = π·σ²_β
fn prior_ebeta2(p: &UnivariateParams) -> f64 {
    p.pi * p.sig2_beta
}
/// spike-and-slab 先验的 4 阶累积量 κ₄(β) = 3π(1−π)·(σ²_β)²
///
/// 推导: E[β⁴]=3π(σ²_β)², E[β²]²=(πσ²_β)²
///        κ₄ = E[β⁴] − 3·E[β²]² = 3π(σ²_β)² − 3π²(σ²_β)² = 3π(1−π)(σ²_β)²
fn prior_ebeta4(p: &UnivariateParams) -> f64 {
    3.0 * p.pi * (1.0 - p.pi) * p.sig2_beta.powi(2)
}

/// 计算 tag j 的遗传效应矩 (A, B)。
///
/// A = Σ_s a2·ebeta2,  B = Σ_s a2²·ebeta4,  其中 a2 = N_j·h_s·r²
///
/// - data: 单条染色体的cost计算所需数据
/// - j: 当前正在计算的 tag SNP
///
/// 返回值：
/// - a：遗传效应 δ_j 这个随机变量的 2 阶矩
pub fn tag_moments(data: &ChromData, j: usize, ebeta2: f64, ebeta4: f64) -> (f64, f64) {
    let n_j = data.n[j]; // 当前所计算的tag SNP的per-SNP样本量
    // 累加器。A = Σ_s ...、B = Σ_s ... 是求和，从 0 开始往里加
    let mut a = 0.0;
    let mut b = 0.0;
    let (col_idxs, r2s) = data.ld.row(j);
    for (k, s) in col_idxs.iter().enumerate() {
        // SNP向量已经被标准化了，SNP的标号已经变成了从0开始的连续的向量
        // data.h[*s as usize] 获取当前行第s个SNP的遗传异质性
        //
        // 意义：计算当前行内（当前索引的连锁SNP邻居）对z值方差的贡献, 本质上是邻居贡献的传输系数
        let a2ij = n_j * data.h[*s as usize] * r2s[k];
        a += a2ij * ebeta2;
        b += a2ij * a2ij * ebeta4;
    }
    (a, b)
}

/// Univariate MiXeR 负对数似然（Gaussian 近似）。
///
/// 实现 bgmg_calculator_unified.cc::calc_unified_univariate_cost_gaussian。
/// 首版简化：不处理 censoring（|z|>zmax），固定 sig2_zeroL=0。
pub fn univariate_cost_gaussian(data: &ChromData, p: &UnivariateParams) -> f64 {
    // 1. 先验矩（univariate 下处处常数，循环外算一次）
    let ebeta2 = prior_ebeta2(p);
    let ebeta4 = prior_ebeta4(p);

    // 2. null 分量标准差（sig2_zeroL=0，所以 sig2_zero 就是参数本身）
    let s1 = p.sig2_zero.sqrt();

    let mut cost = 0.0;

    // 3. 逐个 tag 计算
    for j in 0..data.n_snp() {
        // 3a. LD 传播 -> (A, B) 得到二阶矩和四阶矩
        let (a, b) = tag_moments(data, j, ebeta2, ebeta4);

        // 3b. 无 LD 信号的 tag 跳过（A=0 时 sig2_tag 公式会除零）
        if a == 0.0 {
            continue;
        }

        // 3c. 2 分量混合参数（用 A, B 矩匹配出两个分量）
        let tag_pi0 = b / (b + 3.0 * a * a);
        let tag_pi1 = 1.0 - tag_pi0;
        let sig2_tag = (b + 3.0 * a * a) / (3.0 * a);
        let s2 = (p.sig2_zero + sig2_tag).sqrt();

        // 3d. 混合密度：null 分量 + signal 分量，按各自权重加权
        let pdf0 = gaussian_pdf(data.z[j], s1);
        let pdf1 = gaussian_pdf(data.z[j], s2);
        let pdf = tag_pi0 * pdf0 + tag_pi1 * pdf1;

        // 3e. 负对数似然累加。钳制下界防 pdf 下溢导致 log(0)=−∞
        let pdf = pdf.max(K_MIN_PDF);
        cost += -pdf.ln() * data.weights[j];
    }

    cost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_standard_normal() {
        // 标准正态 (std=1) 在 0 处密度 = 1/√(2π) ≈ 0.3989
        let p = gaussian_pdf(0.0, 1.0);
        assert!((p - 0.3989422804014327).abs() < 1e-12);
    }

    #[test]
    fn pdf_symmetric() {
        // 正负对称
        assert!((gaussian_pdf(1.5, 1.0) - gaussian_pdf(-1.5, 1.0)).abs() < 1e-12);
    }

    #[test]
    fn pdf_extreme_z_not_nan() {
        // 极端 z 不应产生 NaN（可能下溢成 0，但要有限）
        let p = gaussian_pdf(1000.0, 1.0);
        assert!(p.is_finite());
    }

    #[test]
    fn cost_no_ld_is_zero() {
        // 所有 tag 无 LD (A=0) → 全跳过 → cost=0
        use crate::data::ChromData;
        use crate::params::UnivariateParams;
        let data = ChromData::new(vec![1.0, 2.0], vec![100.0, 100.0], vec![0.5, 0.5], &[]);
        let p = UnivariateParams::new(0.1, 0.01, 1.0);
        let cost = univariate_cost_gaussian(&data, &p);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn cost_finite_and_positive() {
        // 有 LD 时 cost 应为有限正数
        use crate::data::ChromData;
        use crate::params::UnivariateParams;
        let triples = vec![(0, 1, 0.5)];
        let data = ChromData::new(vec![1.0, 0.5], vec![100.0, 100.0], vec![0.5, 0.4], &triples);
        let p = UnivariateParams::new(0.1, 0.01, 1.0);
        let cost = univariate_cost_gaussian(&data, &p);
        assert!(cost.is_finite());
        assert!(cost > 0.0);
    }
}
