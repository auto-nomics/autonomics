//! 随机剪枝（random pruning）权重 —— clean-room 复刻原版
//! `bgmg_calculator.cc::set_weights_randprune`。
//!
//! 目标：为每个 tag 算一个权重，反映其在 LD 意义下的"独立代表数"（LD 冗余越大，
//! 权重越小），用于 cost 加权（`cost = Σ −log pdf · weight`）。
//!
//! 算法（n 轮求平均）：
//! 每轮用随机顺序贪心选取一个 LD 独立集（任意两 tag 的 r² < 阈值）。
//! `weight[tag] = (tag 在 n 轮中被选中的次数) / n`。
//! n=64 时是"有效独立 tag 数"的 Monte Carlo 估计。
//!
//! **bit-exact 设计**：原版用 `std::mt19937_64(seed+round)` + libstdc++
//! `uniform_int_distribution`。两者皆可逐位复刻：
//! - [`Mt19937_64`]：`rand_mt::Mt19937GenRand64`（与 `std::mt19937_64` 位兼容，
//!   实测前 3 个输出逐位一致）。
//! - [`uniform_int`]：libstdc++ GCC 的 Lemire 无偏法（`_S_nd`，u128）。
//!
//! 因此给定相同 LD 顺序与 deftag 集合，Rust 权重与原版逐 bit 一致。

use std::collections::BTreeSet;

use rand::RngCore;

use crate::ld_matrix::{LdBlock, LdRandomAccess};

/// MT19937-64，bit-compatible with `std::mt19937_64`。
/// 复用 `rand_mt` crate（`Mt19937GenRand64::new(seed)` 与 std 单值播种逐位一致）。
pub type Mt19937_64 = rand_mt::Mt19937GenRand64;

// =====================================================================
// libstdc++ uniform_int_distribution<int>(0, range-1) —— Lemire 无偏法
// =====================================================================

/// 返回 [0, range-1] 的均匀随机整数。bit-compatible with libstdc++ GCC
/// `uniform_int_distribution<int>(0, range-1)(mt19937_64)`（64-bit 走 `_S_nd` + u128）。
///
/// `range` = 候选数（≥1）。对应 libstdc++ 内部 `__uerange = urange+1 = range`。
fn uniform_int(rng: &mut Mt19937_64, range: u64) -> u64 {
    let mut x = rng.next_u64();
    let mut product = (x as u128) * (range as u128);
    let mut low = product as u64;
    if low < range {
        // __threshold = -range % range  （无符号）
        let threshold = (0u64.wrapping_sub(range)) % range;
        while low < threshold {
            x = rng.next_u64();
            product = (x as u128) * (range as u128);
            low = product as u64;
        }
    }
    (product >> 64) as u64
}

// =====================================================================
// 随机剪枝
// =====================================================================

/// 随机剪枝参数。
#[derive(Clone, Copy, Debug)]
pub struct RandpruneConfig {
    /// 轮数（原版 `--randprune-n`，默认 64）。
    pub n: u32,
    /// r² 阈值（原版 `--randprune-r2`，默认 0.1）。
    pub r2_threshold: f64,
    /// 是否用 1/w_ld 权重（原版 `use_w_ld`，默认 false）。
    pub use_w_ld: bool,
    /// 随机种子（原版 `--seed`，默认 123）。
    pub seed: u64,
}

impl Default for RandpruneConfig {
    fn default() -> Self {
        Self {
            n: 64,
            r2_threshold: 0.1,
            use_w_ld: false,
            seed: 123,
        }
    }
}

/// 计算随机剪枝权重。
///
/// - `ld`：稀疏 LD（任意 [`LdRandomAccess`]——单块 CSR 或块对角视图均可）
/// - `n_snp`：SNP 总数（unified index 空间）
/// - `tags`：tag 子集（snp index），即参与剪枝的 SNP
/// - `tag_defined`：每个 tag 是否有效（z/n 有限），长度 == tags.len()；None 视为全有效
///
/// 返回 per-tag 权重（长度 == tags.len()），与原版 `weights_[tag_index]` 逐位一致。
///
/// 泛型于 [`LdRandomAccess`]：调用方可直接传入逐染色体的块对角视图
///（[`crate::ld_matrix::BlockDiagonal`]），无需 `merge_blocks` 出全局 CSR——
/// 峰值内存从 ~2× nnz 降到 ~1× nnz，结果与吃 merged CSR 逐位相同。
pub fn randprune_weights<L: LdRandomAccess + ?Sized>(
    ld: &L,
    n_snp: usize,
    tags: &[u32],
    tag_defined: Option<&[bool]>,
    cfg: &RandpruneConfig,
) -> Vec<f64> {
    let num_tag = tags.len();
    // snp → tag position（-1 表示非 tag）
    let mut snp_to_tag = vec![-1i32; n_snp];
    for (i, &snp) in tags.iter().enumerate() {
        snp_to_tag[snp as usize] = i as i32;
    }
    let defined = tag_defined.unwrap_or(&[]);

    let mut weight = vec![0.0f64; num_tag];

    for prune_i in 0..cfg.n {
        let mut rng = Mt19937_64::new(cfg.seed + prune_i as u64);

        let mut passed = vec![false; num_tag]; // 本轮通过剪枝的 tag
        let mut processed = vec![false; num_tag];
        let mut non_processed: BTreeSet<i32> = (0..num_tag as i32).collect();
        // 把未定义的 tag 标为 processed 并移出 non_processed
        for tag_i in 0..num_tag {
            let is_def = defined.is_empty() || defined[tag_i];
            if !is_def {
                processed[tag_i] = true;
                non_processed.remove(&(tag_i as i32));
            }
        }

        let mut candidate: Vec<i32> = (0..num_tag as i32).collect();

        while !candidate.is_empty() {
            let r = uniform_int(&mut rng, candidate.len() as u64) as usize;
            let random_tag = candidate[r] as usize;
            if processed[random_tag] {
                // 碰撞：用当前 non_processed（升序）重建 candidate
                candidate = non_processed.iter().copied().collect();
                continue;
            }
            passed[random_tag] = true;

            // 标记与 random_tag 有 LD（r²≥阈值）且是 tag、未处理的邻居
            let snp = tags[random_tag] as usize;
            let (cols, r2s) = ld.row(snp);
            let mut num_changes = 0i32;
            for (k, &nb_snp) in cols.iter().enumerate() {
                let nb_tag = snp_to_tag[nb_snp as usize];
                if nb_tag < 0 {
                    continue;
                }
                if r2s[k] < cfg.r2_threshold {
                    continue;
                }
                let nb = nb_tag as usize;
                if processed[nb] {
                    continue;
                }
                processed[nb] = true;
                non_processed.remove(&nb_tag);
                num_changes += 1;
            }
            if num_changes == 0 {
                break; // 卡住，取消本轮
            }
        }

        // 累加本轮权重
        for tag_i in 0..num_tag {
            if !passed[tag_i] {
                continue;
            }
            if cfg.use_w_ld {
                let snp = tags[tag_i] as usize;
                let (cols, r2s) = ld.row(snp);
                let mut w_ld = 0.0f64;
                for (k, &nb_snp) in cols.iter().enumerate() {
                    let nb_tag = snp_to_tag[nb_snp as usize];
                    if nb_tag < 0 {
                        continue;
                    }
                    if !passed[nb_tag as usize] {
                        continue;
                    }
                    w_ld += r2s[k];
                }
                weight[tag_i] += 1.0 / w_ld;
            } else {
                weight[tag_i] += 1.0;
            }
        }
    }

    let nf = cfg.n as f64;
    for w in weight.iter_mut() {
        *w /= nf;
    }
    weight
}

// =====================================================================
// 逆 LD-score 加权（单趟流式友好，无需随机访问）
// =====================================================================

/// 从一个 tag 的 Σ r² 算逆 LD-score 权重：`1 / (1 + Σ r²)`。
///
/// "+1" 容纳变异自身，避免无 LD 邻居的 tag 除零/权重过大。这是 LDSC 风格的加权：
/// LD 冗余越高（邻居多、r² 大）权重越小，与 randprune"独立代表数"同向但更便宜。
///
/// 抽成独立函数，让节点的**流式 COO 扫描**（边读边累加 Σ r²）和基于
/// [`LdRandomAccess`] 的 [`ldscore_weights`] 共用同一个变换。
pub fn ldscore_weight(sum_r2: f64) -> f64 {
    1.0 / (1.0 + sum_r2)
}

/// 对每个 SNP 算逆 LD-score 权重（`1 / (1 + 该 SNP 邻居的 Σ r²)`）。
///
/// 与 [`randprune_weights`] 不同的加权方案：单趟扫每行求 Σ r² 即可，**不需要**
/// 多轮随机访问，因此可由节点在流式读取 LD 时顺手累加、根本不建 CSR。
///
/// 返回 per-SNP 权重（长度 == `n_snp`）。
pub fn ldscore_weights<L: LdRandomAccess + ?Sized>(ld: &L, n_snp: usize) -> Vec<f64> {
    let mut w = vec![0.0; n_snp];
    for (j, weight) in w.iter_mut().enumerate() {
        let (_, r2s) = ld.row(j);
        let sum_r2: f64 = r2s.iter().copied().sum();
        *weight = ldscore_weight(sum_r2);
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mt19937_64_matches_cpp_std() {
        // bit-exact：rand_mt 与 std::mt19937_64 前 3 个输出（g++ 实测）逐位一致
        let mut rng = Mt19937_64::new(42);
        assert_eq!(rng.next_u64(), 13930160852258120406);
        assert_eq!(rng.next_u64(), 11788048577503494824);
        assert_eq!(rng.next_u64(), 13874630024467741450);
        let mut r2 = Mt19937_64::new(42);
        assert_eq!(r2.next_u64(), 13930160852258120406);
    }

    #[test]
    fn uniform_int_in_range() {
        let mut rng = Mt19937_64::new(1);
        for _ in 0..1000 {
            let r = uniform_int(&mut rng, 100);
            assert!(r < 100);
        }
    }

    #[test]
    fn randprune_basic() {
        // 3 个 tag（snp 0,1,2），0 与 1 有 LD(r²=0.5)，0 与 2 无 LD
        let triples = vec![(0, 1, 0.5)];
        let ld = LdBlock::from_coo(&triples, 3);
        let tags = vec![0u32, 1, 2];
        let w = randprune_weights(&ld, 3, &tags, None, &RandpruneConfig::default());
        assert_eq!(w.len(), 3);
        // 权重应为正有限
        for wi in &w {
            assert!(*wi >= 0.0 && wi.is_finite());
        }
        // 总权重大致反映"有效独立 tag 数"≈ 2（0/1 互斥 + 2 独立），允许统计波动
        let sum: f64 = w.iter().sum();
        assert!(sum > 1.0 && sum < 3.0, "sum={sum}");
    }

    #[test]
    fn randprune_deterministic() {
        let triples = vec![(0u32, 1u32, 0.5), (0, 2, 0.3), (1, 2, 0.4)];
        let ld = LdBlock::from_coo(&triples, 3);
        let tags = vec![0u32, 1, 2];
        let w1 = randprune_weights(&ld, 3, &tags, None, &RandpruneConfig::default());
        let w2 = randprune_weights(&ld, 3, &tags, None, &RandpruneConfig::default());
        assert_eq!(w1, w2, "同种子应逐位一致");
    }

    #[test]
    fn ldscore_weights_basic() {
        // tag0 有两邻居 Σr²=0.5+0.3=0.8 → 1/1.8；tag1 一邻居 0.4 → 1/1.4；tag2 无邻居 → 1/1
        let triples = vec![(0u32, 1u32, 0.5), (0, 2, 0.3), (1, 2, 0.4)];
        let ld = LdBlock::from_coo(&triples, 3);
        let w = ldscore_weights(&ld, 3);
        assert!((w[0] - 1.0 / 1.8).abs() < 1e-12);
        assert!((w[1] - 1.0 / 1.4).abs() < 1e-12);
        assert!((w[2] - 1.0).abs() < 1e-12);
        // 全正、有限
        for wi in &w {
            assert!(*wi > 0.0 && wi.is_finite());
        }
    }

    #[test]
    fn randprune_on_block_diagonal_matches_merged() {
        // Tier 1 核心断言：randprune 直接吃 BlockDiagonal 与吃 merged CSR 逐位一致。
        use crate::ld_matrix::{BlockDiagonal, LdRandomAccess};
        let chr_a = vec![(0u32, 1u32, 0.8), (0, 2, 0.3), (1, 2, 0.6)];
        let chr_b = vec![(0u32, 1u32, 0.5), (1, 2, 0.9)];
        let block_a = LdBlock::from_coo(&chr_a, 3);
        let block_b = LdBlock::from_coo(&chr_b, 3);
        let merged = LdBlock::merge_blocks(&[(0, block_a.clone()), (3, block_b.clone())], 6);
        let view = BlockDiagonal::new(vec![(0, block_a), (3, block_b)]);

        let tags: Vec<u32> = (0..6).collect();
        let cfg = RandpruneConfig::default();
        let w_merged = randprune_weights(&merged, 6, &tags, None, &cfg);
        let w_view = randprune_weights(&view, 6, &tags, None, &cfg);
        assert_eq!(
            w_merged, w_view,
            "BlockDiagonal 与 merged CSR 权重应逐位一致"
        );
    }
}
