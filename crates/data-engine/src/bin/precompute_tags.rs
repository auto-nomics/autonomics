//! precompute_tags2 —— chr22 单染色体教学版。
//!
//! 演示核心思路：**LD 只扫 Iceberg 一次**，物化进 `Vec<(a, b, r²)>`，然后这份
//! 内存数据**复用两次**：
//!   1. 筛 r²>0.8 建 LdBlock 邻接 → select_tags 选 tag
//!   2. 筛 tag 诱导（任一端是 tag）→ 输出子图
//!
//! 不用 JOIN：tag 选择要的是候选 SNP 列表（从 AF 来），子图要两端 h（按 index
//! 查 h_vec）——AF 和 LD 分开读、用 rsid→idx 关联，比 SQL JOIN 干净（JOIN 只能
//! 关联一端 AF）。

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use arrow_array::{
    Array, Float32Array, Float64Array, Int64Array, RecordBatch, StringArray, StringViewArray,
};
use arrow_schema::{DataType, Field, Schema};
use datafusion::prelude::SessionContext;
use futures::TryStreamExt;
use parquet::arrow::ArrowWriter;

use datalake::Datalake;
use mixer::extract::{ExtractConfig, select_tags};
use mixer::ld_matrix::LdBlock;
use tokio::time::Instant;

const MAF_MIN: f64 = 0.05; // extract MAF 下限
const R2_PRUNE: f64 = 0.8; // extract LD 剪枝阈值（> 才剪）
const R2_MIN: f64 = 0.05; // 子图收的最低 r²（fold 阈值）
const GLOBAL_SUBSET: usize = 2_000_000; // 全基因组 subset 总预算（按染色体 SNP 占比分配）
const SEED: u64 = 123;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

/// 杂合度 h = 2·maf·(1-maf)。
#[inline]
fn h_of(maf: f64) -> f64 {
    2.0 * maf * (1.0 - maf)
}

fn col_str(b: &RecordBatch, name: &str) -> Result<Vec<String>, BoxErr> {
    let col = b
        .column_by_name(name)
        .ok_or_else(|| BoxErr::from(format!("column {name} missing")))?;
    if let Some(a) = col.as_any().downcast_ref::<StringArray>() {
        Ok((0..a.len())
            .map(|i| {
                if a.is_null(i) {
                    String::new()
                } else {
                    a.value(i).to_string()
                }
            })
            .collect())
    } else if let Some(a) = col.as_any().downcast_ref::<StringViewArray>() {
        Ok((0..a.len())
            .map(|i| {
                if a.is_null(i) {
                    String::new()
                } else {
                    a.value(i).to_string()
                }
            })
            .collect())
    } else {
        Err(BoxErr::from(format!("column {name} not a string type")))
    }
}

fn col_f64<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Float64Array, BoxErr> {
    b.column_by_name(name)
        .ok_or_else(|| BoxErr::from(format!("column {name} missing")))?
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| BoxErr::from(format!("column {name} not Float64")))
}

/// 演示用：统计的染色体范围。
const STATS_CHROMS: &[u32] = &[
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
];

/// 查询每条染色体的 SNP 数量（`COUNT(*) FROM af.eur_af WHERE chrom=N`）。
/// 返回 `Vec<(chrom, count)>`。
async fn chrom_snp_counts(
    ctx: &SessionContext,
    chroms: &[u32],
) -> Result<Vec<(u32, usize)>, BoxErr> {
    let mut out = Vec::with_capacity(chroms.len());
    for c in chroms {
        let sql = format!("SELECT COUNT(*) AS n FROM iceberg.af.eur_af WHERE chrom = {c}");
        let batches = ctx.sql(&sql).await?.collect().await?;
        let n = batches
            .first()
            .and_then(|b| b.column_by_name("n"))
            .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(0) as usize)
            .ok_or_else(|| BoxErr::from(format!("chr{c}: COUNT 结果解析失败")))?;
        out.push((*c, n));
    }
    Ok(out)
}

/// 打印各染色体 SNP 数量占比（含累计条形图，方便看分布）。
fn print_chrom_proportions(counts: &[(u32, usize)]) {
    let total: usize = counts.iter().map(|(_, n)| n).sum();
    if total == 0 {
        eprintln!("(no SNPs)");
        return;
    }
    // 找最大占比用于条形图缩放（40 格满）
    let max_pct = counts
        .iter()
        .map(|(_, n)| (*n as f64) / (total as f64) * 100.0)
        .fold(0.0_f64, f64::max)
        .max(1e-9);
    eprintln!("各染色体 SNP 占比（总计 {total}）:");
    for (chrom, n) in counts {
        let pct = (*n as f64) / (total as f64) * 100.0;
        let bars = ((pct / max_pct) * 40.0).round() as usize;
        eprintln!(
            "  chr{chrom:>2}: {:>10} ({pct:5.2}%) {:<40}",
            n,
            "█".repeat(bars)
        );
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxErr> {
    // 唯一 CLI 参数：染色体号
    #[allow(non_snake_case)]
    let CHR: u32 = std::env::args()
        .nth(1)
        .ok_or("用法: precompute_tags2 <chrom>\n例: cargo run --bin precompute_tags2 -- 22")?
        .parse()?;
    eprintln!("=== 预算 chr{CHR} ===");

    let ctx = Datalake::new().get_ctx().await?;

    // 演示：各染色体 SNP 数量占比（独立于下面的 chr 预算，只跑 COUNT）
    let counts = chrom_snp_counts(&ctx, STATS_CHROMS).await?;
    print_chrom_proportions(&counts);

    // 按染色体 SNP 占比分配 subset 配额（而非固定 2M）
    let total_snps: usize = counts.iter().map(|(_, n)| n).sum();
    let chr_snps = counts
        .iter()
        .find(|(c, _)| *c == CHR)
        .map(|(_, n)| *n)
        .unwrap_or(0);
    let proportion = if total_snps > 0 {
        chr_snps as f64 / total_snps as f64
    } else {
        1.0
    };
    let subset = ((GLOBAL_SUBSET as f64) * proportion).max(1.0) as usize;
    eprintln!(
        "[0] chr{CHR} SNP 占比 {proportion:.4}（{chr_snps}/{total_snps}）→ subset 配额 {subset}（全基因组预算 {GLOBAL_SUBSET}）"
    );

    // ───────────────────────────────────────────────────────────────
    // Step 1: 读 AF（chr22）→ rsids / maf_vec / rsid→idx / h_vec
    // ───────────────────────────────────────────────────────────────
    let af_sql = format!("SELECT id, alt_freq FROM iceberg.af.eur_af WHERE chrom = {CHR}");
    let af_batches = ctx.sql(&af_sql).await?.collect().await?;
    let mut rsids: Vec<String> = Vec::new();
    let mut maf_vec: Vec<f64> = Vec::new();
    let mut h_vec: Vec<f64> = Vec::new();
    let mut rsid_to_idx: HashMap<String, u32> = HashMap::new();
    for b in &af_batches {
        let ids = col_str(b, "id")?;
        let afs = col_f64(b, "alt_freq")?;
        for r in 0..b.num_rows() {
            let id = ids[r].clone();
            if rsid_to_idx.contains_key(&id) {
                continue;
            }
            let f = afs.value(r);
            let maf = f.min(1.0 - f); // 对称 MAF
            let idx = rsids.len() as u32;
            rsid_to_idx.insert(id.clone(), idx);
            rsids.push(id);
            maf_vec.push(maf);
            h_vec.push(h_of(maf));
        }
    }
    let n_snp = rsids.len();
    eprintln!("[1] AF chr{CHR}: {n_snp} SNPs");

    // ───────────────────────────────────────────────────────────────
    // Step 2: 读 LD 一次（r²≥R2_MIN），物化进 Vec —— 关键步骤
    // 后续建邻接、出子图都复用这份数据，不再碰 Iceberg。
    // ───────────────────────────────────────────────────────────────
    let ld_sql = format!(
        "SELECT id_a, id_b, unphased_r2 FROM iceberg.ld_matrix.eur_chr{CHR} WHERE unphased_r2 >= {R2_MIN}"
    );
    let mut ld_pairs: Vec<(u32, u32, f64)> = Vec::new(); // (global_a, global_b, r²)
    let mut stream = ctx.sql(&ld_sql).await?.execute_stream().await?;
    let start_ts = Instant::now();
    while let Some(batch) = stream.try_next().await? {
        let a_ids = col_str(&batch, "id_a")?;
        let b_ids = col_str(&batch, "id_b")?;
        let r2s = col_f64(&batch, "unphased_r2")?;
        for r in 0..batch.num_rows() {
            let a = a_ids[r].as_str();
            let b = b_ids[r].as_str();
            // 两端都得在 AF 面板里（有 index）才算有效对
            if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
                ld_pairs.push((ta, tb, r2s.value(r)));
            }
        }
    }

    let elapsed = start_ts.elapsed().as_secs();
    eprintln!(
        "[2] LD chr{CHR}: 物化 {} 对 (r²≥{R2_MIN}) —— 只扫这一次, 耗时 {elapsed} s",
        ld_pairs.len()
    );

    // ───────────────────────────────────────────────────────────────
    // Step 3: 从 ld_pairs 筛 r²>R2_PRUNE 建邻接 CSR（第一次复用）
    // ───────────────────────────────────────────────────────────────
    let mut triples: Vec<(u32, u32, f64)> = Vec::new();
    for &(a, b, r2) in &ld_pairs {
        if r2 > R2_PRUNE {
            triples.push((a, b, r2));
            triples.push((b, a, r2)); // 对称化双向（select_tags 要 ld.row 双向访问）
        }
    }
    let adj = LdBlock::from_coo(&triples, n_snp);
    eprintln!("[3] 邻接: {} 对 r²>{R2_PRUNE}", triples.len() / 2);

    // ───────────────────────────────────────────────────────────────
    // Step 4: select_tags（MAF 过滤 → seeded 随机 subset → 贪心剪枝）
    // 内部完成随机抽样 + 剪枝，返回 tag 的全局 index。
    // ───────────────────────────────────────────────────────────────
    let cfg = ExtractConfig {
        maf_min: MAF_MIN,
        r2_threshold: R2_PRUNE,
        subset,
        seed: SEED,
    };
    let tags = select_tags(&maf_vec, &adj, &cfg);
    let tag_set: std::collections::HashSet<u32> = tags.iter().copied().collect();
    eprintln!(
        "[4] tags: {} 个（subset={subset}, seed={SEED}）",
        tags.len()
    );

    // ───────────────────────────────────────────────────────────────
    // Step 5: 从 ld_pairs 筛 tag 诱导子图，**直接存 rsid**（不存 index）。
    // 保证 id_a 永远是 tag（如果 b 是 tag 则交换）。
    // 持久化数据只用 rsid——index 是内存临时簿记，不落盘。
    // ───────────────────────────────────────────────────────────────
    let schema = Arc::new(Schema::new(vec![
        Field::new("id_a", DataType::Utf8, false), // ← tag 的 rsid
        Field::new("id_b", DataType::Utf8, false), // ← 邻居的 rsid
        Field::new("r2", DataType::Float32, false),
        Field::new("h_a", DataType::Float32, false), // tag 的 h
        Field::new("h_b", DataType::Float32, false), // 邻居的 h
    ]));
    let mut a_buf: Vec<String> = Vec::new();
    let mut b_buf: Vec<String> = Vec::new();
    let mut r2_buf: Vec<f32> = Vec::new();
    let mut ha_buf: Vec<f32> = Vec::new();
    let mut hb_buf: Vec<f32> = Vec::new();
    for &(a, b, r2) in &ld_pairs {
        if tag_set.contains(&a) {
            a_buf.push(rsids[a as usize].clone());
            b_buf.push(rsids[b as usize].clone());
            r2_buf.push(r2 as f32);
            ha_buf.push(h_vec[a as usize] as f32);
            hb_buf.push(h_vec[b as usize] as f32);
        }
        if tag_set.contains(&b) {
            a_buf.push(rsids[b as usize].clone());
            b_buf.push(rsids[a as usize].clone());
            r2_buf.push(r2 as f32);
            ha_buf.push(h_vec[b as usize] as f32);
            hb_buf.push(h_vec[a as usize] as f32);
        }
    }
    let n_edges = a_buf.len();
    let subgraph_path = format!("chr{}_subgraph.parquet", CHR);
    {
        let file = std::fs::File::create(&subgraph_path)?;
        let mut writer = ArrowWriter::try_new(file, schema.clone(), None)?;
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(a_buf)),
                Arc::new(StringArray::from(b_buf)),
                Arc::new(Float32Array::from(r2_buf)),
                Arc::new(Float32Array::from(ha_buf)),
                Arc::new(Float32Array::from(hb_buf)),
            ],
        )?;
        writer.write(&batch)?;
        writer.close()?;
    }
    eprintln!("[5] 子图: {n_edges} 条边 → {subgraph_path}");

    // ───────────────────────────────────────────────────────────────
    // Step 6: 写 tag rsid 列表
    // ───────────────────────────────────────────────────────────────
    let tags_path = format!("chr{}_tags.txt", CHR);
    {
        let mut f = std::fs::File::create(&tags_path)?;
        for idx in &tags {
            writeln!(f, "{}", rsids[*idx as usize])?;
        }
    }
    eprintln!("[6] tags: {} rsid → {tags_path}", tags.len());

    // ───────────────────────────────────────────────────────────────
    // Step 7: 按 tag 聚合充分统计量（N-free），输出 chr{N}_tagsuff.parquet。
    //
    // m1[tag] = N_tag × S1[tag],  S1 = Σ h_neighbor × r²
    // m2[tag] = N_tag² × S2[tag], S2 = Σ (h_neighbor × r²)²
    // sum_r2[tag] = SR[tag],       SR = Σ r²
    // weight[tag] = 1/(1+SR)        （LdScore 模式）
    //
    // 运行时只需逐元素乘 N_tag（来自 sumstats），不再扫任何 LD 数据。
    // ───────────────────────────────────────────────────────────────
    let mut s1 = vec![0.0f64; n_snp];
    let mut s2 = vec![0.0f64; n_snp];
    let mut sr = vec![0.0f64; n_snp];
    for &(a, b, r2) in &ld_pairs {
        if tag_set.contains(&a) {
            let hb = h_vec[b as usize];
            let v = hb * r2;
            s1[a as usize] += v;
            s2[a as usize] += v * v;
            sr[a as usize] += r2;
        }
        if tag_set.contains(&b) {
            let ha = h_vec[a as usize];
            let v = ha * r2;
            s1[b as usize] += v;
            s2[b as usize] += v * v;
            sr[b as usize] += r2;
        }
    }
    // 只输出 tag 行（非 tag 的 S1/S2/SR 无意义）
    let mut tag_id_buf: Vec<String> = Vec::with_capacity(tags.len());
    let mut s1_buf: Vec<f64> = Vec::with_capacity(tags.len());
    let mut s2_buf: Vec<f64> = Vec::with_capacity(tags.len());
    let mut sr_buf: Vec<f64> = Vec::with_capacity(tags.len());
    let mut wt_buf: Vec<f64> = Vec::with_capacity(tags.len());
    for &idx in &tags {
        let i = idx as usize;
        tag_id_buf.push(rsids[i].clone());
        s1_buf.push(s1[i]);
        s2_buf.push(s2[i]);
        sr_buf.push(sr[i]);
        wt_buf.push(1.0 / (1.0 + sr[i]));
    }
    let suff_schema = Arc::new(Schema::new(vec![
        Field::new("id_tag", DataType::Utf8, false),
        Field::new("s1", DataType::Float64, false),
        Field::new("s2", DataType::Float64, false),
        Field::new("sr", DataType::Float64, false),
        Field::new("weight", DataType::Float64, false),
    ]));
    let suff_path = format!("chr{}_tagsuff.parquet", CHR);
    {
        use arrow_array::Float64Array;
        let file = std::fs::File::create(&suff_path)?;
        let mut writer = ArrowWriter::try_new(file, suff_schema.clone(), None)?;
        let batch = RecordBatch::try_new(
            suff_schema,
            vec![
                Arc::new(StringArray::from(tag_id_buf)),
                Arc::new(Float64Array::from(s1_buf)),
                Arc::new(Float64Array::from(s2_buf)),
                Arc::new(Float64Array::from(sr_buf)),
                Arc::new(Float64Array::from(wt_buf)),
            ],
        )?;
        writer.write(&batch)?;
        writer.close()?;
    }
    eprintln!("[7] tagsuff: {} tags → {suff_path}", tags.len());

    Ok(())
}
