//! 单条染色体的稀疏LD矩阵，CSR格式
//!
//! 数据湖中ld_matrix.eur_chr{N} 存储的是COO三元组,
//! 计算MiXeR需要转为CSR的矩阵存储形式以支持行扫描的需求

/// LD block in single chromesome, stored in CSR format
#[derive(Default, Debug, Clone)]
pub struct LdBlock {
    /// tag SNP 总数
    pub n_tag: usize,
    /// row_ptr.len() == n_tag + 1.
    /// tag `i` 的邻居 = col_indx[row_ptr[i]...row_ptr[i+1]]，对应 r2 同区间.
    pub row_ptr: Vec<u32>,
    pub column_index: Vec<u32>,
    pub r2: Vec<f64>,
}

impl LdBlock {
    /// 从COO构建CSR
    ///
    /// `triples`: (tag_idx, snp_idx, r2) 的列表，顺序任意。
    /// `n_tag`: tag 总数（用于确定 row_ptr 长度）。
    pub fn from_coo(triples: &[(u32, u32, f64)], n_tag: usize) -> Self {
        // 第1步：数每个 tag 有多少个邻居，错位存到 row_ptr[+1]
        let mut row_ptr = vec![0u32; n_tag + 1];
        for (tag_idx, _snp_idx, _r2) in triples {
            row_ptr[(*tag_idx as usize) + 1] += 1;
        }

        // 第2步：前缀和，row_ptr 变成真正的行起点偏移
        for i in 1..=n_tag {
            row_ptr[i] += row_ptr[i - 1];
        }

        // 第3步：按行填值。pos 是写入游标，追踪每行当前该写到哪个位置
        let nnz = triples.len();
        let mut col_idx = vec![0u32; nnz];
        let mut r2 = vec![0.0f64; nnz];
        let mut pos = row_ptr.clone(); // pos[i] = tag i 下一个该写入的位置

        for (tag_idx, snp_idx, r2_val) in triples {
            let p = pos[*tag_idx as usize] as usize;
            col_idx[p] = *snp_idx;
            r2[p] = *r2_val;
            pos[*tag_idx as usize] += 1;
        }

        LdBlock {
            n_tag,
            row_ptr,
            column_index: col_idx,
            r2,
        }
    }

    /// 遍历 tag `i` 的所有邻居，返回 (snp_idx, r2) 的切片。
    pub fn row(&self, i: usize) -> (&[u32], &[f64]) {
        let start = self.row_ptr[i] as usize;
        let end = self.row_ptr[i + 1] as usize;
        (&self.column_index[start..end], &self.r2[start..end])
    }
}

/// 按 tag 行号随机访问 LD 邻居的统一接口。
///
/// `LdBlock`（单块 CSR）和 [`BlockDiagonal`]（多染色体块对角视图）都实现它，
/// 让 `randprune_weights` / 充分统计量构建等"需要随机访问某个 tag 邻居"的逻辑
/// 可以**不 merge 出全局 CSR**，直接在逐染色体块上工作——省掉 `merge_blocks`
/// 那次整份复制（峰值 ~2× nnz → ~1× nnz）。
pub trait LdRandomAccess {
    /// 全局行数（= 全局 SNP 数）。
    fn n_tag(&self) -> usize;
    /// 取 tag `i` 的全部邻居 `(snp_idx, r2)` 切片。`i` 必须在 `[0, n_tag)`。
    fn row(&self, i: usize) -> (&[u32], &[f64]);
}

impl LdRandomAccess for LdBlock {
    fn n_tag(&self) -> usize {
        self.n_tag
    }
    fn row(&self, i: usize) -> (&[u32], &[f64]) {
        LdBlock::row(self, i)
    }
}

/// 多染色体块对角 LD 视图：把若干**连续**的本地 CSR 块当成一个全局 CSR 来随机访问。
///
/// 各块占据全局行 `[offset_k, offset_k + block_k.n_tag)`，且必须首尾相接覆盖
/// `[0, n_total)`。`row(i)` 用二分定位 i 所属块、转成本地行号后委托给该块。
///
/// 因为 LD 不跨染色体，块对角视图的 `row(i)` 与"把所有块 merge 成一张全局 CSR
/// 后的 `row(i)`"逐字节一致——但**不需要那次整份复制**。用于让 randprune 直接吃
/// `chrom_blocks`（`Vec<(offset, LdBlock)>`）而非 `merge_blocks` 的产物。
#[derive(Default, Debug, Clone)]
pub struct BlockDiagonal {
    /// 各块起始全局行号（升序）。`offsets.len() == blocks.len()`。
    offsets: Vec<usize>,
    /// 各块的本地 CSR（行号 0..n_tag）。
    blocks: Vec<LdBlock>,
    /// 全局行数（= 末块 offset + 末块 n_tag）。
    n_total: usize,
}

impl BlockDiagonal {
    /// 从 `(offset, block)` 列表构建。要求块覆盖 `[0, n_total)` 无缝；本方法据此推断
    /// `n_total`，不做严格校验（调用方负责连续性，与 `merge_blocks` 同前）。
    pub fn new(blocks: Vec<(usize, LdBlock)>) -> Self {
        let n_total = blocks.last().map(|(off, b)| off + b.n_tag).unwrap_or(0);
        let offsets = blocks.iter().map(|(off, _)| *off).collect();
        Self {
            offsets,
            blocks: blocks.into_iter().map(|(_, b)| b).collect(),
            n_total,
        }
    }
}

impl LdRandomAccess for BlockDiagonal {
    fn n_tag(&self) -> usize {
        self.n_total
    }

    fn row(&self, i: usize) -> (&[u32], &[f64]) {
        // 找最大的 offset <= i（i 所属块）。
        let k = match self.offsets.binary_search_by(|o| o.cmp(&i)) {
            // i 恰为某块起点
            Ok(k) => k,
            // i 落在 offsets[k-1] .. offsets[k] 之间 → 属于块 k-1
            Err(k) => k.saturating_sub(1),
        };
        let local = i - self.offsets[k];
        self.blocks[k].row(local)
    }
}

impl LdBlock {
    /// 把若干块对角的 CSR 子块拼成一个全局 CSR。
    ///
    /// 用于多染色体 LD 流式装配：每条染色体独立建一个本地 CSR（行号为
    /// `0..n_tag`，但 `column_index` 里存的是**全局** snp index），然后由本方法
    /// 按 `row_offset` 把各块的 `row_ptr` 平移到全局行号空间、把 `column_index`
    /// /`r2` 顺序拼接。LD 不跨染色体，因此全局矩阵呈块对角，拼接即完整还原。
    ///
    /// - `blocks[k] = (row_offset_k, block_k)`：第 k 块占据全局行
    ///   `[row_offset_k, row_offset_k + block_k.n_tag)`。
    /// - 各块的 `column_index` 必须已经是全局 snp index（调用方在 fill 阶段写入
    ///   全局 col，本方法不再做偏移）。
    /// - `n_total` 为全局行数（应等于各块 `n_tag` 之和且 `row_offset` 连续无缝）。
    pub fn merge_blocks(blocks: &[(usize, LdBlock)], n_total: usize) -> Self {
        let total_nnz: usize = blocks.iter().map(|(_, b)| b.r2.len()).sum();
        let mut row_ptr = vec![0u32; n_total + 1];
        let mut column_index = Vec::with_capacity(total_nnz);
        let mut r2 = Vec::with_capacity(total_nnz);

        // nnz_before：在全局 col_idx/r2 中，当前块之前已累计写入多少个元素。
        let mut nnz_before: u32 = 0;
        for (row_offset, block) in blocks {
            // 列下标 / r2 直接拼接（列下标调用方已写成全局）。
            column_index.extend_from_slice(&block.column_index);
            r2.extend_from_slice(&block.r2);
            // 把本地 row_ptr 平移到全局：global[row_offset + i] = nnz_before + local[i]。
            for i in 0..=block.n_tag {
                row_ptr[row_offset + i] = nnz_before + block.row_ptr[i];
            }
            nnz_before += block.row_ptr[block.n_tag];
        }

        // 连续无缝时 row_ptr[n_total] 应等于总 nnz——作为不变量校验。
        assert_eq!(
            row_ptr[n_total] as usize, total_nnz,
            "merge_blocks: row_ptr 末尾 ({}) 与总 nnz ({}) 不一致，blocks 可能未连续覆盖 [0, n_total)",
            row_ptr[n_total], total_nnz,
        );

        LdBlock {
            n_tag: n_total,
            row_ptr,
            column_index,
            r2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csr_basic() {
        // tag0 空, tag1 两个邻居, tag2 一个
        let triples = vec![(1u32, 7u32, 0.8), (1, 9, 0.3), (2, 2, 0.6)];
        let blk = LdBlock::from_coo(&triples, 3);

        // row_ptr 是前缀和: 前0行0个, 前1行0个, 前2行2个, 前3行3个
        assert_eq!(blk.row_ptr, vec![0, 0, 2, 3]);

        // tag0 没邻居
        let (c, r) = blk.row(0);
        assert!(c.is_empty());
        assert!(r.is_empty());

        // tag1 的两个邻居
        let (c, r) = blk.row(1);
        assert_eq!(c.len(), 2);
        assert_eq!(r.len(), 2);
        // 行内顺序是输入遇到顺序, 这里输入恰好有序
        assert_eq!(c, &[7, 9]);
        assert_eq!(r, &[0.8, 0.3]);

        // tag2 的一个邻居
        let (c, r) = blk.row(2);
        assert_eq!(c, &[2]);
        assert_eq!(r, &[0.6]);
    }

    #[test]
    fn csr_unordered_input() {
        // 乱序输入, 验证同一行的元素被连续分到一起
        let triples = vec![(2u32, 2u32, 0.6), (1u32, 9u32, 0.3), (1u32, 7u32, 0.8)];
        let blk = LdBlock::from_coo(&triples, 3);
        assert_eq!(blk.row_ptr, vec![0, 0, 2, 3]);

        // tag1 的邻居作为集合断言 (行内顺序不保证)
        let (c, _r) = blk.row(1);
        let mut got: Vec<u32> = c.to_vec();
        got.sort();
        assert_eq!(got, vec![7, 9]);
    }

    #[test]
    fn csr_empty() {
        let blk = LdBlock::from_coo(&[], 5);
        assert_eq!(blk.row_ptr, vec![0, 0, 0, 0, 0, 0]);
        for i in 0..5 {
            assert!(blk.row(i).0.is_empty());
        }
    }

    #[test]
    fn merge_blocks_matches_concatenated_from_coo() {
        // 两个染色体，全局 index 空间连续：
        //   chr A: rows 0..3 (base 0)，triples 用全局 col
        //   chr B: rows 3..6 (base 3)
        // merge_blocks 的结果必须与“把两块 triples 拼成一份再 from_coo”完全一致。
        let chr_a = vec![(0u32, 1u32, 0.8), (0, 2, 0.3), (1, 2, 0.6)];
        let chr_b = [(3u32, 4u32, 0.5), (4, 5, 0.9)];

        // (a) 参考实现：全局 from_coo。
        let mut all = chr_a.clone();
        all.extend(chr_b.iter().copied());
        let reference = LdBlock::from_coo(&all, 6);

        // (b) 流式实现：每染色体本地 from_coo（行号本地化，列号保持全局），再 merge。
        let local_a: Vec<(u32, u32, f64)> = chr_a.iter().map(|(t, s, r)| (*t, *s, *r)).collect();
        let local_b: Vec<(u32, u32, f64)> =
            chr_b.iter().map(|(t, s, r)| (*t - 3, *s, *r)).collect();
        let block_a = LdBlock::from_coo(&local_a, 3);
        let block_b = LdBlock::from_coo(&local_b, 3);
        let merged = LdBlock::merge_blocks(&[(0, block_a), (3, block_b)], 6);

        // row_ptr 必须逐字节一致。
        assert_eq!(merged.row_ptr, reference.row_ptr);
        assert_eq!(merged.n_tag, reference.n_tag);

        // 每行的邻居集合必须一致（行内顺序 from_coo 不保证，故按集合比较）。
        for i in 0..6 {
            let (m_c, m_r) = merged.row(i);
            let (r_c, r_r) = reference.row(i);
            assert_eq!(m_c.len(), r_c.len(), "row {i} 邻居数不一致");
            // 配对后按 col 排序比较 (col, r2)
            let mut m: Vec<(u32, f64)> = m_c.iter().copied().zip(m_r.iter().copied()).collect();
            let mut r: Vec<(u32, f64)> = r_c.iter().copied().zip(r_r.iter().copied()).collect();
            m.sort_by_key(|a| a.0);
            r.sort_by_key(|a| a.0);
            assert_eq!(m, r, "row {i} 邻居集合不一致");
        }
    }

    #[test]
    fn merge_blocks_handles_empty_block() {
        // 中间一条染色体无 LD（nnz=0 但占 2 行）。
        let empty = LdBlock::from_coo(&[], 2);
        let full = LdBlock::from_coo(&[(0u32, 1u32, 0.7)], 2);
        let merged = LdBlock::merge_blocks(&[(0, empty), (2, full)], 4);
        assert_eq!(merged.row_ptr, vec![0, 0, 0, 1, 1]);
        let (c, r) = merged.row(2);
        assert_eq!(c, &[1]);
        assert_eq!(r, &[0.7]);
    }

    #[test]
    fn block_diagonal_matches_merged_csr() {
        // 块对角视图的 row(i) 必须与 merge_blocks 后的全局 CSR 逐项一致——
        // 这保证 randprune 直接吃 BlockDiagonal 与吃 merged CSR 结果完全相同。
        // 约定（与节点一致）：块的行号本地化（0..n_tag），列号保持全局。
        use crate::ld_matrix::LdRandomAccess;
        // chr A 占全局 snp 0..3，chr B 占 3..6。LD 不跨染色体。
        //   chr A: (0,1,0.8) (0,2,0.3) (1,2,0.6)
        //   chr B: (3,4,0.5) (4,5,0.9)   ← 全局；本地行号 = 全局 − offset
        let block_a = LdBlock::from_coo(&[(0u32, 1u32, 0.8), (0, 2, 0.3), (1, 2, 0.6)], 3);
        let block_b = LdBlock::from_coo(&[(0u32, 4u32, 0.5), (1, 5, 0.9)], 3);

        let merged = LdBlock::merge_blocks(&[(0, block_a.clone()), (3, block_b.clone())], 6);
        let view = crate::ld_matrix::BlockDiagonal::new(vec![(0, block_a), (3, block_b)]);
        assert_eq!(view.n_tag(), 6);

        for i in 0..6 {
            let (mc, mr) = merged.row(i);
            let (vc, vr) = view.row(i);
            assert_eq!(mc, vc, "row {i} col 不一致");
            assert_eq!(mr, vr, "row {i} r2 不一致");
        }
    }
}
