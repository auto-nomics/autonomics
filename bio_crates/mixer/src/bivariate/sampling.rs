//! Bivariate sampling cost —— Monte Carlo 估计，打破高斯 cost 的 pi12/rho_beta 退化。
//!
//! clean-room 复刻原版 `bgmg_calculator_unified.cc::calc_unified_bivariate_cost_sampling`
//! + `find_unified_bivariate_tag_delta_sampling`。
//!
//! 思路：对每个 tag，按 (pi1, pi2, pi12) 对其 LD 邻居做**多项采样**，生成 `k_max` 个
//! 因果配置。每个配置 k 给出一组 (δ1², δ2², δ1·δ2) 的实现值，组装成一个 2D 高斯协差，
//! 算 pdf。最终 `pdf_tag = (1/k_max)·Σ_k pdf_k`，即真实混合似然的 Monte Carlo 估计。
//! 平均保留了高阶矩信息（高斯近似丢弃的部分），因此能识别 pi12/rho_beta。
//!
//! 与 C++ 的关系：**统计一致，非逐 bit**。C++ 用 xorshift128+ + libstdc++ binomial +
//! SIMD 部分洗牌；这里用 `SmallRng` + `rand_distr::Binomial` + Fisher-Yates 部分洗牌。
//! k_max=20000、~11000 tag 下 MC 方差极小，两条独立估计的 cost 曲线及其最小值高度收敛。
//! 关键行为（逐分量 max(0,·) 钳制、sig2_zeroC=1、sig2_zeroL=0、per-tag 播种）均逐字镜像。

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use rand_distr::Distribution;
use rayon::prelude::*;
use std::f64::consts::PI;

use crate::bivariate::data::BivariateData;
use crate::bivariate::params::BivariateParams;

const K_MIN_TAG_PDF: f64 = 1e-100;
const F32_MIN_POS: f64 = 1.401_298_464_324_817e-45;
const NUM_COMPONENTS: usize = 3;

/// 零均值 2D 高斯密度（与 `cost::gaussian2_pdf` 同实现；此处复制以保持采样热路径独立）。
fn gaussian2_pdf(z1: f64, z2: f64, a11: f64, a12: f64, a22: f64) -> f64 {
    let dt = a11 * a22 - a12 * a12;
    if dt <= 0.0 || dt.is_nan() {
        return 0.0;
    }
    let log_exp = -0.5 * (a22 * z1 * z1 + a11 * z2 * z2 - 2.0 * a12 * z1 * z2) / dt;
    let log_dt = -0.5 * dt.ln();
    let log_pi = -(2.0 * PI).ln();
    (log_pi + log_dt + log_exp).exp() + F32_MIN_POS
}

/// per-tag 播种（镜像 C++ key1=seed, key2=1+snp_idx 的确定性结构）。
fn tag_seed(seed: u64, snp_index: u32) -> u64 {
    let k1 = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let k2 = (1 + snp_index as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    k1 ^ k2
}

struct SamplingBuffers<'a> {
    delta20: &'a mut [f64],
    delta02: &'a mut [f64],
    delta11: &'a mut [f64],
    slots: &'a mut [u32],
}

/// 单个 tag 的采样 pdf（实现 `find_unified_bivariate_tag_delta_sampling` + 内层 pdf 循环）。
///
/// 返回该 tag 的混合 pdf（未取对数）。`buf_*` 是长度 `k_max` 的工作缓冲（复用，避免反复分配）。
fn tag_pdf_sampling(
    data: &BivariateData,
    p: &BivariateParams,
    tag_j: usize,
    k_max: usize,
    seed: u64,
    buffers: SamplingBuffers<'_>,
) -> f64 {
    let SamplingBuffers {
        delta20,
        delta02,
        delta11,
        slots,
    } = buffers;
    // 先验矩分量（常数，跨 snp）
    let sb1 = p.sig2_beta[0];
    let sb2 = p.sig2_beta[1];
    let rho = p.rho_beta;
    let pi = [p.pi[0], p.pi[1], p.pi[2]];

    let n1j = data.n1[tag_j];
    let n2j = data.n2[tag_j];

    // 重置工作缓冲
    for x in delta20.iter_mut() {
        *x = 0.0;
    }
    for x in delta02.iter_mut() {
        *x = 0.0;
    }
    for x in delta11.iter_mut() {
        *x = 0.0;
    }
    // slots = [0..k_max)
    for (i, s) in slots.iter_mut().enumerate() {
        *s = i as u32;
    }

    let mut rng = SmallRng::seed_from_u64(tag_seed(seed, tag_j as u32));

    // 累积 pi>0.5 时的"全 causal"基线（GWAS 数据 pi<<0.5，恒为 0，但保留以镜像 C++）
    let mut d20_inf = 0.0f64;
    let mut d02_inf = 0.0f64;
    let mut d11_inf = 0.0f64;

    let (cols, r2s) = data.ld.row(tag_j);
    for (k_nb, &s) in cols.iter().enumerate() {
        let hi = data.h[s as usize];
        let r2 = r2s[k_nb];
        let a1 = r2 * hi * n1j; // sig2_zeroC = 1
        let a2 = r2 * hi * n2j;

        // 三成分的 delta 值
        // comp0(trait1-only): d20=a1*sb1, d02=0,        d11=0
        // comp1(trait2-only): d20=0,       d02=a2*sb2,  d11=0
        // comp2(shared):      d20=a1*sb1,  d02=a2*sb2,  d11=rho*sqrt(a1*sb1*a2*sb2)
        let d20_c2 = a1 * sb1;
        let d02_c2 = a2 * sb2;
        let d11_c2 = rho * (d20_c2 * d02_c2).sqrt();
        let d20_val = [d20_c2, 0.0, d20_c2];
        let d02_val = [0.0, d02_c2, d02_c2];
        let d11_val = [0.0, 0.0, d11_c2];

        // 多项采样：对 k_max 个 slot 依次按 (pi1,pi2,pi12) 条件二项分配
        // counts[c] = 本次邻居分到成分 c 的 slot 数；total = Σ counts ≤ k_max
        let mut counts = [0u32; NUM_COMPONENTS];
        let mut total = 0u32;
        let mut pp = pi;
        for c in 0..NUM_COMPONENTS {
            let n_remain = k_max as u32 - total;
            let pc = pp[c].clamp(0.0, 1.0);
            let draw = if n_remain == 0 || pc <= 0.0 {
                0
            } else if pc >= 1.0 {
                n_remain
            } else {
                rand_distr::Binomial::new(u64::from(n_remain), pc)
                    .unwrap()
                    .sample(&mut rng) as u32
            };
            counts[c] = draw;
            total += draw;
            // 条件重归一化：剩余概率乘以 1/(1-p_c)
            if pc < 1.0 {
                let factor = 1.0 / (1.0 - pc);
                for value in pp.iter_mut().take(NUM_COMPONENTS).skip(c + 1) {
                    *value *= factor;
                }
            }
        }

        // 把 total 个随机 slot 选到 slots[0..total]（Fisher-Yates 部分洗牌）
        // 统计等价于 C++ 的 xorshift128plus_shuffle32_partial（选 total 个随机子集到一端）
        if total > 0 {
            for i in 0..total as usize {
                let j = rng.gen_range(i..k_max);
                slots.swap(i, j);
            }
        }

        // pi>0.5 优化分支：抽样少量"非 causal"并扣除（GWAS 下不触发，但保留以镜像）
        let mut idx = 0usize;
        for c in 0..NUM_COMPONENTS {
            let mut d20 = d20_val[c];
            let mut d02 = d02_val[c];
            let mut d11 = d11_val[c];
            let mut pi_val = pi[c];
            if pi_val > 0.5 {
                pi_val = 1.0 - pi_val;
                d20_inf += d20;
                d02_inf += d02;
                d11_inf += d11;
                d20 *= -1.0;
                d02 *= -1.0;
                d11 *= -1.0;
            }
            let _ = pi_val;
            for _ in 0..counts[c] {
                let slot = slots[idx] as usize;
                delta20[slot] += d20;
                delta02[slot] += d02;
                delta11[slot] += d11;
                idx += 1;
            }
        }
        debug_assert_eq!(idx, total as usize);
    }

    // null 协差（sig2_zeroL=0）
    let sz11 = p.sig2_zero[0];
    let sz22 = p.sig2_zero[1];
    let sz12 = p.rho_zero * (p.sig2_zero[0] * p.sig2_zero[1]).sqrt();

    let z1 = data.z1[tag_j];
    let z2 = data.z2[tag_j];
    let pi_k = 1.0 / k_max as f64;
    let mut pdf_tag = 0.0f64;
    for k in 0..k_max {
        // 逐分量 max(0, delta+delta_inf) 钳制（镜像 C++ l.710-712，含 delta11）
        let a11 = (delta20[k] + d20_inf).max(0.0) + sz11;
        let a22 = (delta02[k] + d02_inf).max(0.0) + sz22;
        let a12 = (delta11[k] + d11_inf).max(0.0) + sz12;
        pdf_tag += gaussian2_pdf(z1, z2, a11, a12, a22) * pi_k;
    }
    pdf_tag
}

/// Bivariate sampling 负对数似然（Monte Carlo）。
///
/// 对齐 `calc_unified_bivariate_cost_sampling`：rayon 并行遍历 deftag，各 tag 独立采样，
/// 末尾 `cost = Σ_tag −log(pdf_tag)·weight`。`seed` 对应原版 `seed_`（默认 123）。
pub fn bivariate_cost_sampling(
    data: &BivariateData,
    p: &BivariateParams,
    k_max: usize,
    seed: u64,
) -> f64 {
    let tags: Vec<u32> = data
        .tags
        .iter()
        .copied()
        .filter(|&t| data.weights[t as usize] > 0.0)
        .collect();

    // 并行：每个 tag 一组工作缓冲（thread-local 复用，避免反复分配）
    total_pdf_worker(&tags, data, p, k_max, seed)
}

fn total_pdf_worker(
    tags: &[u32],
    data: &BivariateData,
    p: &BivariateParams,
    k_max: usize,
    seed: u64,
) -> f64 {
    // 线程局部缓冲：用 thread_local 复用 k_max 长度的 buffer
    thread_local! {
        static BUF20: std::cell::RefCell<Vec<f64>> = const { std::cell::RefCell::new(Vec::new()) };
        static BUF02: std::cell::RefCell<Vec<f64>> = const { std::cell::RefCell::new(Vec::new()) };
        static BUF11: std::cell::RefCell<Vec<f64>> = const { std::cell::RefCell::new(Vec::new()) };
        static SLOTS: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    tags.par_iter()
        .map(|&tag| {
            let j = tag as usize;
            let w = data.weights[j];
            BUF20.with(|b| {
                BUF02.with(|b2| {
                    BUF11.with(|b11| {
                        SLOTS.with(|sl| {
                            let mut bm = b.borrow_mut();
                            let mut bn = b2.borrow_mut();
                            let mut be = b11.borrow_mut();
                            let mut bs = sl.borrow_mut();
                            bm.resize(k_max, 0.0);
                            bn.resize(k_max, 0.0);
                            be.resize(k_max, 0.0);
                            bs.resize(k_max, 0);
                            let pdf = tag_pdf_sampling(
                                data,
                                p,
                                j,
                                k_max,
                                seed,
                                SamplingBuffers {
                                    delta20: &mut bm,
                                    delta02: &mut bn,
                                    delta11: &mut be,
                                    slots: &mut bs,
                                },
                            );
                            let pdf = if pdf > 0.0 { pdf } else { K_MIN_TAG_PDF };
                            let mut inc = -pdf.ln() * w;
                            if !inc.is_finite() {
                                inc = -K_MIN_TAG_PDF.ln() * w;
                            }
                            inc
                        })
                    })
                })
            })
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_data() -> BivariateData {
        // 4 SNP，tag0 与 1,2,3 有 LD
        let triples = vec![(0, 1, 0.5), (0, 2, 0.4), (0, 3, 0.3)];
        let mut d = BivariateData::new(
            vec![1.0, 0.5, 0.3, 0.2],
            vec![0.8, 0.4, 0.2, 0.1],
            vec![1000.0; 4],
            vec![1000.0; 4],
            vec![0.4, 0.4, 0.4, 0.4],
            &triples,
        );
        d.weights = vec![1.0, 0.0, 0.0, 0.0]; // 只有 tag0 是 deftag
        d.tags = vec![0];
        d
    }

    #[test]
    fn sampling_cost_finite_positive() {
        let d = demo_data();
        let p = BivariateParams {
            pi: [0.0005, 0.0001, 0.0007],
            sig2_beta: [0.04, 0.08],
            sig2_zero: [1.0, 1.05],
            rho_beta: 0.3,
            rho_zero: 0.1,
        };
        let c = bivariate_cost_sampling(&d, &p, 2000, 123);
        assert!(c.is_finite(), "cost={c}");
        assert!(c > 0.0);
    }

    #[test]
    fn sampling_deterministic_with_seed() {
        // 同种子两次采样 cost 相同（可复现）
        let d = demo_data();
        let p = BivariateParams {
            pi: [0.0005, 0.0001, 0.0007],
            sig2_beta: [0.04, 0.08],
            sig2_zero: [1.0, 1.05],
            rho_beta: 0.3,
            rho_zero: 0.1,
        };
        let c1 = bivariate_cost_sampling(&d, &p, 2000, 123);
        let c2 = bivariate_cost_sampling(&d, &p, 2000, 123);
        assert!((c1 - c2).abs() < 1e-9, "{c1} vs {c2}");
    }

    #[test]
    fn sampling_converges_to_gaussian_at_small_pi() {
        // pi 极小时，每个配置几乎无 causal → delta≈0 → pdf_tag ≈ gaussian2_pdf(只含 null)
        // 即 sampling cost ≈ 把每个 tag 当纯 null 高斯算的 cost。
        let d = demo_data();
        let p = BivariateParams {
            pi: [1e-12, 1e-12, 1e-12],
            sig2_beta: [0.04, 0.08],
            sig2_zero: [1.0, 1.05],
            rho_beta: 0.3,
            rho_zero: 0.1,
        };
        let c = bivariate_cost_sampling(&d, &p, 4000, 123);
        // null 协差下的单高斯 cost：tag0 的 -log gaussian2_pdf(z1,z2, 1.0, sz12, 1.05)
        let sz12: f64 = 0.1 * (1.0_f64 * 1.05).sqrt();
        let pdf = gaussian2_pdf(1.0, 0.8, 1.0, sz12, 1.05);
        let expected = -pdf.ln();
        assert!(
            (c - expected).abs() / expected.abs() < 0.05,
            "c={c} expected={expected}"
        );
    }
}
