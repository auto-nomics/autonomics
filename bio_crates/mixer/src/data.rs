//! 单条染色体的全部 cost function 输入数据。
//!
//! 关键约定：tag 和 snp 共用同一套 index 空间（0..n_snp）。每个 SNP 既是
//! 被建模的 tag（有自己的 z-score），也可能是其他 tag 的 LD 邻居（贡献效应）。
//! 因此 `LdBlock.column_index` 里的 snp index 直接索引进下面的 z/n/h 向量。
//!
//! 多染色体时，cost = 各染色体 cost 之和（LD 不跨染色体，互相独立）。

use crate::ld_matrix::LdBlock;

/// 单条染色体的 cost function 输入。
#[derive(Debug, Clone)]
pub struct ChromData {
    /// per-snp z-score（GWAS 检验统计量）。仅 tag 的 z 被读取。
    pub z: Vec<f64>,
    /// per-snp 样本量 N
    pub n: Vec<f64>,
    /// per-snp heterozygosity = 2·maf·(1−maf)，来自 af.eur_af.alt_freq
    pub h: Vec<f64>,
    /// per-snp 权重（随机剪枝 / 逆 LD-score）。非 tag 处应为 0。
    pub weights: Vec<f64>,
    /// 稀疏 LD，col_idx 索引进上面的 z/n/h 向量。
    pub ld: LdBlock,
    /// tag 子集（snp index）。cost 只遍历这些 snp。
    /// 原版 MiXeR 里 tag = 有有效 z 且通过 extract 筛选的 snp，是 snp 的子集。
    pub tags: Vec<u32>,
}

impl ChromData {
    /// SNP 总数（tag 与 snp 共用此 index 空间）。
    pub fn n_snp(&self) -> usize {
        self.z.len()
    }

    /// 从原始向量 + COO LD 三元组构建，权重默认全 1.0，**所有 snp 都是 tag**。
    ///
    /// 要求 z/n/h 长度相同；`triples` 里的 tag/snp index 都必须 < 该长度。
    /// 需要指定 tag 子集（extract）+ 非均匀权重（randprune）时，构建后覆盖
    /// `tags` 和 `weights` 字段即可。
    pub fn new(z: Vec<f64>, n: Vec<f64>, h: Vec<f64>, triples: &[(u32, u32, f64)]) -> Self {
        assert_eq!(z.len(), n.len(), "z/n 长度不一致");
        assert_eq!(z.len(), h.len(), "z/h 长度不一致");
        let n_snp = z.len();
        Self {
            z,
            n,
            h,
            weights: vec![1.0; n_snp],
            ld: LdBlock::from_coo(triples, n_snp),
            tags: (0..n_snp as u32).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_basic() {
        // 3 个 SNP: tag1 与 snp0/snp2 有 LD
        let z = vec![1.0, 2.0, 3.0];
        let n = vec![100.0, 100.0, 100.0];
        let h = vec![0.5, 0.4, 0.3];
        let triples = vec![(1, 0, 0.8), (1, 2, 0.5)];
        let chrom = ChromData::new(z, n, h, &triples);

        assert_eq!(chrom.n_snp(), 3);
        assert_eq!(chrom.weights, vec![1.0, 1.0, 1.0]);

        // tag1 有两个邻居 snp0, snp2
        let (col, _) = chrom.ld.row(1);
        let mut got: Vec<u32> = col.to_vec();
        got.sort();
        assert_eq!(got, vec![0, 2]);
    }

    #[test]
    fn empty_ld() {
        // 无 LD 对的退化情形：每个 SNP 独立
        let z = vec![0.5, 0.6];
        let n = vec![50.0, 50.0];
        let h = vec![0.4, 0.4];
        let chrom = ChromData::new(z, n, h, &[]);
        assert_eq!(chrom.n_snp(), 2);
        assert!(chrom.ld.row(0).0.is_empty());
        assert!(chrom.ld.row(1).0.is_empty());
    }

    #[test]
    #[should_panic]
    fn mismatched_lengths_panics() {
        ChromData::new(vec![1.0], vec![1.0, 2.0], vec![1.0], &[]);
    }
}
