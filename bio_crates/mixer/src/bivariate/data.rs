//! Bivariate cost function 的输入数据。
//!
//! 与 univariate 的 `ChromData` 平行，但持有两个 trait 的 z/N。LD 面板与
//! heterozygosity h 对两个 trait 共享（同一参考面板、同一 AF）。
//!
//! 约定与 `ChromData` 一致：tag 与 snp 共用 index 空间（0..n_snp）；
//! `tags` 是被拟合的 tag 子集（snp 的子集），cost 只遍历 tags。

use crate::ld_matrix::LdBlock;

/// 双变量 cost 输入。
#[derive(Debug, Clone)]
pub struct BivariateData {
    /// trait1 的 per-snp z-score（仅 tag 被读取）。
    pub z1: Vec<f64>,
    /// trait2 的 per-snp z-score。
    pub z2: Vec<f64>,
    /// trait1 的 per-snp 样本量 N。
    pub n1: Vec<f64>,
    /// trait2 的 per-snp 样本量 N。
    pub n2: Vec<f64>,
    /// per-snp heterozygosity = 2·maf·(1−maf)，两 trait 共享。
    pub h: Vec<f64>,
    /// per-snp 权重（随机剪枝）。非 tag 处应为 0。
    pub weights: Vec<f64>,
    /// 稀疏 LD，col_idx 索引进上面的向量。
    pub ld: LdBlock,
    /// tag 子集（snp index）。cost 只遍历这些 snp。
    pub tags: Vec<u32>,
}

impl BivariateData {
    /// SNP 总数。
    pub fn n_snp(&self) -> usize {
        self.z1.len()
    }

    /// 从原始向量 + COO LD 三元组构建，权重默认全 1.0，所有 snp 都是 tag。
    ///
    /// 要求 z1/z2/n1/n2/h 长度相同；`triples` 的 index 都 < 该长度。
    /// 需要 tag 子集 + 非均匀权重时，构建后覆盖 `tags`、`weights`。
    pub fn new(
        z1: Vec<f64>,
        z2: Vec<f64>,
        n1: Vec<f64>,
        n2: Vec<f64>,
        h: Vec<f64>,
        triples: &[(u32, u32, f64)],
    ) -> Self {
        assert_eq!(z1.len(), z2.len(), "z1/z2 长度不一致");
        assert_eq!(z1.len(), n1.len(), "z1/n1 长度不一致");
        assert_eq!(z1.len(), n2.len(), "z1/n2 长度不一致");
        assert_eq!(z1.len(), h.len(), "z1/h 长度不一致");
        let n_snp = z1.len();
        Self {
            z1,
            z2,
            n1,
            n2,
            h,
            weights: vec![1.0; n_snp],
            ld: LdBlock::from_coo(triples, n_snp),
            tags: (0..n_snp as u32).collect(),
        }
    }
}
