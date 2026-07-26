//! Univariate MiXeR (fit1) 端到端测试：从 Iceberg 数据湖读取真实 GWAS 汇总统计，
//! 组装 [`mixer::data::ChromData`]，调用 [`mixer::fit::fit1`] 拟合 spike-and-slab 模型。
//!
//! 数据源：
//! - GWAS：`iceberg.gwas.bmi_ieu_a_2`（IEU a:2 BMI）。列 `effect_size`(β) / `std_error`(SE) /
//!   `sample_size`(N) / `rsid`。表中 `z_score` 列为空，故 Z = β / SE 就地计算。
//! - 等位基因频率：`iceberg.af.eur_af`（`id` = rsid，`alt_freq`）→ 杂合度 h = 2·f·(1−f)。
//! - LD 矩阵：`iceberg.ld_matrix.eur_chr{chrom}`（`id_a`, `id_b`, `unphased_r2`）。
//!
//! universe = GWAS ∩ AF（按 rsid）。tag 集 = universe 全部 SNP；权重由 randprune 生成
//! （与生产节点 `UnivariateMixerNode` 完全一致）。LD 不跨染色体，多染色体合并后呈块对角，
//! cost 与逐染色体求和等价。
//!
//! 这是一个重型测试（需连数据湖、读数百万 LD 对、跑差分进化×N）：
//! 默认 `#[ignore]`，不会随 `cargo test` 自动执行。手动运行：
//!
//! ```sh
//! cargo test -p mixer --test test_univariate_mixer_e2e -- --ignored --nocapture
//! ```
//!
//! 可用环境变量调参（均有默认）：
//! - `MIXER_E2E_CHROMS`：逗号分隔染色体，如 `21,22`（默认 `22`）。
//! - `MIXER_E2E_POS_LO` / `MIXER_E2E_POS_HI`：位置窗口 bp（默认 16_000_000 / 25_000_000）。
//!   None 表示不限制（跑整条染色体，很慢且耗内存）。
//! - `MIXER_E2E_DIFFEVO_REPEATS`：差分进化重复次数（默认 2，对齐原版 `--diffevo-fast-repeats=2`）。
//! - `MIXER_E2E_R2_MIN`：LD r² 下限（默认 0.05）。

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{Array, Float64Array, StringArray};
use datafusion::prelude::SessionContext;
use datalake::Datalake;

use mixer::data::{ChromData, UnivariateSufficient};
use mixer::fit::{FitConfig, fit1};
use mixer::weights::{RandpruneConfig, randprune_weights};

/// GWAS 表（全限定名，不含 `iceberg.` 前缀——SQL 里再拼）。
const GWAS_TABLE: &str = "gwas.bmi_ieu_a_2";

fn env_u32_list(key: &str, default: &[u32]) -> Vec<u32> {
    std::env::var(key)
        .ok()
        .map(|s| s.split(',').filter_map(|t| t.trim().parse().ok()).collect())
        .unwrap_or_else(|| default.to_vec())
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// 可选的位置窗口。`None` = 不限制。
fn pos_window() -> Option<(i64, i64)> {
    let lo = std::env::var("MIXER_E2E_POS_LO").ok()?.parse().ok()?;
    let hi = std::env::var("MIXER_E2E_POS_HI").ok()?.parse().ok()?;
    Some((lo, hi))
}

/// 从 RecordBatch 取 Float64 列。
fn col_f64<'a>(batch: &'a arrow_array::RecordBatch, name: &str) -> &'a Float64Array {
    batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("列 {name} 不存在"))
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap_or_else(|| panic!("列 {name} 不是 Float64"))
}

/// 从 RecordBatch 取 Utf8 列。
fn col_str<'a>(batch: &'a arrow_array::RecordBatch, name: &str) -> &'a StringArray {
    batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("列 {name} 不存在"))
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap_or_else(|| panic!("列 {name} 不是 Utf8"))
}

#[tokio::test]
#[ignore]
async fn e2e_fit1_on_iceberg_gwas() {
    let chroms = env_u32_list("MIXER_E2E_CHROMS", &[22]);
    let r2_min = env_f64("MIXER_E2E_R2_MIN", 0.05);
    let diffevo_repeats = env_usize("MIXER_E2E_DIFFEVO_REPEATS", 2);
    let window = pos_window();

    assert!(
        !chroms.is_empty(),
        "MIXER_E2E_CHROMS 至少指定一条染色体"
    );

    let dk = Arc::new(Datalake::new());
    let ctx: SessionContext = dk
        .get_ctx()
        .await
        .expect("无法连 Iceberg 数据湖（检查 ICEBERG_* 环境变量）");

    // -----------------------------------------------------------------
    // 逐染色体：GWAS ∩ AF → universe（rsid → 连续 index，附带 z/n/h）
    // -----------------------------------------------------------------
    let mut rsid_to_idx: HashMap<String, u32> = HashMap::new();
    let mut z_vec: Vec<f64> = Vec::new();
    let mut n_vec: Vec<f64> = Vec::new();
    let mut h_vec: Vec<f64> = Vec::new();
    let mut ld_triples: Vec<(u32, u32, f64)> = Vec::new();

    let pos_guard = match window {
        Some((lo, hi)) => format!("AND s.pos_col >= {lo} AND s.pos_col < {hi}"),
        None => String::new(),
    };

    for chrom in &chroms {
        // (a) GWAS ∩ AF：Z = β/SE，h = 2·f·(1−f)。
        //     af.eur_af.chrom 是 Int64；gwas.bmi_ieu_a_2.chrom 是 String。
        let universe_sql = format!(
            r#"SELECT a.id AS rsid, a.alt_freq AS af,
                      s.effect_size AS beta, s.std_error AS se, s.sample_size AS n
               FROM iceberg.af.eur_af AS a
               INNER JOIN iceberg.{gwas} AS s ON a.id = s.rsid
               WHERE a.chrom = {chrom}
                 AND s.chrom = '{chrom}'
                 {pos_guard}
                 AND s.effect_size IS NOT NULL
                 AND s.std_error IS NOT NULL
                 AND s.std_error > 0
                 AND a.alt_freq IS NOT NULL"#,
            gwas = GWAS_TABLE,
            chrom = chrom,
            pos_guard = pos_guard,
        );
        let batches = ctx
            .sql(&universe_sql)
            .await
            .expect("universe SQL 解析失败")
            .collect()
            .await
            .expect("universe 查询失败");

        for batch in &batches {
            let rsids = col_str(batch, "rsid");
            let afs = col_f64(batch, "af");
            let betas = col_f64(batch, "beta");
            let ses = col_f64(batch, "se");
            let ns = col_f64(batch, "n");
            for row in 0..batch.num_rows() {
                if rsids.is_null(row) {
                    continue;
                }
                let rsid = rsids.value(row);
                if rsid_to_idx.contains_key(rsid) {
                    continue; // 重复 rsid 保留首次 index
                }
                let se = ses.value(row);
                let beta = betas.value(row);
                if se == 0.0 || !beta.is_finite() {
                    continue;
                }
                let f = afs.value(row);
                let idx = rsid_to_idx.len() as u32;
                rsid_to_idx.insert(rsid.to_string(), idx);
                z_vec.push(beta / se);
                n_vec.push(ns.value(row));
                h_vec.push(2.0 * f * (1.0 - f));
            }
        }

        // (b) LD 矩阵（COO）：r² ≥ r2_min，两端点须在 universe 内。
        let ld_pos_guard = match window {
            Some((lo, hi)) => {
                format!("AND pos_a >= {lo} AND pos_a < {hi}")
            }
            None => String::new(),
        };
        let ld_sql = format!(
            "SELECT id_a, id_b, unphased_r2 FROM iceberg.ld_matrix.eur_chr{chrom} \
             WHERE unphased_r2 >= {r2_min} {ld_pos_guard}",
            chrom = chrom,
            r2_min = r2_min,
            ld_pos_guard = ld_pos_guard,
        );
        let ld_batches = ctx
            .sql(&ld_sql)
            .await
            .expect("ld SQL 解析失败")
            .collect()
            .await
            .expect("LD 查询失败");
        for batch in &ld_batches {
            let a_ids = col_str(batch, "id_a");
            let b_ids = col_str(batch, "id_b");
            let r2s = col_f64(batch, "unphased_r2");
            for row in 0..batch.num_rows() {
                if a_ids.is_null(row) || b_ids.is_null(row) {
                    continue;
                }
                let a = a_ids.value(row);
                let b = b_ids.value(row);
                if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
                    ld_triples.push((ta, tb, r2s.value(row)));
                }
            }
        }
    }

    let n_snp = z_vec.len();
    assert!(n_snp > 1000, "universe 过小（{n_snp}），检查窗口/染色体");
    assert!(!ld_triples.is_empty(), "无 LD 对，检查 r2_min/窗口");

    // -----------------------------------------------------------------
    // 组装 ChromData + randprune 权重（与生产节点一致）
    // -----------------------------------------------------------------
    let mut data = ChromData::new(z_vec, n_vec, h_vec, &ld_triples);
    let tags: Vec<u32> = (0..n_snp as u32).collect();
    let rp_cfg = RandpruneConfig {
        n: 64,
        r2_threshold: 0.1,
        use_w_ld: false,
        seed: 123,
    };
    data.weights = randprune_weights(&data.ld, n_snp, &tags, None, &rp_cfg);

    // -----------------------------------------------------------------
    // fit1：差分进化 × repeats → Nelder-Mead 精修
    // -----------------------------------------------------------------
    let cfg = FitConfig {
        diffevo_repeats,
        ..Default::default()
    };
    // 压缩成充分统计量：单趟扫 LD 折成 m1/m2，之后拟合不再触碰 CSR。
    let suff = UnivariateSufficient::from_chrom_data(&data);
    let result = fit1(&suff, &cfg);

    println!("=== MiXeR fit1 e2e（Iceberg GWAS = {gwas}）===", gwas = GWAS_TABLE);
    println!(
        "染色体 {chroms:?}  窗口 {win:?}  r2_min={r2_min}  diffevo_repeats={dr}",
        chroms = chroms,
        win = window,
        r2_min = r2_min,
        dr = diffevo_repeats,
    );
    println!(
        "SNP 数: {n_snp}  LD 对: {nld}  tag 数: {ntags}  sum_weights: {sw:.2}",
        n_snp = n_snp,
        nld = ld_triples.len(),
        ntags = data.tags.len(),
        sw = data.weights.iter().sum::<f64>(),
    );
    println!(
        "pi        = {:.6}\nsig2_beta = {:.6}\nsig2_zero = {:.6}\nh2        = {:.6}\n\
         nc        = {:.2}  nc@p9 = {:.2}\nAIC={:.2}  BIC={:.2}  cost={:.4}",
        result.params.pi,
        result.params.sig2_beta,
        result.params.sig2_zero,
        result.h2,
        result.nc,
        result.nc_p9,
        result.aic,
        result.bic,
        result.loglike,
    );

    // 基本合理性断言（真实 GWAS 的 BMI h² 通常 0.2~0.4 量级；这里只做宽松校验）
    assert!(result.params.pi > 0.0 && result.params.pi < 1.0, "pi 越界");
    assert!(result.params.sig2_beta > 0.0, "sig2_beta 非正");
    assert!(result.params.sig2_zero > 0.0, "sig2_zero 非正");
    assert!(result.h2.is_finite() && result.h2 >= 0.0, "h2 异常: {}", result.h2);
    assert!(result.aic.is_finite(), "AIC 非有限");
}
