//! Bivariate MiXeR (`fit2`) transform node.
//!
//! 联合两个 GWAS trait 估计共享/特异 causal 变异与遗传相关。
//!
//! 输入（4 个端口）：
//! - port 0：trait1 sumstats（Z, N, rsid）
//! - port 1：trait2 sumstats（Z, N, rsid）
//! - port 2：trait1 的 univariate fit1 结果（pi, sig2_beta, sig2_zero，单行）
//! - port 3：trait2 的 univariate fit1 结果（pi, sig2_beta, sig2_zero，单行）
//!
//! 从 Iceberg 数据湖读 LD（`ld_matrix.eur_chr{N}`）与 AF（`af.eur_af`），组装
//! [`mixer::bivariate::BivariateData`]，固定两个 univariate 约束，调用
//! [`mixer::bivariate::fit2`] 拟合，输出单行 bivariate 结果。
//!
//! 加权：节点用 Rust 原生 randprune（`mixer::weights::randprune_weights`，与原版
//! `set_weights_randprune` bit-exact，见记忆 `mixer-ld-dump-workflow`）。tag 集 = 全部 SNP。
//! cost/optimizer 已交叉验证。
//!
//! 内存注记：与 univariate 不同，bivariate 的 CSR **无法**经充分统计量压缩消除——
//! 生产用的 sampling cost 逐邻居做 Monte Carlo 多项采样，内在需要 per-neighbor 的
//! `h·r²` 全表（识别 pi12/rho_beta 的高阶信息在其中），不可折叠成标量。故 CSR 是
//! sampling cost 钉死的常驻对象，换 ldscore 加权也消不掉它；bivariate 的内存优化
//! 走 Tier 1（`BlockDiagonal` 视图，消全局 COO 缓冲 + merge 复制），而非加权替换。

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::prelude::DataFrame;
use futures::TryStreamExt;
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
pub enum BivariateMixerError {
    /// 查询/执行失败，带"步骤 + 染色体 + SQL"上下文，便于定位。
    #[error("bivariate_mixer @ {context}: {detail}")]
    Step { context: String, detail: String },

    #[error("bivariate_mixer invalid input: {0}")]
    InvalidInput(String),

    #[error("bivariate_mixer arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),

    #[error("bivariate_mixer datalake error: {0}")]
    Datalake(String),
}

impl BivariateMixerError {
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

impl From<BivariateMixerError> for DagError {
    fn from(e: BivariateMixerError) -> Self {
        DagError::NodeError {
            node_type: "bivariate_mixer".to_string(),
            msg: e.to_string(),
        }
    }
}

impl From<datalake::error::Error> for BivariateMixerError {
    fn from(e: datalake::error::Error) -> Self {
        BivariateMixerError::Datalake(e.to_string())
    }
}

// =====================================================================
// Schemas
// =====================================================================

const INPUT_Z_COL: &str = "Z";
const INPUT_N_COL: &str = "N";
const INPUT_RSID_COL: &str = "rsid";
const PARAM_PI: &str = "pi";
const PARAM_SB: &str = "sig2_beta";
const PARAM_SZ: &str = "sig2_zero";

/// sumstats 输入端口 schema（trait1、trait2 共用）。
fn sumstats_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(INPUT_Z_COL, DataType::Float64, true),
        Field::new(INPUT_N_COL, DataType::Float64, true),
        Field::new(INPUT_RSID_COL, DataType::Utf8, false),
    ]))
}

/// fit1 结果端口 schema（取 pi/sig2_beta/sig2_zero 三列）。
fn fit1_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(PARAM_PI, DataType::Float64, false),
        Field::new(PARAM_SB, DataType::Float64, false),
        Field::new(PARAM_SZ, DataType::Float64, false),
    ]))
}

/// 输出端口 schema：单行 bivariate fit2 结果。
fn output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("pi1", DataType::Float64, false),
        Field::new("pi2", DataType::Float64, false),
        Field::new("pi12", DataType::Float64, false),
        Field::new("rho_beta", DataType::Float64, false),
        Field::new("rho_zero", DataType::Float64, false),
        Field::new("rg", DataType::Float64, false),
        Field::new("dice", DataType::Float64, false),
        Field::new("h2_t1", DataType::Float64, false),
        Field::new("h2_t2", DataType::Float64, false),
        Field::new("loglike", DataType::Float64, false),
    ]))
}

fn build_result_batch(
    r: &mixer::bivariate::BivariateFitResult,
) -> Result<RecordBatch, BivariateMixerError> {
    let schema = output_schema();
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![r.pi1])),
            Arc::new(Float64Array::from(vec![r.pi2])),
            Arc::new(Float64Array::from(vec![r.pi12])),
            Arc::new(Float64Array::from(vec![r.rho_beta])),
            Arc::new(Float64Array::from(vec![r.rho_zero])),
            Arc::new(Float64Array::from(vec![r.rg])),
            Arc::new(Float64Array::from(vec![r.dice])),
            Arc::new(Float64Array::from(vec![r.h2_t1])),
            Arc::new(Float64Array::from(vec![r.h2_t2])),
            Arc::new(Float64Array::from(vec![r.loglike])),
        ],
    )?;
    Ok(batch)
}

// =====================================================================
// Config / Spec
// =====================================================================

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BivariateMixerNodeSpec {
    /// 参与拟合的染色体列表，如 `[21, 22]`。
    pub chromosomes: Vec<u32>,
    /// 差分进化重复次数（原版 `--diffevo-fast-repeats`，默认 20）。
    #[serde(default = "default_diffevo_repeats")]
    pub diffevo_repeats: usize,
    /// r² 阈值：低于此值的 LD 对忽略（首版固定 sig2_zeroL=0）。
    #[serde(default = "default_r2_min")]
    pub r2_min: f64,
    /// brute1/brent1 是否用 sampling cost（true=打破 pi12/rho_beta 退化，得到可识别的遗传重叠）。
    #[serde(default = "default_sampling")]
    pub sampling: bool,
    /// sampling cost 的 MC 配置数（原版 `--kmax`，默认 20000）。
    #[serde(default = "default_k_max")]
    pub k_max: usize,
    /// 随机剪枝轮数（原版 `--randprune-n`，默认 64）。
    #[serde(default = "default_randprune_n")]
    pub randprune_n: u32,
    /// 随机剪枝 r² 阈值（原版 `--randprune-r2`，默认 0.1）。
    #[serde(default = "default_randprune_r2")]
    pub randprune_r2: f64,
    /// 随机种子（原版 `--seed`，默认 123；randprune 与 extract 共用）。
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// 是否启用 extract（tag 子集化）。开启后只在 ~`extract_subset` 个 tag 上拟合，
    /// LD CSR 也只存 tag 相关对（tag×panel，~2.4GB 而非全量 ~12GB）。关闭则 tags=全集。
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
    /// tag 诱导 LD 子图的 Iceberg 表名。默认 `"eur_subgraph"`。
    /// 设了跳过 fold 的 ld_matrix 扫描，直接查子图表建 CSR。
    #[serde(default = "default_panel_ld")]
    pub panel_ld: Option<String>,
}

fn default_diffevo_repeats() -> usize {
    20
}
fn default_r2_min() -> f64 {
    0.05
}
fn default_sampling() -> bool {
    true
}
fn default_k_max() -> usize {
    20000
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
fn default_panel_ld() -> Option<String> {
    Some("eur_subgraph".to_string())
}

// =====================================================================
// Node
// =====================================================================

const BIVARIATE_MIXER_NODE_KIND: &str = "bivariate_mixer";

#[derive(Clone)]
pub struct BivariateMixerNode {
    meta: NodePorts,
    spec: BivariateMixerNodeSpec,
}

fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(Some(sumstats_schema()))
        .add_input_port(Some(sumstats_schema()))
        .add_input_port(Some(fit1_schema()))
        .add_input_port(Some(fit1_schema()))
        .add_output_port(Some(output_schema()))
}

impl BivariateMixerNode {
    pub fn new(spec: BivariateMixerNodeSpec) -> Self {
        Self {
            meta: port_layout(),
            spec,
        }
    }
}

pub struct BivariateMixerNodeFactory {}

impl NodeFactory for BivariateMixerNodeFactory {
    fn kind(&self) -> &'static str {
        BIVARIATE_MIXER_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Fits bivariate MiXeR (fit2) on two GWAS traits, conditioned on each trait's univariate fit1."
    }

    fn doc(&self) -> &'static str {
        "Bivariate MiXeR (fit2) transform node. Takes four inputs: trait1 \
        sumstats (Z, N, rsid), trait2 sumstats (Z, N, rsid), trait1's fit1 \
        result (pi, sig2_beta, sig2_zero), and trait2's fit1 result. Queries \
        the Iceberg data lake for the LD matrix (`ld_matrix.eur_chr{N}`) and \
        allele frequency (`af.eur_af`), assembles a `BivariateData`, runs \
        `mixer::bivariate::fit2`, and outputs a single-row result DataFrame \
        with the bivariate parameters (pi1, pi2, pi12, rho_beta, rho_zero) and \
        derived quantities (rg, dice, h2_t1, h2_t2, loglike)."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(BivariateMixerNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: BivariateMixerNodeSpec = serde_json::from_value(spec)?;
        let node = BivariateMixerNode::new(config);
        Ok(Box::new(node))
    }
}

#[async_trait]
impl DagNode for BivariateMixerNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        BIVARIATE_MIXER_NODE_KIND
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
        use crate::dag::runtime::RuntimeStatus;
        let t0 = std::time::Instant::now();
        let n_chrom = self.spec.chromosomes.len();
        reporter.status(RuntimeStatus::Running);
        reporter.info(format!(
            "fit2: start (chromosomes={n_chrom}, r2_min={}, extract={}, sampling={}, \
             k_max={}, diffevo_repeats={})",
            self.spec.r2_min,
            self.spec.extract_enabled,
            self.spec.sampling,
            self.spec.k_max,
            self.spec.diffevo_repeats,
        ));

        if inputs.len() != 4 {
            return Err(BivariateMixerError::InvalidInput(format!(
                "bivariate_mixer 需要 4 个输入（trait1/trait2 sumstats + trait1/trait2 fit1），收到 {}",
                inputs.len()
            ))
            .into());
        }

        // Fresh, isolated context for this execution — no shared CatalogList,
        // so concurrent nodes / re-runs never collide on `register_table`.
        // Dropped at the end of this call.
        let ctx = node_ctx.session();

        // 1. 按 **port index** 查输入（不能用位置下标——`build_inputs` 推送顺序
        //    是 petgraph 边存储序，与 `to_port` 无关；见 ldsc_rg/lcv/mtag 的同样
        //    pattern）。端口布局：0/1=sumstats，2/3=fit1 约束。
        let sumstats1 = inputs
            .iter()
            .find(|i| i.port == 0)
            .ok_or_else(|| {
                BivariateMixerError::InvalidInput(
                    "missing trait1 sumstats input (port 0)".into(),
                )
            })?;
        let sumstats2 = inputs
            .iter()
            .find(|i| i.port == 1)
            .ok_or_else(|| {
                BivariateMixerError::InvalidInput(
                    "missing trait2 sumstats input (port 1)".into(),
                )
            })?;
        let fit1_t1 = inputs
            .iter()
            .find(|i| i.port == 2)
            .ok_or_else(|| {
                BivariateMixerError::InvalidInput(
                    "missing trait1 fit1 result input (port 2)".into(),
                )
            })?;
        let fit1_t2 = inputs
            .iter()
            .find(|i| i.port == 3)
            .ok_or_else(|| {
                BivariateMixerError::InvalidInput(
                    "missing trait2 fit1 result input (port 3)".into(),
                )
            })?;

        // 校验两个上游 sumstats 的必需列
        for (i, inp) in [sumstats1, sumstats2].iter().enumerate() {
            let sch = inp.data.schema();
            let avail: Vec<&str> = sch.fields().iter().map(|f| f.name().as_str()).collect();
            for needed in [INPUT_Z_COL, INPUT_N_COL, INPUT_RSID_COL] {
                if !sch.fields().iter().any(|f| f.name() == needed) {
                    return Err(BivariateMixerError::InvalidInput(format!(
                        "trait{} sumstats 缺少必需列 '{needed}'；现有列: {avail:?}",
                        i + 1
                    ))
                    .into());
                }
            }
        }
        ctx.register_table("sumstats1", sumstats1.data.clone().into_view())
            .map_err(|e| BivariateMixerError::Step {
                context: "register sumstats1".into(),
                detail: e.to_string(),
            })?;
        ctx.register_table("sumstats2", sumstats2.data.clone().into_view())
            .map_err(|e| BivariateMixerError::Step {
                context: "register sumstats2".into(),
                detail: e.to_string(),
            })?;

        // 2. 从 fit1 结果端口取 univariate 约束（pi, sig2_beta, sig2_zero）
        let c1 = read_constraint(&fit1_t1.data).await?;
        let c2 = read_constraint(&fit1_t2.data).await?;
        reporter.info(format!(
            "constraints: t1(pi={:.5}, sig2_beta={:.6}, sig2_zero={:.4})  \
             t2(pi={:.5}, sig2_beta={:.6}, sig2_zero={:.4})",
            c1.pi, c1.sig2_beta, c1.sig2_zero, c2.pi, c2.sig2_beta, c2.sig2_zero
        ));

        // 3. 读 universe（trait1 ∩ trait2 ∩ af）→ z1/z2/n1/n2/h/maf
        let mut rsid_to_idx: HashMap<String, u32> = HashMap::new();
        let mut z1 = Vec::new();
        let mut z2 = Vec::new();
        let mut n1 = Vec::new();
        let mut n2 = Vec::new();
        let mut h = Vec::new();
        let mut maf_vec: Vec<f64> = Vec::new();
        let mut chrom_base: Vec<u32> = Vec::new();
        for chrom in &self.spec.chromosomes {
            chrom_base.push(rsid_to_idx.len() as u32);
            let universe_sql = format!(
                r#"SELECT a.id AS rsid, a.alt_freq AS af,
                          s1."{z}" AS z1, s1."{n}" AS n1,
                          s2."{z}" AS z2, s2."{n}" AS n2
                   FROM iceberg.af.eur_af AS a
                   INNER JOIN sumstats1 AS s1 ON a.id = s1."{rsid}"
                   INNER JOIN sumstats2 AS s2 ON a.id = s2."{rsid}"
                   WHERE a.chrom = {chrom}"#,
                z = INPUT_Z_COL,
                n = INPUT_N_COL,
                rsid = INPUT_RSID_COL,
                chrom = chrom,
            );
            let df = BivariateMixerError::df_ctx(
                ctx.sql(&universe_sql).await,
                "universe (af ∩ trait1 ∩ trait2)",
                Some(*chrom),
                Some(&universe_sql),
            )?;
            let batches = BivariateMixerError::df_ctx(
                df.collect().await,
                "collect universe batches",
                Some(*chrom),
                None,
            )?;
            for batch in &batches {
                let rsids = col_as_string(batch, "rsid")?;
                let afs = col_as_f64(batch, "af")?;
                let z1s = col_as_f64(batch, "z1")?;
                let n1s = col_as_f64(batch, "n1")?;
                let z2s = col_as_f64(batch, "z2")?;
                let n2s = col_as_f64(batch, "n2")?;
                for row in 0..batch.num_rows() {
                    let rsid = rsids[row].as_str();
                    if rsid_to_idx.contains_key(rsid) {
                        continue;
                    }
                    rsid_to_idx.insert(rsid.to_string(), rsid_to_idx.len() as u32);
                    let f = afs.value(row);
                    let maf = f.min(1.0 - f);
                    z1.push(z1s.value(row));
                    z2.push(z2s.value(row));
                    n1.push(n1s.value(row));
                    n2.push(n2s.value(row));
                    h.push(2.0 * maf * (1.0 - maf));
                    maf_vec.push(maf);
                }
            }
        }
        let n_snp = z1.len();
        if n_snp == 0 {
            return Err(BivariateMixerError::InvalidInput(format!(
                "no SNPs overlap trait1 ∩ trait2 ∩ af.eur_af for chromosomes {:?}",
                self.spec.chromosomes
            ))
            .into());
        }
        reporter.info(format!(
            "universe: {n_snp} SNPs (trait1 ∩ trait2 ∩ af) over {n_chrom} chromosomes, \
             elapsed {:.2}s",
            t0.elapsed().as_secs_f64()
        ));

        // 4+5. 选 tag 子集 + 折 LD 成 tag-row CSR。
        //
        // 两条路径：
        // - **预计算子图**（`panel_ld` 设定，默认）：直接查 `eur_subgraph` 表（离线
        //   precompute_tags 产出的 tag 诱导 LD 子图）。表里 `id_a` 永远是 tag、
        //   `id_b` 是任意 panel 邻居，r²≥0.05 已过滤。tags = DISTINCT id_a ∩ universe，
        //   LD 边 = (tag→neighbor) 单向——bivariate cost 只调 `ld.row(tag)`，单向即足。
        //   一趟小表查询替代下面的两趟 ld_matrix 全扫描（extract + fold），CSR 从
        //   ~2.4GB 预过滤源构建而非 ~12GB 全表。
        // - **内联 extract + fold**（`panel_ld` 为 None）：运行时扫 ld_matrix 做
        //   select_tags + tag-row 对称化 CSR。
        let (tags, ld_triples): (Vec<u32>, Vec<(u32, u32, f64)>) = if let Some(table) =
            &self.spec.panel_ld
        {
            reporter.info(format!(
                "fold: loading precomputed tag-induced subgraph ({table})"
            ));
            let chrom_list: Vec<String> = self
                .spec
                .chromosomes
                .iter()
                .map(|c| c.to_string())
                .collect();
            let sub_sql = format!(
                "SELECT id_a, id_b, r2 FROM iceberg.mixer.{table} WHERE chrom IN ({})",
                chrom_list.join(", ")
            );
            let df = BivariateMixerError::df_ctx(
                ctx.sql(&sub_sql).await,
                "load subgraph",
                None,
                Some(&sub_sql),
            )?;
            let mut stream = BivariateMixerError::df_ctx(
                df.execute_stream().await,
                "subgraph stream",
                None,
                None,
            )?;
            let mut tag_set: std::collections::HashSet<u32> = std::collections::HashSet::new();
            let mut edges: Vec<(u32, u32, f64)> = Vec::new();
            while let Some(batch) =
                BivariateMixerError::df_ctx(stream.try_next().await, "subgraph batch", None, None)?
            {
                let a_ids = col_as_string(&batch, "id_a")?;
                let b_ids = col_as_string(&batch, "id_b")?;
                let r2s = col_as_f64(&batch, "r2")?;
                for row in 0..batch.num_rows() {
                    let a = a_ids[row].as_str();
                    let b = b_ids[row].as_str();
                    // id_a = tag、id_b = 邻居；两端都必须落在 universe 内。
                    if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
                        tag_set.insert(ta);
                        edges.push((ta, tb, r2s.value(row)));
                    }
                }
            }
            // tags 按 index 升序（与 randprune/cost 的遍历顺序无关，但确定性更友好）。
            let mut tags: Vec<u32> = tag_set.into_iter().collect();
            tags.sort_unstable();
            reporter.info(format!(
                "fold: {} tags, {} LD edges from subgraph, elapsed {:.2}s",
                tags.len(),
                edges.len(),
                t0.elapsed().as_secs_f64()
            ));
            (tags, edges)
        } else {
            // 内联路径：extract（select_tags）+ fold（tag-row 对称 CSR）。
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
                    let df = BivariateMixerError::df_ctx(
                        ctx.sql(&adj_sql).await,
                        "extract adjacency (ld r²>thr)",
                        Some(*chrom),
                        Some(&adj_sql),
                    )?;
                    let mut stream = BivariateMixerError::df_ctx(
                        df.execute_stream().await,
                        "extract adjacency stream",
                        Some(*chrom),
                        None,
                    )?;
                    while let Some(batch) = BivariateMixerError::df_ctx(
                        stream.try_next().await,
                        "extract adjacency batch",
                        Some(*chrom),
                        None,
                    )? {
                        for_each_ld_pair(&batch, &rsid_to_idx, |a, b, r2| {
                            adj_triples.push(((a as usize - base) as u32, b, r2));
                            adj_triples.push(((b as usize - base) as u32, a, r2));
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
            reporter.info(format!(
                "extract: {} tags ({}), elapsed {:.2}s",
                tags.len(),
                if self.spec.extract_enabled {
                    "MAF≥min + LD prune + subset"
                } else {
                    "all SNPs"
                },
                t0.elapsed().as_secs_f64()
            ));

            // fold：tag-row 对称 CSR，收"≥1 端点是 tag"的对并双向补齐。
            let mut ld_triples: Vec<(u32, u32, f64)> = Vec::new();
            for chrom in &self.spec.chromosomes {
                let ld_sql = format!(
                    "SELECT id_a, id_b, unphased_r2 FROM iceberg.ld_matrix.eur_chr{chrom} WHERE unphased_r2 >= {r2}",
                    chrom = chrom,
                    r2 = self.spec.r2_min,
                );
                let df = BivariateMixerError::df_ctx(
                    ctx.sql(&ld_sql).await,
                    "LD fold (ld r²≥r2min, tag-row)",
                    Some(*chrom),
                    Some(&ld_sql),
                )?;
                let mut stream = BivariateMixerError::df_ctx(
                    df.execute_stream().await,
                    "LD fold stream",
                    Some(*chrom),
                    None,
                )?;
                while let Some(batch) = BivariateMixerError::df_ctx(
                    stream.try_next().await,
                    "LD fold batch",
                    Some(*chrom),
                    None,
                )? {
                    for_each_ld_pair(&batch, &rsid_to_idx, |a, b, r2| {
                        if tag_set.contains(&a) {
                            ld_triples.push((a, b, r2));
                        }
                        if tag_set.contains(&b) {
                            ld_triples.push((b, a, r2));
                        }
                    })?;
                }
            }
            (tags, ld_triples)
        };

        // 6. 组装 BivariateData + randprune 权重。
        let mut data = mixer::bivariate::BivariateData::new(z1, z2, n1, n2, h, &ld_triples);
        let rp_cfg = mixer::weights::RandpruneConfig {
            n: self.spec.randprune_n,
            r2_threshold: self.spec.randprune_r2,
            use_w_ld: false,
            seed: self.spec.seed,
        };
        data.weights = mixer::weights::randprune_weights(&data.ld, n_snp, &tags, None, &rp_cfg);
        data.tags = tags;

        // 5. 跑 fit2（DE×repeats → NM → brute1 → brent1），全程 CPU-bound。
        let cfg = mixer::bivariate::Fit2Config {
            diffevo_repeats: self.spec.diffevo_repeats,
            sampling: self.spec.sampling,
            k_max: self.spec.k_max,
            seed: self.spec.seed,
            ..Default::default()
        };
        reporter.info(format!(
            "fold: {} LD triples (tag-row symmetric), sum_weights={:.2}, elapsed {:.2}s — \
             entering fit2",
            ld_triples.len(),
            data.weights.iter().sum::<f64>(),
            t0.elapsed().as_secs_f64()
        ));
        let fit_t0 = std::time::Instant::now();
        let result = mixer::bivariate::fit2(&data, c1, c2, &cfg);
        reporter.info(format!(
            "fit2: optimization done in {:.2}s (total elapsed {:.2}s)",
            fit_t0.elapsed().as_secs_f64(),
            t0.elapsed().as_secs_f64()
        ));
        reporter.info(format!(
            "fit2 result: pi1={:.5} pi2={:.5} pi12={:.5} rho_beta={:.4} rho_zero={:.4} \
             rg={:.4} dice={:.4} h2_t1={:.4} h2_t2={:.4} loglike={:.2}",
            result.pi1,
            result.pi2,
            result.pi12,
            result.rho_beta,
            result.rho_zero,
            result.rg,
            result.dice,
            result.h2_t1,
            result.h2_t2,
            result.loglike,
        ));
        if !(-1.0..=1.0).contains(&result.rg) {
            reporter.warn(format!(
                "fit2: rg={:.4} outside [-1,1] — possible model misspecification",
                result.rg
            ));
        }
        if result.pi12 > 0.999 || result.pi12 < 1e-6 {
            reporter.warn(format!(
                "fit2: pi12={:.6} at boundary — genetic overlap identifiability degeneracy \
                 (rg remains identifiable; reruns may differ)",
                result.pi12
            ));
        }
        if !result.loglike.is_finite() {
            reporter.warn(format!(
                "fit2: non-finite loglike ({:?}) — optimizer may have diverged",
                result.loglike
            ));
        }

        // 7. 打包结果
        let batch = build_result_batch(&result)?;
        let df =
            BivariateMixerError::df_ctx(ctx.read_batch(batch), "read result batch", None, None)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        reporter.info(format!(
            "fit2: finished in {:.2}s",
            t0.elapsed().as_secs_f64()
        ));
        Ok(res)
    }
}

/// 从 fit1 结果 DataFrame 读 (pi, sig2_beta, sig2_zero) 单行约束。
async fn read_constraint(
    df: &DataFrame,
) -> Result<mixer::bivariate::UnivariateConstraint, BivariateMixerError> {
    let batches = BivariateMixerError::df_ctx(
        df.clone().collect().await,
        "read fit1 constraint result",
        None,
        None,
    )?;
    let batch = batches.into_iter().next().ok_or_else(|| {
        BivariateMixerError::InvalidInput(
            "fit1 约束结果为空——上游 univariate_mixer 是否产出了单行 (pi, sig2_beta, sig2_zero)？"
                .into(),
        )
    })?;
    let pi = single_f64(&batch, PARAM_PI)?;
    let sig2_beta = single_f64(&batch, PARAM_SB)?;
    let sig2_zero = single_f64(&batch, PARAM_SZ)?;
    Ok(mixer::bivariate::UnivariateConstraint {
        pi,
        sig2_beta,
        sig2_zero,
    })
}

// =====================================================================
// Arrow 列提取 helpers
// =====================================================================

fn single_f64(batch: &RecordBatch, name: &str) -> Result<f64, BivariateMixerError> {
    // fit1 结果由 univariate_mixer 节点产出，schema 声明为 Float64（非 Iceberg
    // round-trip），所以这里仍严格匹配 Float64；若遇到 Float32 友好提示。
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| BivariateMixerError::InvalidInput(format!("fit1 结果缺列 '{name}'")))?;
    if let Some(a) = col.as_any().downcast_ref::<Float64Array>() {
        return Ok(a.value(0));
    }
    if let Some(a) = col.as_any().downcast_ref::<arrow_array::Float32Array>() {
        return Ok(a.value(0) as f64);
    }
    Err(BivariateMixerError::InvalidInput(format!(
        "列 '{name}' 非 Float32/Float64 (got {})",
        col.data_type()
    )))
}

fn col_as_string(
    batch: &RecordBatch,
    name: &str,
) -> Result<Vec<String>, BivariateMixerError> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| BivariateMixerError::InvalidInput(format!("column '{name}' not found")))?;
    super::meta::string_opt_values(col.as_ref())
        .map(|vals| vals.into_iter().map(|v| v.unwrap_or_default()).collect())
        .ok_or_else(|| {
            BivariateMixerError::InvalidInput(format!("column '{name}' is not a string type"))
        })
}

/// 对一条 LD batch 的每个 pair（两端点都在 universe 内）调用 `emit(global_a, global_b, r2)`。
/// 不做 r² 过滤——由调用方在闭包里决定。
fn for_each_ld_pair(
    batch: &RecordBatch,
    rsid_to_idx: &HashMap<String, u32>,
    mut emit: impl FnMut(u32, u32, f64),
) -> Result<(), BivariateMixerError> {
    let a_ids = col_as_string(batch, "id_a")?;
    let b_ids = col_as_string(batch, "id_b")?;
    let r2s = col_as_f64(batch, "unphased_r2")?;
    for row in 0..batch.num_rows() {
        let a = a_ids[row].as_str();
        let b = b_ids[row].as_str();
        if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
            emit(ta, tb, r2s.value(row));
        }
    }
    Ok(())
}

/// 读 f64 列的轻量视图：兼容 Iceberg 里常见的 `Float32` 存储（如 `r2`、
/// `h_a`），按需 `as f64` 提升。调用方仍用 `.value(row)` 取值——零分配。
///
/// 上游 GWAS sumstats 的 Z/N 与 `af.eur_af.alt_freq` 仍是 Float64，走 `F64`
/// 分支无额外开销；只有 `eur_subgraph` 的 `r2` 等列走 `F32` 分支。
enum F64Col<'a> {
    F64(&'a Float64Array),
    F32(&'a arrow_array::Float32Array),
}

impl F64Col<'_> {
    #[inline]
    fn value(&self, i: usize) -> f64 {
        match self {
            F64Col::F64(a) => a.value(i),
            F64Col::F32(a) => a.value(i) as f64,
        }
    }
}

fn col_as_f64<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<F64Col<'a>, BivariateMixerError> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| BivariateMixerError::InvalidInput(format!("column '{name}' not found")))?;
    if let Some(a) = col.as_any().downcast_ref::<Float64Array>() {
        return Ok(F64Col::F64(a));
    }
    if let Some(a) = col.as_any().downcast_ref::<arrow_array::Float32Array>() {
        return Ok(F64Col::F32(a));
    }
    Err(BivariateMixerError::InvalidInput(format!(
        "column '{name}' is not Float32/Float64 (got {})",
        col.data_type()
    )))
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_type_is_stable() {
        assert_eq!(BIVARIATE_MIXER_NODE_KIND, "bivariate_mixer");
    }

    #[tokio::test]
    async fn construct_node_with_spec() {
        let spec = BivariateMixerNodeSpec {
            chromosomes: vec![21, 22],
            diffevo_repeats: 5,
            r2_min: 0.05,
            sampling: true,
            k_max: 20000,
            randprune_n: 64,
            randprune_r2: 0.1,
            seed: 123,
            extract_enabled: true,
            extract_maf: 0.05,
            extract_subset: 2_000_000,
            extract_r2: 0.8,
            panel_ld: None,
        };
        let node = BivariateMixerNode::new(spec);
        assert_eq!(node.kind(), "bivariate_mixer");
        // 4 输入端口、1 输出端口
        assert_eq!(node.ports().input_ports().len(), 4);
        assert_eq!(node.ports().output_ports().len(), 1);
    }
}
