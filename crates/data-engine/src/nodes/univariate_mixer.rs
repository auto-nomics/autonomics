//! Univariate MiXeR (`fit1`) transform node.
//!
//! 接收上游 GWAS 汇总统计 `DataFrame`（含 Z-score、样本量、rsid），从
//! Iceberg 数据湖读取 LD 矩阵（`ld_matrix.eur_chr{N}`）和 allele frequency
//! （`af.eur_af`），组装成 [`mixer::data::ChromData`]，调用
//! [`mixer::fit::fit1`] 拟合 spike-and-slab 模型，输出单行结果
//! `DataFrame`（pi, sig2_beta, sig2_zero, h2, nc, nc_p9, aic, bic, loglike）。

use std::sync::Arc;

use ahash::AHashMap;

use arrow_array::{Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodePorts};
use crate::{
    dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory},
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

/// rsid→全局 index 映射。LD 表的 id_a/id_b 是字符串，每行都要哈希查找两次；
/// 用 ahash（远快于 std 的 SipHash）替代 std HashMap。
type RsidMap = AHashMap<String, u32>;

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
    /// 随机种子（原版 `--seed`，默认 123；extract 使用）。
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

/// 预算面板的 Iceberg 表名（`iceberg.mixer` 命名空间下）。节点内部使用，
/// 不暴露给 spec / agent。由 `precompute_tags` 离线产出。
const TAGSUFF_TABLE: &str = "eur_tagsuff";

/// Univariate MiXeR 拟合节点。
///
/// 输入：上游 sumstats（Z, N, rsid）。从数据湖取 LD 矩阵与 AF，组装
/// [`mixer::data::ChromData`]，调用 [`mixer::fit::fit1`]，输出单行结果。
#[derive(Clone)]
pub struct UnivariateMixerNode {
    meta: NodePorts,
    spec: UnivariateMixerNodeSpec,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(input_schema()))
        .add_output_port(Some(output_schema()))
}

impl UnivariateMixerNode {
    pub fn new(spec: UnivariateMixerNodeSpec) -> Self {
        Self {
            meta: port_layout(),
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
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: UnivariateMixerNodeSpec = serde_json::from_value(spec)?;
        let node = UnivariateMixerNode::new(config);
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

    async fn execute(
        &mut self,
        node_ctx: &crate::node_registry::registry::NodeCtx,
        inputs: &[NodeInput],
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        // Phase-level + per-chromosome observations flow to the scheduler's
        // observers (the `run_dag` tool's live output) via `reporter`. Every
        // emission is `try_send` (fire-and-forget): a saturated channel drops
        // the line silently, so logging can never block or deadlock the node.
        use crate::dag::runtime::RuntimeStatus;
        let t0 = std::time::Instant::now();
        let n_chrom = self.spec.chromosomes.len();
        reporter.status(RuntimeStatus::Running);
        reporter.info(format!(
            "fit1: start (chromosomes={n_chrom}, r2_min={}, \
             extract={}, diffevo_repeats={})",
            self.spec.r2_min, self.spec.extract_enabled, self.spec.diffevo_repeats,
        ));

        let input = inputs.first().ok_or(UnivariateMixerError::InvalidInput(
            "no input DataFrame".into(),
        ))?;

        // 1. Build a fresh, isolated context per execution — no shared
        //    CatalogList, so concurrent nodes / re-runs never collide on
        //    `register_table`. Dropped at the end of this call.
        let ctx = node_ctx.session();

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
                let msg = format!(
                    "上游 sumstats 缺少必需列 '{needed}'；现有列: {avail:?}。\
                     MiXeR 需要 Z(浮点)、N(样本量)、rsid(SNP标识) 三列。"
                );
                reporter.error(format!("fit1: abort — {msg}"));
                return Err(UnivariateMixerError::InvalidInput(msg).into());
            }
        }
        ctx.register_table("sumstats", input.data.clone().into_view())
            .map_err(|e| UnivariateMixerError::Step {
                context: "register sumstats view".into(),
                detail: e.to_string(),
            })?;

        // 3. 读 sumstats → rsid → (Z, N)。不读 af.eur_af——h 和 maf 已在 tagsuff 里预算好。
        let mut rsid_to_idx: RsidMap = RsidMap::new();
        let mut z_vec: Vec<f64> = Vec::new();
        let mut n_vec: Vec<f64> = Vec::new();
        let sumstats_sql = format!(
            r#"SELECT "{rsid}" AS rsid, "{z}" AS zc, "{n}" AS nc FROM sumstats"#,
            rsid = INPUT_RSID_COL,
            z = INPUT_Z_COL,
            n = INPUT_N_COL,
        );
        let ss_df = UnivariateMixerError::df_ctx(
            ctx.sql(&sumstats_sql).await,
            "read sumstats",
            None,
            Some(&sumstats_sql),
        )?;
        let ss_batches =
            UnivariateMixerError::df_ctx(ss_df.collect().await, "collect sumstats", None, None)?;
        for batch in &ss_batches {
            let rsids = col_as_string(batch, "rsid")?;
            let zs = col_as_f64(batch, "zc")?;
            let ns = col_as_f64(batch, "nc")?;
            for row in 0..batch.num_rows() {
                let rsid = rsids.value(row);
                if rsid_to_idx.contains_key(rsid) {
                    continue;
                }
                rsid_to_idx.insert(rsid.to_string(), rsid_to_idx.len() as u32);
                z_vec.push(zs.value(row));
                n_vec.push(ns.value(row));
            }
        }
        let n_snp = z_vec.len();
        if n_snp == 0 {
            let msg = "sumstats 为空（0 SNPs）".to_string();
            reporter.error(format!("fit1: abort — {msg}"));
            return Err(UnivariateMixerError::InvalidInput(msg).into());
        }
        reporter.info(format!("sumstats: {n_snp} SNPs loaded (no af.eur_af read)"));

        // 4. 全面板 totalhet / n_snp_ref——单次聚合查询，sub-second。
        let chrom_list: Vec<String> = self
            .spec
            .chromosomes
            .iter()
            .map(|c| c.to_string())
            .collect();
        let panel_sql = format!(
            "SELECT SUM(2.0 * LEAST(alt_freq, 1.0 - alt_freq) * (1.0 - LEAST(alt_freq, 1.0 - alt_freq))) AS th, \
             COUNT(*) AS n FROM iceberg.af.eur_af WHERE chrom IN ({})",
            chrom_list.join(", ")
        );
        let panel_df = UnivariateMixerError::df_ctx(
            ctx.sql(&panel_sql).await,
            "panel totalhet",
            None,
            Some(&panel_sql),
        )?;
        let panel_batches =
            UnivariateMixerError::df_ctx(panel_df.collect().await, "collect totalhet", None, None)?;
        let totalhet = panel_batches
            .first()
            .and_then(|b| b.column_by_name("th"))
            .and_then(|c| {
                c.as_any()
                    .downcast_ref::<Float64Array>()
                    .map(|a| a.value(0))
            })
            .unwrap_or(0.0);
        let n_snp_ref = panel_batches
            .first()
            .and_then(|b| b.column_by_name("n"))
            .and_then(|c| {
                c.as_any()
                    .downcast_ref::<arrow_array::Int64Array>()
                    .map(|a| a.value(0) as usize)
            })
            .unwrap_or(n_snp);
        reporter.info(format!(
            "panel: {n_snp_ref} ref SNPs, totalhet={totalhet:.1}"
        ));

        // 5. 从预算面板表加载 per-tag 充分统计量（S1/S2/weight），逐元素乘 N。
        reporter.info("fold: loading precomputed per-tag sufficient stats");
        let suff_sql = format!("SELECT id_tag, s1, s2, weight FROM iceberg.mixer.{TAGSUFF_TABLE}");
        let suff_df = UnivariateMixerError::df_ctx(
            ctx.sql(&suff_sql).await,
            "load tagsuff table",
            None,
            Some(&suff_sql),
        )?;
        let suff_batches =
            UnivariateMixerError::df_ctx(suff_df.collect().await, "collect tagsuff", None, None)?;

        let mut m1 = vec![0.0f64; n_snp];
        let mut m2 = vec![0.0f64; n_snp];
        let mut weights = vec![0.0f64; n_snp];
        let mut tags: Vec<u32> = Vec::new();
        let mut panel_tag_count = 0u64;
        for batch in &suff_batches {
            let id_col = batch
                .column_by_name("id_tag")
                .ok_or_else(|| UnivariateMixerError::InvalidInput("col id_tag missing".into()))?
                .as_any()
                .downcast_ref::<arrow_array::StringArray>()
                .ok_or_else(|| UnivariateMixerError::InvalidInput("col id_tag not Utf8".into()))?;
            let s1_col = col_as_f64(batch, "s1")?;
            let s2_col = col_as_f64(batch, "s2")?;
            let wt_col = col_as_f64(batch, "weight")?;
            for r in 0..batch.num_rows() {
                panel_tag_count += 1;
                let rsid = id_col.value(r);
                if let Some(&idx) = rsid_to_idx.get(rsid) {
                    let n_tag = n_vec[idx as usize];
                    let s1 = s1_col.value(r);
                    let s2 = s2_col.value(r);
                    m1[idx as usize] = n_tag * s1;
                    m2[idx as usize] = n_tag * n_tag * s2;
                    weights[idx as usize] = wt_col.value(r);
                    tags.push(idx);
                }
            }
        }
        drop(n_vec);
        drop(rsid_to_idx);
        reporter.info(format!(
            "fold: {panel_tag_count} tags in panel, {} in universe — m1/m2/weights computed via N×S1 (no LD scan)",
            tags.len()
        ));
        if tags.is_empty() {
            reporter.warn("0 tags overlap universe — fit1 will be degenerate");
        }

        let suff = mixer::data::UnivariateSufficient {
            z: z_vec,
            weights,
            m1,
            m2,
            tags,
            totalhet,
            n_snp: n_snp_ref,
        };

        // 6. 跑 fit1（DE×repeats → Nelder-Mead 精修），只读充分统计量。
        let cfg = mixer::fit::FitConfig {
            diffevo_repeats: self.spec.diffevo_repeats,
            seed: self.spec.seed,
            ..Default::default()
        };
        // fit1 is a synchronous CPU-bound optimizer (DE×repeats → Nelder-Mead)
        // with no internal await/progress hooks — the channel will go quiet
        // until it returns. Bracket it so observers know *why* it's silent and
        // how long the dominant phase actually took.
        let fit_t0 = std::time::Instant::now();
        reporter.info(format!(
            "fit1: entering optimization (diffevo_repeats={}); no mid-phase progress, \
             this CPU-bound phase dominates runtime",
            self.spec.diffevo_repeats
        ));
        let result = mixer::fit::fit1(&suff, &cfg);
        reporter.info(format!(
            "fit1: optimization done in {:.2}s (total elapsed {:.2}s)",
            fit_t0.elapsed().as_secs_f64(),
            t0.elapsed().as_secs_f64()
        ));
        // Surface fitted params + flag degeneracies. In spike-and-slab only
        // (π, σ²β) are jointly identifiable while h² stays identifiable, so a π
        // pinned at 0/1 is the canonical instability signature — warn so the
        // observer doesn't chase a non-bug (cf. chr22 π=0.19→0.999 across seeds).
        reporter.info(format!(
            "fit1 result: pi={:.4} sig2_beta={:.4} sig2_zero={:.4} h2={:.4} \
             nc={:.0} nc_p9={:.0} loglike={:.2} aic={:.2} bic={:.2}",
            result.params.pi,
            result.params.sig2_beta,
            result.params.sig2_zero,
            result.h2,
            result.nc,
            result.nc_p9,
            result.loglike,
            result.aic,
            result.bic,
        ));
        if !(0.0..=1.0).contains(&result.h2) {
            reporter.warn(format!(
                "fit1: h2={:.4} outside [0,1] — possible M mismatch or overfitting",
                result.h2
            ));
        }
        if result.params.pi > 0.999 || result.params.pi < 1e-4 {
            reporter.warn(format!(
                "fit1: pi={:.4} at boundary — spike-and-slab identifiability degeneracy \
                 (π↔σ²β trade off; h² remains identifiable; reruns may differ)",
                result.params.pi
            ));
        }
        if !result.loglike.is_finite() || !result.aic.is_finite() || !result.bic.is_finite() {
            reporter.warn(format!(
                "fit1: non-finite goodness-of-fit (loglike={:?} aic={:?} bic={:?}) — \
                 optimizer may have diverged",
                result.loglike, result.aic, result.bic
            ));
        }

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
        reporter.info(format!(
            "fit1: finished in {:.2}s",
            t0.elapsed().as_secs_f64()
        ));
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

// /// 对一条 LD batch 的每个 pair（两端点都在 universe 内）调用 `emit(global_a, global_b, r2)`。
// ///
// /// 不做 r² 过滤、不做 base 偏移——由调用方在闭包里决定过滤阈值与方向（用于 LdScore
// /// 的双向折叠、extract 的对称邻接构建）。
// fn for_each_ld_pair(
//     batch: &RecordBatch,
//     rsid_to_idx: &RsidMap,
//     mut emit: impl FnMut(u32, u32, f64),
// ) -> Result<(), UnivariateMixerError> {
//     let a_ids = col_as_string(batch, "id_a")?;
//     let b_ids = col_as_string(batch, "id_b")?;
//     let r2s = col_as_f64(batch, "unphased_r2")?;
//     for row in 0..batch.num_rows() {
//         let a = a_ids.value(row);
//         let b = b_ids.value(row);
//         if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
//             emit(ta, tb, r2s.value(row));
//         }
//     }
//     Ok(())
// }
//
// /// 对一条 LD batch 施加与原版一致过滤（`r2 ≥ r2_min` 且两端点都在 universe
// /// 内），对每个存活项调用 `emit(local_tag, global_snp, r2)`。
// ///
// /// 计数遍与填值遍共用这一份代码路径，保证两遍看到完全相同的条目集合——
// /// `local_tag = global_tag - base`（本染色体行区间内的本地行号），
// /// `global_snp` 保留全局 index（直接写入 CSR 的 `column_index`）。
// fn for_each_ld_entry(
//     batch: &RecordBatch,
//     base: u32,
//     r2_min: f64,
//     rsid_to_idx: &RsidMap,
//     mut emit: impl FnMut(u32, u32, f64),
// ) -> Result<(), UnivariateMixerError> {
//     let a_ids = col_as_string(batch, "id_a")?;
//     let b_ids = col_as_string(batch, "id_b")?;
//     let r2s = col_as_f64(batch, "unphased_r2")?;
//     for row in 0..batch.num_rows() {
//         let r2 = r2s.value(row);
//         if r2 < r2_min {
//             continue;
//         }
//         let a = a_ids.value(row);
//         let b = b_ids.value(row);
//         if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
//             // ta 应属于本染色体区间 [base, base+n_k)；checked_sub 防御下溢。
//             if let Some(local_tag) = ta.checked_sub(base) {
//                 emit(local_tag, tb, r2);
//             }
//         }
//     }
//     Ok(())
// }
//
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
            seed: 123,
            extract_enabled: true,
            extract_maf: 0.05,
            extract_subset: 2_000_000,
            extract_r2: 0.8,
        };
        let node = UnivariateMixerNode::new(spec);
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
