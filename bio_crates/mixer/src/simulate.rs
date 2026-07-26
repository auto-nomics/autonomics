//! 前向模拟器：用已知参数 + LD 结构采样合成 GWAS z-score。
//!
//! cost function 的逆运算。生成模型（与 cost 假设完全一致）：
//!   1. 每个 snp s 以概率 π 成为 causal；causal 则 β_s ~ N(0, σ²_β)，否则 0
//!   2. tag j 的遗传效应 δ_j = Σ_s √(N_j·h_s·r²_{js})·β_s
//!   3. z_j = δ_j + ε，ε ~ N(0, sig2_zero)
//!
//! 用途：生成"已知答案"的数据，验证 fit1 能否回收参数。

use crate::data::ChromData;
use crate::params::UnivariateParams;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use rand_distr::{Distribution, Normal};

/// 用已知参数采样 z-score 向量（长度 = data.n_snp()）。
///
/// 基于Univariate MiXeR的spike-and-slab模型进行采样
pub fn simulate(data: &ChromData, params: &UnivariateParams, seed: u64) -> Vec<f64> {
    let mut rng = SmallRng::seed_from_u64(seed);
    let beta_dist = Normal::new(0.0, params.sig2_beta.sqrt()).unwrap();
    let noise_dist = Normal::new(0.0, params.sig2_zero.sqrt()).unwrap();
    let n_snp = data.n_snp();

    // 1. 逐 snp 采样效应（spike-and-slab）
    let beta: Vec<f64> = (0..n_snp)
        .map(|_| {
            if rng.gen_range(0.0..1.0) < params.pi {
                // Slab
                beta_dist.sample(&mut rng)
            } else {
                // Spike
                0.0
            }
        })
        .collect();

    // 2-3. 逐 tag 累加 LD 邻居贡献 + 噪声
    let mut z = vec![0.0; n_snp];

    for (j, _) in (0..n_snp).enumerate() {
        let mut delta = 0.0;
        let n_j = data.n[j];
        let (cols, r2s) = data.ld.row(j);
        for (k, s) in cols.iter().enumerate() {
            let a2ij = n_j * data.h[*s as usize] * r2s[k] as f64;
            delta += a2ij.sqrt() * beta[*s as usize];
        }
        z[j] = delta + noise_dist.sample(&mut rng);
    }
    z
}

#[cfg(test)]
mod tests {}
