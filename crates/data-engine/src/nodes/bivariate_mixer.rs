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

use arrow_array::{Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::prelude::{DataFrame, SessionContext};
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
pub enum BivariateMixerError {
    #[error("failed to read upstream batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("datalake error: {0}")]
    Datalake(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
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

// =====================================================================
// Node
// =====================================================================

const BIVARIATE_MIXER_NODE_KIND: &str = "bivariate_mixer";

#[derive(Clone)]
pub struct BivariateMixerNode {
    meta: NodePorts,
    ctx: SessionContext,
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
    pub fn new(ctx: SessionContext, spec: BivariateMixerNodeSpec) -> Self {
        Self {
            meta: port_layout(),
            ctx,
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
        node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: BivariateMixerNodeSpec = serde_json::from_value(spec)?;
        let ctx = new_isolated_ctx(node_ctx.runtime_env, node_ctx.iceberg_catalog);
        let node = BivariateMixerNode::new(ctx, config);
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

    async fn execute(&mut self, inputs: &[NodeInput]) -> Result<PortOutputs, DagError> {
        if inputs.len() != 4 {
            return Err(BivariateMixerError::InvalidInput(format!(
                "bivariate_mixer 需要 4 个输入（trait1/trait2 sumstats + trait1/trait2 fit1），收到 {}",
                inputs.len()
            ))
            .into());
        }

        let ctx = &self.ctx;

        // 1. 注册两个 sumstats 为临时表
        ctx.register_table("sumstats1", inputs[0].data.clone().into_view())?;
        ctx.register_table("sumstats2", inputs[1].data.clone().into_view())?;

        // 2. 从 fit1 结果端口取 univariate 约束（pi, sig2_beta, sig2_zero）
        let c1 = read_constraint(&inputs[2].data).await?;
        let c2 = read_constraint(&inputs[3].data).await?;

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
                z = INPUT_Z_COL, n = INPUT_N_COL, rsid = INPUT_RSID_COL, chrom = chrom,
            );
            let batches = ctx.sql(&universe_sql).await?.collect().await?;
            for batch in &batches {
                let rsids = col_as_string(batch, "rsid")?;
                let afs = col_as_f64(batch, "af")?;
                let z1s = col_as_f64(batch, "z1")?;
                let n1s = col_as_f64(batch, "n1")?;
                let z2s = col_as_f64(batch, "z2")?;
                let n2s = col_as_f64(batch, "n2")?;
                for row in 0..batch.num_rows() {
                    let rsid = rsids.value(row);
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

        // 4. extract：选 tag 子集（r²>extract_r2 邻接 → select_tags）。
        let tags: Vec<u32> = if self.spec.extract_enabled {
            let mut adj_blocks: Vec<(usize, mixer::ld_matrix::LdBlock)> = Vec::new();
            for (ci, chrom) in self.spec.chromosomes.iter().enumerate() {
                let base = chrom_base[ci] as usize;
                let n_k = (if ci + 1 < chrom_base.len() { chrom_base[ci + 1] as usize } else { n_snp }) - base;
                if n_k == 0 {
                    continue;
                }
                let adj_sql = format!(
                    "SELECT id_a, id_b, unphased_r2 FROM iceberg.ld_matrix.eur_chr{chrom} WHERE unphased_r2 > {r2}",
                    chrom = chrom, r2 = self.spec.extract_r2,
                );
                let mut adj_triples: Vec<(u32, u32, f64)> = Vec::new();
                let mut stream = ctx.sql(&adj_sql).await?.execute_stream().await?;
                while let Some(batch) = stream.try_next().await? {
                    for_each_ld_pair(&batch, &rsid_to_idx, |a, b, _r2| {
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

        // 5. 折 LD（r²≥r2_min）成 **tag-row 对称 CSR**：只收"≥1 端点是 tag"的对，
        //    并给每个 tag 行补齐两个方向（row(tag) 含全部 panel 邻居，含非 tag）。
        //    这把 CSR 从全量 ~12GB 降到 tag×panel ~2.4GB，且修正非对称 LD 的漏算。
        let mut ld_triples: Vec<(u32, u32, f64)> = Vec::new();
        for chrom in &self.spec.chromosomes {
            let ld_sql = format!(
                "SELECT id_a, id_b, unphased_r2 FROM iceberg.ld_matrix.eur_chr{chrom} WHERE unphased_r2 >= {r2}",
                chrom = chrom, r2 = self.spec.r2_min,
            );
            let mut stream = ctx.sql(&ld_sql).await?.execute_stream().await?;
            while let Some(batch) = stream.try_next().await? {
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

        // 5. 跑 fit2
        let cfg = mixer::bivariate::Fit2Config {
            diffevo_repeats: self.spec.diffevo_repeats,
            sampling: self.spec.sampling,
            k_max: self.spec.k_max,
            ..Default::default()
        };
        let result = mixer::bivariate::fit2(&data, c1, c2, &cfg);

        // 7. 打包结果
        let batch = build_result_batch(&result)?;
        let df = ctx.read_batch(batch)?;
        let mut res: PortOutputs = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

/// 从 fit1 结果 DataFrame 读 (pi, sig2_beta, sig2_zero) 单行约束。
async fn read_constraint(
    df: &DataFrame,
) -> Result<mixer::bivariate::UnivariateConstraint, BivariateMixerError> {
    let batches = df.clone().collect().await?;
    let batch = batches
        .into_iter()
        .next()
        .ok_or_else(|| BivariateMixerError::InvalidInput("fit1 结果为空".into()))?;
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
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| BivariateMixerError::InvalidInput(format!("fit1 结果缺列 '{name}'")))?;
    let arr = col
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| BivariateMixerError::InvalidInput(format!("列 '{name}' 非 Float64")))?;
    Ok(arr.value(0))
}

fn col_as_string<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a StringArray, BivariateMixerError> {
    batch
        .column_by_name(name)
        .ok_or_else(|| BivariateMixerError::InvalidInput(format!("column '{name}' not found")))?
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| BivariateMixerError::InvalidInput(format!("column '{name}' is not Utf8")))
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
        let a = a_ids.value(row);
        let b = b_ids.value(row);
        if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
            emit(ta, tb, r2s.value(row));
        }
    }
    Ok(())
}

fn col_as_f64<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a Float64Array, BivariateMixerError> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| BivariateMixerError::InvalidInput(format!("column '{name}' not found")))?;
    col.as_any().downcast_ref::<Float64Array>().ok_or_else(|| {
        BivariateMixerError::InvalidInput(format!(
            "column '{name}' is not Float64 (got {})",
            col.data_type()
        ))
    })
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
        };
        let node = BivariateMixerNode::new(SessionContext::new(), spec);
        assert_eq!(node.kind(), "bivariate_mixer");
        // 4 输入端口、1 输出端口
        assert_eq!(node.ports().input_ports().len(), 4);
        assert_eq!(node.ports().output_ports().len(), 1);
    }
}
