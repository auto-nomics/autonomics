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

use crate::ld_matrix::LdBlock;

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
/// - `ld`：稀疏 LD（CSR，按 snp index 索引）
/// - `n_snp`：SNP 总数（unified index 空间）
/// - `tags`：tag 子集（snp index），即参与剪枝的 SNP
/// - `tag_defined`：每个 tag 是否有效（z/n 有限），长度 == tags.len()；None 视为全有效
///
/// 返回 per-tag 权重（长度 == tags.len()），与原版 `weights_[tag_index]` 逐位一致。
pub fn randprune_weights(
    ld: &LdBlock,
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
}
