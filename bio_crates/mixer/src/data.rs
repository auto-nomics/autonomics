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

/// Univariate MiXeR cost 的压缩输入：把整张 LD 矩阵折叠成 per-SNP 的两个
/// 充分统计量 `m1`/`m2`，使 cost 求值不再需要 LD 邻居表。
///
/// 推导（对照 [`ChromData`] 下的 `tag_moments`）：对 tag `j`，
///   A_j = ebeta2 · Σ_s n_j·h_s·r²_js
///   B_j = ebeta4 · Σ_s (n_j·h_s·r²_js)²
/// 其中 `ebeta2 = π·σ²_β`、`ebeta4 = 3π(1−π)(σ²_β)²` 仅依赖参数（全部 tag 共享），
/// 而 `n_j·h_s·r²` 是纯数据。故可把参数无关的邻居求和**预算**成两个标量：
///   m1_j = Σ_s n_j·h_s·r²_js
///   m2_j = Σ_s (n_j·h_s·r²_js)²
/// 求值时 `A_j = ebeta2·m1_j`、`B_j = ebeta4·m2_j`，O(1)/tag，无需访问 LD。
///
/// # 等价性
/// 数学上与逐邻居求和完全等价（仅把参数因子提出求和号外）。浮点上相差 ULP 级
/// 重结合，远低于优化器收敛阈值（DE `tol=0.01`、NM `tol=1e-7`），拟合结果不变。
///
/// # 为什么这样做
/// 内存从 `O(nnz)` 的 CSR 塌缩到 `O(n_snp)`。优化器对 cost 的数万次评估只读
/// `m1/m2`，不再触碰 LD——CSR 预算完即可释放。原本每次 cost 评估都重算一遍
/// 邻居求和（`tag_moments` 在 DE/NM 的每一次评估里都被调用）也一并消除。
///
/// 注意：`n` 与 `h` 向量已被折进 `m1/m2`（`h` 另外折进 `totalhet` 供 `h²` 派生），
/// 故本结构不再持有它们。
#[derive(Debug, Clone)]
pub struct UnivariateSufficient {
    /// per-SNP z-score（cost 只读 `tags` 子集）。
    pub z: Vec<f64>,
    /// per-SNP 权重（随机剪枝 / 逆 LD-score）。
    pub weights: Vec<f64>,
    /// m1_j = Σ_s n_j·h_s·r²_js（参数无关，预算一次）。
    pub m1: Vec<f64>,
    /// m2_j = Σ_s (n_j·h_s·r²_js)²（参数无关，预算一次）。
    pub m2: Vec<f64>,
    /// tag 子集（snp index），cost 只遍历这些 snp。
    pub tags: Vec<u32>,
    /// totalhet = Σ_s h_s（全部 SNP），`FitResult` 派生 h² 用。
    pub totalhet: f64,
    /// SNP 总数（= `z.len()`）。
    pub n_snp: usize,
}

impl UnivariateSufficient {
    /// 从已装配的 [`ChromData`]（含全局块对角 CSR）一次性预算充分统计量。
    ///
    /// 单趟扫描全部 LD 行，把每行的邻居求和写进 `m1/m2`。预算完成后，原
    /// `ChromData`（含 `ld`/`n`/`h`）即可由调用方 `drop` 释放——这正是把"扫 LD"
    /// 从优化器的数万次评估降到 1 次的关键。
    pub fn from_chrom_data(data: &ChromData) -> Self {
        let n_snp = data.n_snp();
        let mut m1 = vec![0.0; n_snp];
        let mut m2 = vec![0.0; n_snp];
        for j in 0..n_snp {
            let n_j = data.n[j];
            let (cols, r2s) = data.ld.row(j);
            // 与 tag_moments 完全相同的 a2ij 定义，仅去掉参数因子 ebeta2/ebeta4。
            let mut s1 = 0.0;
            let mut s2 = 0.0;
            for (k, &s) in cols.iter().enumerate() {
                let a2ij = n_j * data.h[s as usize] * r2s[k];
                s1 += a2ij;
                s2 += a2ij * a2ij;
            }
            m1[j] = s1;
            m2[j] = s2;
        }
        let totalhet = data.h.iter().sum();
        Self {
            z: data.z.clone(),
            weights: data.weights.clone(),
            m1,
            m2,
            tags: data.tags.clone(),
            totalhet,
            n_snp,
        }
    }

    /// tag 数（cost 实际遍历的 SNP 数）。
    pub fn n_tag(&self) -> usize {
        self.tags.len()
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

    #[test]
    fn sufficient_m1_m2_match_tag_moments() {
        // m1/m2 必须等于 tag_moments 去掉参数因子后的邻居求和。
        use crate::cost::tag_moments;
        let z = vec![1.0, 2.0, 3.0];
        let n = vec![100.0, 100.0, 100.0];
        let h = vec![0.5, 0.4, 0.3];
        let triples = vec![(1, 0, 0.8), (1, 2, 0.5)];
        let data = ChromData::new(z, n, h, &triples);
        let suff = UnivariateSufficient::from_chrom_data(&data);

        // tag_moments 以 ebeta2/ebeta4=1 调用 → 返回值即原始求和，应与 m1/m2 逐位一致。
        for j in 0..data.n_snp() {
            let (a, b) = tag_moments(&data, j, 1.0, 1.0);
            assert!(
                (a - suff.m1[j]).abs() < 1e-12,
                "j={j}: m1 不一致 ({a} vs {})",
                suff.m1[j]
            );
            assert!(
                (b - suff.m2[j]).abs() < 1e-12,
                "j={j}: m2 不一致 ({b} vs {})",
                suff.m2[j]
            );
        }

        // totalhet = Σ h_s
        assert!((suff.totalhet - (0.5 + 0.4 + 0.3)).abs() < 1e-12);
        assert_eq!(suff.n_snp, 3);
    }
}
