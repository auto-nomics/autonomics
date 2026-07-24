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
}
