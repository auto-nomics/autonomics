//! Univariate MiXeR (`fit1`) transform node.
//!
//! 接收上游 GWAS 汇总统计 `DataFrame`（含 Z-score、样本量、rsid），从
//! Iceberg 数据湖读取 LD 矩阵（`ld_matrix.eur_chr{N}`）和 allele frequency
//! （`af.eur_af`），组装成 [`mixer::data::ChromData`]，调用
//! [`mixer::fit::fit1`] 拟合 spike-and-slab 模型，输出单行结果
//! `DataFrame`（pi, sig2_beta, sig2_zero, h2, nc, nc_p9, aic, bic, loglike）。

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::prelude::SessionContext;
use futures::TryStreamExt;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory, new_isolated_ctx},
};

// =====================================================================
// Error type
// =====================================================================

#[derive(Debug, Error)]
pub enum UnivariateMixerError {
    /// 查询/执行失败，带"步骤 + 染色体 + SQL"上下文，便于定位。
    /// `context` 形如 `"universe (af ∩ sumstats) chr21"`；`detail` 是底层错误信息。
    #[error("univariate_mixer @ {context}: {detail}")]
    Step { context: String, detail: String },

    #[error("univariate_mixer invalid input: {0}")]
    InvalidInput(String),

    #[error("univariate_mixer arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),

    #[error("univariate_mixer datalake error: {0}")]
    Datalake(String),
}

impl UnivariateMixerError {
    /// 把一个 DataFusion Result 包上步骤上下文。
    fn df_ctx<T>(
        r: Result<T, datafusion::error::DataFusionError>,
        step: &str,
        chrom: Option<u32>,
        sql: Option<&str>,
    ) -> Result<T, Self> {
        r.map_err(|e| {
            let mut context = match chrom {
                Some(c) => format!("{step} (chr{c})"),
                None => step.to_string(),
            };
            if let Some(sql) = sql {
                // 截断长 SQL，只保留便于诊断的前缀
                let snip = if sql.len() > 400 {
                    format!("{}…", &sql[..400])
                } else {
                    sql.to_string()
                };
                context.push_str(&format!("\n  SQL: {snip}"));
            }
            Self::Step {
                context,
                detail: e.to_string(),
            }
        })
    }
}

impl From<UnivariateMixerError> for DagError {
    fn from(e: UnivariateMixerError) -> Self {
        DagError::NodeError {
            node_type: "univariate_mixer".to_string(),
            msg: e.to_string(),
        }
    }
}

impl From<datalake::error::Error> for UnivariateMixerError {
    fn from(e: datalake::error::Error) -> Self {
        UnivariateMixerError::Datalake(e.to_string())
    }
}

// =====================================================================
// Schemas
// =====================================================================

/// 上游 GWAS sumstats 固定列名：Z-score、样本量、rsid 连接键。
const INPUT_Z_COL: &str = "Z";
const INPUT_N_COL: &str = "N";
const INPUT_RSID_COL: &str = "rsid";

/// 输入端口 schema：与 ldsc_hsq 一致，便于复用同一份上游 sumstats。
fn input_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(INPUT_Z_COL, DataType::Float64, true),
        Field::new(INPUT_N_COL, DataType::Float64, true),
        Field::new(INPUT_RSID_COL, DataType::Utf8, false),
    ]))
}

/// 输出端口 schema：单行 MiXeR fit1 结果，字段对齐
/// [`mixer::result::FitResult`]。同时作为端口声明（供 DAG 校验下游边）
/// 与 [`build_result_batch`] 的单一真相源。
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("pi", DataType::Float64, false),
        Field::new("sig2_beta", DataType::Float64, false),
        Field::new("sig2_zero", DataType::Float64, false),
        Field::new("h2", DataType::Float64, false),
        Field::new("nc", DataType::Float64, false),
        Field::new("nc_p9", DataType::Float64, false),
        Field::new("aic", DataType::Float64, false),
        Field::new("bic", DataType::Float64, false),
        Field::new("loglike", DataType::Float64, false),
    ]))
}

/// 把 [`mixer::result::FitResult`] 打包成单行 `RecordBatch`。
fn build_result_batch(r: &mixer::result::FitResult) -> Result<RecordBatch, UnivariateMixerError> {
    let schema = output_schema();
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![r.params.pi])),
            Arc::new(Float64Array::from(vec![r.params.sig2_beta])),
            Arc::new(Float64Array::from(vec![r.params.sig2_zero])),
            Arc::new(Float64Array::from(vec![r.h2])),
            Arc::new(Float64Array::from(vec![r.nc])),
            Arc::new(Float64Array::from(vec![r.nc_p9])),
            Arc::new(Float64Array::from(vec![r.aic])),
            Arc::new(Float64Array::from(vec![r.bic])),
            Arc::new(Float64Array::from(vec![r.loglike])),
        ],
    )?;
    Ok(batch)
}

// =====================================================================
// Config / Spec
// =====================================================================

/// cost 加权方案。
///
/// - `LdScore`：逆 LD-score 加权 `1/(1+Σr²)`。单趟顺序扫 LD 即可，**不需要 CSR**，
///   节点按染色体流式装配（读一条、累加 m1/m2+Σr²、丢一条），峰值内存 = 单条
///   染色体的一个 LD batch。与原版 randprune 统计同向但**不逐位一致**。
/// - `Randprune`：原版随机剪枝（多轮贪心独立集）。需要按 tag 随机访问 LD，
///   故仍建逐染色体 CSR，但通过块对角视图 [`mixer::ld_matrix::BlockDiagonal`]
///   直接喂给 randprune，**不再 `merge_blocks`**（峰值 ~2× nnz → ~1× nnz）。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WeightingMode {
    #[default]
    LdScore,
    Randprune,
}

/// Univariate MiXeR 节点配置（DAG spec）。
///
/// 选择要参与拟合的染色体、数据湖表名，以及拟合超参数。
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UnivariateMixerNodeSpec {
    /// 参与拟合的染色体列表，如 `[21, 22]`。每个染色体对应一张
    /// `iceberg.ld_matrix.eur_chr{chrom}` 表（当前固定 EUR 人群）。
    pub chromosomes: Vec<u32>,

    /// 差分进化重复次数（原版 `--diffevo-fast-repeats`，默认 20）。
    #[serde(default = "default_diffevo_repeats")]
    pub diffevo_repeats: usize,
    /// r² 阈值：低于此值的 LD 对忽略（首版固定 sig2_zeroL=0，此字段预留）。
    #[serde(default = "default_r2_min")]
    pub r2_min: f64,
    /// cost 加权方案（默认 `LdScore`，内存友好的流式路径）。
    #[serde(default)]
    pub weighting: WeightingMode,
    /// 随机剪枝轮数（原版 `--randprune-n`，默认 64；仅 `Randprune` 模式使用）。
    #[serde(default = "default_randprune_n")]
    pub randprune_n: u32,
    /// 随机剪枝 r² 阈值（原版 `--randprune-r2`，默认 0.1；仅 `Randprune` 模式使用）。
    #[serde(default = "default_randprune_r2")]
    pub randprune_r2: f64,
    /// 随机种子（原版 `--seed`，默认 123；randprune 与 extract 共用）。
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// 是否启用 extract（tag 子集化）。开启后只在 ~`extract_subset` 个近条件独立
    /// 的 tag SNP 上拟合（MAF≥`extract_maf` + 贪心 LD 剪枝 r²>`extract_r2` + 随机子集），
    /// LD 邻居仍来自全面板。关闭则退回 tags=全集（旧行为）。
    #[serde(default = "default_extract_enabled")]
    pub extract_enabled: bool,
    /// extract 的 MAF 下限（原版 `--maf`，默认 0.05）。
    #[serde(default = "default_extract_maf")]
    pub extract_maf: f64,
    /// extract 的随机子集上限（原版 `--subset`，默认 2_000_000）。
    #[serde(default = "default_extract_subset")]
    pub extract_subset: usize,
    /// extract 的 LD 剪枝阈值（原版 `--r2`，默认 0.8；严格 > 才剪）。
    #[serde(default = "default_extract_r2")]
    pub extract_r2: f64,
}

fn default_diffevo_repeats() -> usize {
    20
}
fn default_r2_min() -> f64 {
    0.05
}
fn default_randprune_n() -> u32 {
    64
}
fn default_randprune_r2() -> f64 {
    0.1
}
fn default_seed() -> u64 {
    123
}
fn default_extract_enabled() -> bool {
    true
}
fn default_extract_maf() -> f64 {
    0.05
}
fn default_extract_subset() -> usize {
    2_000_000
}
fn default_extract_r2() -> f64 {
    0.8
}

// =====================================================================
// Node
// =====================================================================

const UNIVARIATE_MIXER_NODE_KIND: &str = "univariate_mixer";

/// Univariate MiXeR 拟合节点。
///
/// 输入：上游 sumstats（Z, N, rsid）。从数据湖取 LD 矩阵与 AF，组装
/// [`mixer::data::ChromData`]，调用 [`mixer::fit::fit1`]，输出单行结果。
#[derive(Clone)]
pub struct UnivariateMixerNode {
    meta: NodePorts,
    ctx: SessionContext,
    spec: UnivariateMixerNodeSpec,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema()))
        .add_output_port(Some(output_schema()))
}

impl UnivariateMixerNode {
    pub fn new(ctx: SessionContext, spec: UnivariateMixerNodeSpec) -> Self {
        Self {
            meta: port_layout(),
            ctx,
            spec,
        }
    }
}

pub struct UnivariateMixerNodeFactory {}

impl NodeFactory for UnivariateMixerNodeFactory {
    fn kind(&self) -> &'static str {
        UNIVARIATE_MIXER_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Fits univariate MiXeR spike-and-slab (fit1) on a single GWAS trait."
    }

    fn doc(&self) -> &'static str {
        "Univariate MiXeR (fit1) transform node. Takes a single upstream GWAS \
        summary statistics DataFrame (with Z, N, rsid columns), queries the \
        Iceberg data lake for the LD matrix (`ld_matrix.eur_chr{N}`) and \
        allele frequency (`af.eur_af`), assembles a `ChromData` and fits \
        `mixer::fit::fit1`. Outputs a single-row result DataFrame with the \
        fitted parameters (pi, sig2_beta, sig2_zero) and derived quantities \
        (h2, nc, nc_p9, aic, bic, loglike). One typed input port; one typed \
        output port."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(UnivariateMixerNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: UnivariateMixerNodeSpec = serde_json::from_value(spec)?;
        let ctx = new_isolated_ctx(node_ctx.runtime_env, node_ctx.iceberg_catalog);
        let node = UnivariateMixerNode::new(ctx, config);
        Ok(Box::new(node))
    }
}

#[async_trait]
impl DagNode for UnivariateMixerNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        UNIVARIATE_MIXER_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&mut self, inputs: &[NodeInput]) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(UnivariateMixerError::InvalidInput(
            "no input DataFrame".into(),
        ))?;

        // 1. Build a fresh, isolated context per execution — no shared CatalogList,
        let ctx = &self.ctx;

        // 2. 把上游 sumstats 注册为临时表，并**提前校验必需列**（Z/N/rsid）——
        //    缺列时给清晰提示，而不是让后面的 SQL 抛出晦涩错误。
        let in_schema = input.data.schema();
        let avail: Vec<&str> = in_schema
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect();
        for needed in [INPUT_Z_COL, INPUT_N_COL, INPUT_RSID_COL] {
            if !in_schema.fields().iter().any(|f| f.name() == needed) {
                return Err(UnivariateMixerError::InvalidInput(format!(
                    "上游 sumstats 缺少必需列 '{needed}'；现有列: {avail:?}。\
                     MiXeR 需要 Z(浮点)、N(样本量)、rsid(SNP标识) 三列。"
                ))
                .into());
            }
        }
        ctx.register_table("sumstats", input.data.clone().into_view())
            .map_err(|e| UnivariateMixerError::Step {
                context: "register sumstats view".into(),
                detail: e.to_string(),
            })?;

        // 3. 读 universe（sumstats ∩ AF）→ 每个 SNP 的 z/n/h/maf。
        //    所有 SNP 合并进一个连续全局 index 空间；LD 不跨染色体，块对角。
        let mut rsid_to_idx: HashMap<String, u32> = HashMap::new();
        let mut z_vec: Vec<f64> = Vec::new();
        let mut n_vec: Vec<f64> = Vec::new();
        let mut h_vec: Vec<f64> = Vec::new();
        let mut maf_vec: Vec<f64> = Vec::new();
        let mut chrom_base: Vec<u32> = Vec::new(); // 每条染色体在全局空间的起点
        for chrom in &self.spec.chromosomes {
            chrom_base.push(rsid_to_idx.len() as u32);
            let universe_sql = format!(
                r#"SELECT a.id AS rsid, a.alt_freq AS af, s."{z}" AS zc, s."{n}" AS nc
                   FROM iceberg.af.eur_af AS a
                   INNER JOIN sumstats AS s ON a.id = s."{rsid}"
                   WHERE a.chrom = {chrom}"#,
                z = INPUT_Z_COL,
                n = INPUT_N_COL,
                rsid = INPUT_RSID_COL,
                chrom = chrom,
            );
            let df = UnivariateMixerError::df_ctx(
                ctx.sql(&universe_sql).await,
                "universe (af ∩ sumstats)",
                Some(*chrom),
                Some(&universe_sql),
            )?;
            let universe_batches = UnivariateMixerError::df_ctx(
                df.collect().await,
                "collect universe batches",
                Some(*chrom),
                None,
            )?;
            for batch in &universe_batches {
                let rsids = col_as_string(batch, "rsid")?;
                let afs = col_as_f64(batch, "af")?;
                let zs = col_as_f64(batch, "zc")?;
                let ns = col_as_f64(batch, "nc")?;
                for row in 0..batch.num_rows() {
                    let rsid = rsids.value(row);
                    if rsid_to_idx.contains_key(rsid) {
                        continue;
                    }
                    let f = afs.value(row);
                    rsid_to_idx.insert(rsid.to_string(), rsid_to_idx.len() as u32);
                    z_vec.push(zs.value(row));
                    n_vec.push(ns.value(row));
                    let maf = f.min(1.0 - f);
                    h_vec.push(2.0 * maf * (1.0 - maf)); // 杂合度
                    maf_vec.push(maf);
                }
            }
        }
        let n_snp = z_vec.len();
        if n_snp == 0 {
            return Err(UnivariateMixerError::InvalidInput(format!(
                "no SNPs overlap between sumstats and af.eur_af for chromosomes {:?}",
                self.spec.chromosomes
            ))
            .into());
        }
        let totalhet: f64 = h_vec.iter().sum();

        // 4. extract：选 tag 子集（MAF≥maf_min + 贪心 LD 剪枝 r²>r2_threshold + 随机 subset）。
        //    邻接只取 r²>extract_r2 的对（少），按染色体建**对称化** CSR（双向），喂 select_tags。
        let tags: Vec<u32> = if self.spec.extract_enabled {
            let mut adj_blocks: Vec<(usize, mixer::ld_matrix::LdBlock)> = Vec::new();
            for (ci, chrom) in self.spec.chromosomes.iter().enumerate() {
                let base = chrom_base[ci] as usize;
                let n_k = (if ci + 1 < chrom_base.len() {
                    chrom_base[ci + 1] as usize
                } else {
                    n_snp
                }) - base;
                if n_k == 0 {
                    continue;
                }
                let adj_sql = format!(
                    "SELECT id_a, id_b, unphased_r2 FROM iceberg.ld_matrix.eur_chr{chrom} WHERE unphased_r2 > {r2}",
                    chrom = chrom,
                    r2 = self.spec.extract_r2,
                );
                let mut adj_triples: Vec<(u32, u32, f64)> = Vec::new();
                let df = UnivariateMixerError::df_ctx(
                    ctx.sql(&adj_sql).await,
                    "extract adjacency (ld r²>thr)",
                    Some(*chrom),
                    Some(&adj_sql),
                )?;
                let mut stream = UnivariateMixerError::df_ctx(
                    df.execute_stream().await,
                    "extract adjacency stream",
                    Some(*chrom),
                    None,
                )?;
                while let Some(batch) = UnivariateMixerError::df_ctx(
                    stream.try_next().await,
                    "extract adjacency batch",
                    Some(*chrom),
                    None,
                )? {
                    for_each_ld_pair(&batch, &rsid_to_idx, |a, b, _r2| {
                        // 对称化：row(a) 加 b，row(b) 加 a，保证 select_tags 的 ld.row(idx) 完整
                        adj_triples.push(((a as usize - base) as u32, b, 1.0));
                        adj_triples.push(((b as usize - base) as u32, a, 1.0));
                    })?;
                }
                adj_blocks.push((base, mixer::ld_matrix::LdBlock::from_coo(&adj_triples, n_k)));
            }
            let adj = mixer::ld_matrix::BlockDiagonal::new(adj_blocks);
            let ec = mixer::extract::ExtractConfig {
                maf_min: self.spec.extract_maf,
                r2_threshold: self.spec.extract_r2,
                subset: self.spec.extract_subset,
                seed: self.spec.seed,
            };
            mixer::extract::select_tags(&maf_vec, &adj, &ec)
        } else {
            (0..n_snp as u32).collect()
        };
        let tag_set: std::collections::HashSet<u32> = tags.iter().copied().collect();

        // 5. 折 LD（r²≥r2_min）进充分统计量。
        let is_ldscore = self.spec.weighting == WeightingMode::LdScore;
        let suff = if is_ldscore {
            // LdScore（默认，内存友好）：流式读 LD，**双向、tag 过滤**折 m1/m2。
            // 双向是为修正非对称 LD（plink2 每对存一份）下漏算 id_b 的 bug；
            // tag 过滤只折 tag 端点（邻居仍可是非 tag 的 panel SNP，h[b] 照常查）。
            let mut m1 = vec![0.0f64; n_snp];
            let mut m2 = vec![0.0f64; n_snp];
            let mut sum_r2 = vec![0.0f64; n_snp];
            for chrom in &self.spec.chromosomes {
                let ld_sql = format!(
                    "SELECT id_a, id_b, unphased_r2 FROM iceberg.ld_matrix.eur_chr{chrom} WHERE unphased_r2 >= {r2}",
                    chrom = chrom,
                    r2 = self.spec.r2_min,
                );
                let df = UnivariateMixerError::df_ctx(
                    ctx.sql(&ld_sql).await,
                    "LdScore fold (ld r²≥r2min)",
                    Some(*chrom),
                    Some(&ld_sql),
                )?;
                let mut stream = UnivariateMixerError::df_ctx(
                    df.execute_stream().await,
                    "LdScore fold stream",
                    Some(*chrom),
                    None,
                )?;
                while let Some(batch) = UnivariateMixerError::df_ctx(
                    stream.try_next().await,
                    "LdScore fold batch",
                    Some(*chrom),
                    None,
                )? {
                    for_each_ld_pair(&batch, &rsid_to_idx, |a, b, r2| {
                        if r2 < 0.0 {
                            return;
                        }
                        if tag_set.contains(&a) {
                            let ai = a as usize;
                            let a2 = n_vec[ai] * h_vec[b as usize] * r2;
                            m1[ai] += a2;
                            m2[ai] += a2 * a2;
                            sum_r2[ai] += r2;
                        }
                        if tag_set.contains(&b) {
                            let bi = b as usize;
                            let a2 = n_vec[bi] * h_vec[a as usize] * r2;
                            m1[bi] += a2;
                            m2[bi] += a2 * a2;
                            sum_r2[bi] += r2;
                        }
                    })?;
                }
            }
            let weights: Vec<f64> = sum_r2
                .iter()
                .map(|&s| mixer::weights::ldscore_weight(s))
                .collect();
            drop(n_vec);
            drop(h_vec);
            mixer::data::UnivariateSufficient {
                z: z_vec,
                weights,
                m1,
                m2,
                tags,
                totalhet,
                n_snp,
            }
        } else {
            // Randprune：collect 全量 LD 建 CSR（适合对称 LD；非对称数据建议用 LdScore）。
            let mut chrom_blocks: Vec<(usize, mixer::ld_matrix::LdBlock)> = Vec::new();
            for (ci, chrom) in self.spec.chromosomes.iter().enumerate() {
                let base = chrom_base[ci];
                let n_k = ((if ci + 1 < chrom_base.len() {
                    chrom_base[ci + 1]
                } else {
                    n_snp as u32
                }) - base) as usize;
                if n_k == 0 {
                    continue;
                }
                let ld_sql = format!(
                    "SELECT id_a, id_b, unphased_r2 FROM iceberg.ld_matrix.eur_chr{chrom}",
                    chrom = chrom,
                );
                let df = UnivariateMixerError::df_ctx(
                    ctx.sql(&ld_sql).await,
                    "randprune LD (full)",
                    Some(*chrom),
                    Some(&ld_sql),
                )?;
                let ld_batches = UnivariateMixerError::df_ctx(
                    df.collect().await,
                    "collect randprune LD batches",
                    Some(*chrom),
                    None,
                )?;
                let mut row_counts = vec![0u32; n_k];
                for batch in &ld_batches {
                    for_each_ld_entry(batch, base, self.spec.r2_min, &rsid_to_idx, |lt, _, _| {
                        row_counts[lt as usize] += 1;
                    })?;
                }
                let mut row_ptr = vec![0u32; n_k + 1];
                for i in 0..n_k {
                    row_ptr[i + 1] = row_ptr[i] + row_counts[i];
                }
                let nnz_k = row_ptr[n_k] as usize;
                let mut column_index = vec![0u32; nnz_k];
                let mut r2_store = vec![0.0f32; nnz_k];
                let mut cursor = row_ptr.clone();
                for batch in &ld_batches {
                    for_each_ld_entry(
                        batch,
                        base,
                        self.spec.r2_min,
                        &rsid_to_idx,
                        |lt, gs, r2| {
                            let p = cursor[lt as usize] as usize;
                            column_index[p] = gs;
                            r2_store[p] = r2 as f32;
                            cursor[lt as usize] += 1;
                        },
                    )?;
                }
                chrom_blocks.push((
                    base as usize,
                    mixer::ld_matrix::LdBlock {
                        n_tag: n_k,
                        row_ptr,
                        column_index,
                        r2: r2_store,
                    },
                ));
            }
            let view = mixer::ld_matrix::BlockDiagonal::new(std::mem::take(&mut chrom_blocks));
            let rp_cfg = mixer::weights::RandpruneConfig {
                n: self.spec.randprune_n,
                r2_threshold: self.spec.randprune_r2,
                use_w_ld: false,
                seed: self.spec.seed,
            };
            let weights = mixer::weights::randprune_weights(&view, n_snp, &tags, None, &rp_cfg);
            let suff = mixer::data::UnivariateSufficient::from_ld(
                &view, &n_vec, &h_vec, z_vec, weights, tags,
            );
            drop(view);
            drop(n_vec);
            drop(h_vec);
            suff
        };
        drop(rsid_to_idx);

        // 6. 跑 fit1（DE×repeats → Nelder-Mead 精修），只读充分统计量。
        let cfg = mixer::fit::FitConfig {
            diffevo_repeats: self.spec.diffevo_repeats,
            ..Default::default()
        };
        let result = mixer::fit::fit1(&suff, &cfg);

        // 7. 打包单行结果 RecordBatch 并返回。
        let batch = build_result_batch(&result)?;
        let df = UnivariateMixerError::df_ctx(
            ctx.read_batch(batch),
            "read result batch into DataFrame",
            None,
            None,
        )?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// Arrow 列提取 helpers
// =====================================================================

/// 按列名取 `StringArray`（rsid 等 Utf8 列）。
fn col_as_string<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a StringArray, UnivariateMixerError> {
    batch
        .column_by_name(name)
        .ok_or_else(|| UnivariateMixerError::InvalidInput(format!("column '{name}' not found")))?
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| UnivariateMixerError::InvalidInput(format!("column '{name}' is not Utf8")))
}

/// 按列名取 `Float64Array`（Z/N/alt_freq/unphased_r2 等）。
///
/// 要求上游 schema 为 Float64（AF/LD 表与输入端口 schema 均为 Float64）。
fn col_as_f64<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a Float64Array, UnivariateMixerError> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| UnivariateMixerError::InvalidInput(format!("column '{name}' not found")))?;
    col.as_any().downcast_ref::<Float64Array>().ok_or_else(|| {
        UnivariateMixerError::InvalidInput(format!(
            "column '{name}' is not Float64 (got {})",
            col.data_type()
        ))
    })
}

/// 对一条 LD batch 的每个 pair（两端点都在 universe 内）调用 `emit(global_a, global_b, r2)`。
///
/// 不做 r² 过滤、不做 base 偏移——由调用方在闭包里决定过滤阈值与方向（用于 LdScore
/// 的双向折叠、extract 的对称邻接构建）。
fn for_each_ld_pair(
    batch: &RecordBatch,
    rsid_to_idx: &HashMap<String, u32>,
    mut emit: impl FnMut(u32, u32, f64),
) -> Result<(), UnivariateMixerError> {
    let a_ids = col_as_string(batch, "id_a")?;
    let b_ids = col_as_string(batch, "id_b")?;
    let r2s = col_as_f64(batch, "unphased_r2")?;
    for row in 0..batch.num_rows() {
        let a = a_ids.value(row);
        let b = b_ids.value(row);
        if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
            emit(ta, tb, r2s.value(row));
        }
    }
    Ok(())
}

/// 对一条 LD batch 施加与原版一致过滤（`r2 ≥ r2_min` 且两端点都在 universe
/// 内），对每个存活项调用 `emit(local_tag, global_snp, r2)`。
///
/// 计数遍与填值遍共用这一份代码路径，保证两遍看到完全相同的条目集合——
/// `local_tag = global_tag - base`（本染色体行区间内的本地行号），
/// `global_snp` 保留全局 index（直接写入 CSR 的 `column_index`）。
fn for_each_ld_entry(
    batch: &RecordBatch,
    base: u32,
    r2_min: f64,
    rsid_to_idx: &HashMap<String, u32>,
    mut emit: impl FnMut(u32, u32, f64),
) -> Result<(), UnivariateMixerError> {
    let a_ids = col_as_string(batch, "id_a")?;
    let b_ids = col_as_string(batch, "id_b")?;
    let r2s = col_as_f64(batch, "unphased_r2")?;
    for row in 0..batch.num_rows() {
        let r2 = r2s.value(row);
        if r2 < r2_min {
            continue;
        }
        let a = a_ids.value(row);
        let b = b_ids.value(row);
        if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
            // ta 应属于本染色体区间 [base, base+n_k)；checked_sub 防御下溢。
            if let Some(local_tag) = ta.checked_sub(base) {
                emit(local_tag, tb, r2);
            }
        }
    }
    Ok(())
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_type_is_stable() {
        assert_eq!(UNIVARIATE_MIXER_NODE_KIND, "univariate_mixer");
    }

    #[tokio::test]
    async fn construct_node_with_spec() {
        // 仅验证节点能按 spec 构造、端口 schema 正确。
        let spec = UnivariateMixerNodeSpec {
            chromosomes: vec![21, 22],
            diffevo_repeats: 5,
            r2_min: 0.05,
            weighting: WeightingMode::LdScore,
            randprune_n: 64,
            randprune_r2: 0.1,
            seed: 123,
            extract_enabled: true,
            extract_maf: 0.05,
            extract_subset: 2_000_000,
            extract_r2: 0.8,
        };
        let node = UnivariateMixerNode::new(SessionContext::new(), spec);
        assert_eq!(node.kind(), "univariate_mixer");
        // 一个输入端口（sumstats）、一个输出端口（fit1 结果）
        assert_eq!(node.ports().input_ports().len(), 1);
        assert_eq!(node.ports().output_ports().len(), 1);
    }
    //
    // #[tokio::test]
    // async fn load_iceberg_ld_matrix_panel() {
    //     let dk = Datalake::default();
    //     let ctx = dk.get_ctx().await.unwrap();
    //     let ld_df_chr22 = ctx.sql("SELECT * FROM iceberg.af.eur_af").await.unwrap();
    //     ld_df_chr22
    //         .limit(0, Some(10))
    //         .unwrap()
    //         .show()
    //         .await
    //         .unwrap();
    //     panic!()
    // }
}
