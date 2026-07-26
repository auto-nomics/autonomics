//! extract —— tag SNP 子集选择，clean-room 复刻原版 `mixer.py snps`。
//!
//! 选出一组近条件独立的 tag SNP（~2M），供 fit 时只在子集上拟合。
//! 算法（对照 `bivar_mixer/cli.py:399-431` + `bgmg_calculator.cc:430-477` `perform_ld_clump`）：
//! 1. MAF 过滤：丢弃 `min(af,1-af) < maf_min` 的 SNP。
//! 2. 随机 subset：从候选里随机抽 `subset` 个（seeded，可复现）。
//! 3. 贪心 LD 剪枝：按随机优先级降序遍历，每个保留的代表把 `r² > r2_threshold` 的
//!    未处理邻居剪掉（确定性，给定优先级）——`perform_ld_clump` 的 1:1 翻译。
//!
//! **不需要 bit-exact**：原版 step2 用未 seed 的 Python `random.sample`，本就不可复现；
//! extract 是预处理（选哪些 SNP 当 tag），两套不同 extract 给出的 fit 都合法。
//! 这里用 seeded `SmallRng`，结果可复现。

use rand::rngs::SmallRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

use crate::ld_matrix::LdRandomAccess;

/// extract 参数（默认值对齐原版 `mixer.py snps`）。
#[derive(Clone, Debug)]
pub struct ExtractConfig {
    /// MAF 下限（`min(af,1-af) >= maf_min`），默认 0.05。
    pub maf_min: f64,
    /// LD 剪枝阈值（**严格 >** 才剪，与 `perform_ld_clump` 一致），默认 0.8。
    pub r2_threshold: f64,
    /// 随机 subset 上限，默认 2_000_000。
    pub subset: usize,
    /// 随机种子，默认 123。
    pub seed: u64,
}

impl Default for ExtractConfig {
    fn default() -> Self {
        Self { maf_min: 0.05, r2_threshold: 0.8, subset: 2_000_000, seed: 123 }
    }
}

/// 选 tag 子集。返回选中的 snp index（无特定顺序）。
///
/// - `maf`：per-SNP 的 `min(af, 1-af)`，长度 = n_snp。
/// - `ld`：LD 邻接（只需含 `r² > r2_threshold` 的对即可；传更大 CSR 也正确但更慢），
///   用于贪心剪枝时查每个 SNP 的高 LD 邻居。`row(i)` 返回 snp-index 邻居。
pub fn select_tags<L: LdRandomAccess + ?Sized>(maf: &[f64], ld: &L, cfg: &ExtractConfig) -> Vec<u32> {
    let n_snp = maf.len();
    let mut rng = SmallRng::seed_from_u64(cfg.seed);

    // 1. MAF 过滤 + 给每个候选分配随机优先级 U[0,1)
    let mut priority = vec![f64::NAN; n_snp];
    let mut candidates: Vec<u32> = Vec::new();
    for (i, &m) in maf.iter().enumerate() {
        if m.is_finite() && m >= cfg.maf_min {
            priority[i] = rng.gen_range(0.0..1.0);
            candidates.push(i as u32);
        }
    }

    // 2. 随机 subset：洗牌候选，取前 subset 个（等价于无放回均匀抽样）
    candidates.shuffle(&mut rng);
    let take = cfg.subset.min(candidates.len());
    let mut in_subset = vec![false; n_snp];
    for &s in &candidates[..take] {
        in_subset[s as usize] = true;
    }

    // 3. 贪心 LD 剪枝：按优先级降序排 subset，依次保留代表，剪掉 r²>threshold 的邻居
    let mut order: Vec<u32> = candidates[..take].to_vec();
    order.sort_by(|&a, &b| {
        priority[b as usize]
            .partial_cmp(&priority[a as usize])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut pruned = vec![false; n_snp];
    let mut tags: Vec<u32> = Vec::with_capacity(take);
    for &idx in &order {
        if pruned[idx as usize] {
            continue;
        }
        tags.push(idx);
        // 代表 idx 把它在 LD 里 r²>threshold 且仍在 subset 的邻居剪掉。
        // 用 f32 比较（对齐原版 perform_ld_clump 的 `r2_value <= r2_threshold` f32 比较），
        // 避免 `0.8f32 as f64 = 0.8000000119... > 0.8` 的精度误判。
        let thr_f32 = cfg.r2_threshold as f32;
        let (cols, r2s) = ld.row(idx as usize);
        for (k, &nb) in cols.iter().enumerate() {
            if (nb as usize) < n_snp
                && in_subset[nb as usize]
                && !pruned[nb as usize]
                && r2s[k] > thr_f32
            {
                pruned[nb as usize] = true;
            }
        }
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ld_matrix::LdBlock;

    fn ld_from(triples: &[(u32, u32, f64)], n: usize) -> LdBlock {
        LdBlock::from_coo(triples, n)
    }

    #[test]
    fn maf_filter_excludes_low_maf() {
        // 4 SNP：maf = [0.06, 0.02, 0.05, 0.1]，阈值 0.05 → 候选 {0,2,3}
        let maf = vec![0.06, 0.02, 0.05, 0.1];
        let ld = ld_from(&[], 4); // 无 LD → 不剪枝
        let cfg = ExtractConfig { maf_min: 0.05, r2_threshold: 0.8, subset: 100, seed: 1 };
        let tags = select_tags(&maf, &ld, &cfg);
        let mut t = tags.clone();
        t.sort();
        assert_eq!(t, vec![0, 2, 3], "应排除 maf<0.05 的 idx 1");
    }

    #[test]
    fn subset_caps_count() {
        let maf = vec![0.1; 100];
        let ld = ld_from(&[], 100);
        let cfg = ExtractConfig { maf_min: 0.05, r2_threshold: 0.8, subset: 10, seed: 1 };
        let tags = select_tags(&maf, &ld, &cfg);
        assert!(tags.len() <= 10, "subset 上限 10，got {}", tags.len());
    }

    #[test]
    fn ld_pruning_removes_high_r2_neighbors() {
        // 3 SNP：0-1 r²=0.9（应互斥），0-2 r²=0.3（可共存）
        let maf = vec![0.1, 0.1, 0.1];
        let ld = ld_from(&[(0, 1, 0.9), (0, 2, 0.3)], 3);
        let cfg = ExtractConfig { maf_min: 0.05, r2_threshold: 0.8, subset: 100, seed: 1 };
        let tags = select_tags(&maf, &ld, &cfg);
        // 0 和 1 不能同时入选（r²=0.9>0.8）
        let has0 = tags.contains(&0);
        let has1 = tags.contains(&1);
        assert!(!(has0 && has1), "r²=0.9 的 0 和 1 不应同时入选: {:?}", tags);
    }

    #[test]
    fn deterministic_with_seed() {
        let maf = vec![0.1; 50];
        let mut triples = Vec::new();
        for i in 0..50u32 {
            for j in (i + 1)..50 {
                triples.push((i, j, 0.5));
            }
        }
        let ld = ld_from(&triples, 50);
        let cfg = ExtractConfig { maf_min: 0.05, r2_threshold: 0.8, subset: 20, seed: 42 };
        let t1 = select_tags(&maf, &ld, &cfg);
        let t2 = select_tags(&maf, &ld, &cfg);
        assert_eq!(t1, t2, "同 seed 应可复现");
    }

    #[test]
    fn r2_boundary_strict_greater() {
        // r²==0.8（阈值边界）应保留两端（严格 > 才剪）
        let maf = vec![0.1, 0.1];
        let ld = ld_from(&[(0, 1, 0.8)], 2);
        let cfg = ExtractConfig { maf_min: 0.05, r2_threshold: 0.8, subset: 100, seed: 1 };
        let tags = select_tags(&maf, &ld, &cfg);
        assert_eq!(tags.len(), 2, "r²==0.8 不应剪（严格>），got {:?}", tags);
    }
}
