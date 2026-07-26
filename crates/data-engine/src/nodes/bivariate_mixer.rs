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
//! 注：当前用权重全 1（无 Rust 原生 randprune）。生产级精度需接入 randprune 权重
//! （见 `mixer` crate 的 README/记忆 `mixer-ld-dump-workflow`）。cost/optimizer 已交叉验证。

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::prelude::DataFrame;
use datalake::Datalake;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::meta::{DagNode, NodeInput, NodeMeta};
use crate::{
    data_engine::dag::{DagError, graph::PortOutputs},
    node_registry::registry::{NodeCtx, NodeFactory},
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

fn build_result_batch(r: &mixer::bivariate::BivariateFitResult) -> Result<RecordBatch, BivariateMixerError> {
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
    /// 随机种子（原版 `--seed`，默认 123）。
    #[serde(default = "default_seed")]
    pub seed: u64,
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

// =====================================================================
// Node
// =====================================================================

const BIVARIATE_MIXER_NODE_KIND: &str = "bivariate_mixer";

#[derive(Clone)]
pub struct BivariateMixerNode {
    meta: NodeMeta,
    datalake: Arc<Datalake>,
    spec: BivariateMixerNodeSpec,
}

impl BivariateMixerNode {
    pub fn new(datalake: Arc<Datalake>, spec: BivariateMixerNodeSpec) -> Self {
        // 4 输入：trait1 sumstats, trait2 sumstats, trait1 fit1, trait2 fit1
        let meta = NodeMeta::new()
            .add_input_port(Some(sumstats_schema()))
            .add_input_port(Some(sumstats_schema()))
            .add_input_port(Some(fit1_schema()))
            .add_input_port(Some(fit1_schema()))
            .add_output_port(Some(output_schema()));
        Self { meta, datalake, spec }
    }
}

pub struct BivariateMixerNodeFactory {}

impl NodeFactory for BivariateMixerNodeFactory {
    fn kind(&self) -> &'static str {
        BIVARIATE_MIXER_NODE_KIND
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(BivariateMixerNodeSpec)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: BivariateMixerNodeSpec = serde_json::from_value(spec)?;
        let node = BivariateMixerNode::new(node_ctx.datalake, config);
        Ok(Box::new(node))
    }
}

#[async_trait]
impl DagNode for BivariateMixerNode {
    fn meta(&self) -> &NodeMeta {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn node_type(&self) -> &str {
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

        let ctx = self.datalake.get_ctx().await.map_err(BivariateMixerError::from)?;

        // 1. 注册两个 sumstats 为临时表
        ctx.register_table("sumstats1", inputs[0].data.clone().into_view())?;
        ctx.register_table("sumstats2", inputs[1].data.clone().into_view())?;

        // 2. 从 fit1 结果端口取 univariate 约束（pi, sig2_beta, sig2_zero）
        let c1 = read_constraint(&inputs[2].data).await?;
        let c2 = read_constraint(&inputs[3].data).await?;

        // 3. 逐染色体：universe = trait1 ∩ trait2 ∩ af，收集 z1/z2/n1/n2/h + LD
        let mut rsid_to_idx: HashMap<String, u32> = HashMap::new();
        let mut z1 = Vec::new();
        let mut z2 = Vec::new();
        let mut n1 = Vec::new();
        let mut n2 = Vec::new();
        let mut h = Vec::new();
        let mut ld_triples: Vec<(u32, u32, f64)> = Vec::new();

        for chrom in &self.spec.chromosomes {
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
                    let idx = rsid_to_idx.len() as u32;
                    rsid_to_idx.insert(rsid.to_string(), idx);
                    let f = afs.value(row);
                    z1.push(z1s.value(row));
                    z2.push(z2s.value(row));
                    n1.push(n1s.value(row));
                    n2.push(n2s.value(row));
                    h.push(2.0 * f * (1.0 - f));
                }
            }

            let ld_sql = format!(
                "SELECT id_a, id_b, unphased_r2 FROM iceberg.ld_matrix.eur_chr{chrom}",
                chrom = chrom,
            );
            let ld_batches = ctx.sql(&ld_sql).await?.collect().await?;
            for batch in &ld_batches {
                let a_ids = col_as_string(batch, "id_a")?;
                let b_ids = col_as_string(batch, "id_b")?;
                let r2s = col_as_f64(batch, "unphased_r2")?;
                for row in 0..batch.num_rows() {
                    let r2 = r2s.value(row);
                    if r2 < self.spec.r2_min {
                        continue;
                    }
                    let a = a_ids.value(row);
                    let b = b_ids.value(row);
                    if let (Some(&ta), Some(&tb)) = (rsid_to_idx.get(a), rsid_to_idx.get(b)) {
                        ld_triples.push((ta, tb, r2));
                    }
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

        // 4. 组装 BivariateData，并计算随机剪枝权重（生产级精度，与原版 bit-exact）。
        //    tag 集 = 全部 SNP（节点不做 extract）。
        let mut data = mixer::bivariate::BivariateData::new(z1, z2, n1, n2, h, &ld_triples);
        let tags: Vec<u32> = (0..n_snp as u32).collect();
        let rp_cfg = mixer::weights::RandpruneConfig {
            n: self.spec.randprune_n,
            r2_threshold: self.spec.randprune_r2,
            use_w_ld: false,
            seed: self.spec.seed,
        };
        data.weights = mixer::weights::randprune_weights(&data.ld, n_snp, &tags, None, &rp_cfg);

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
    Ok(mixer::bivariate::UnivariateConstraint { pi, sig2_beta, sig2_zero })
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

fn col_as_string<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray, BivariateMixerError> {
    batch
        .column_by_name(name)
        .ok_or_else(|| BivariateMixerError::InvalidInput(format!("column '{name}' not found")))?
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| BivariateMixerError::InvalidInput(format!("column '{name}' is not Utf8")))
}

fn col_as_f64<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Float64Array, BivariateMixerError> {
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| BivariateMixerError::InvalidInput(format!("column '{name}' not found")))?;
    col.as_any().downcast_ref::<Float64Array>().ok_or_else(|| {
        BivariateMixerError::InvalidInput(format!("column '{name}' is not Float64 (got {})", col.data_type()))
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
        };
        let node = BivariateMixerNode::new(Arc::new(Datalake::new()), spec);
        assert_eq!(node.node_type(), "bivariate_mixer");
        // 4 输入端口、1 输出端口
        assert_eq!(node.meta().input_ports().len(), 4);
        assert_eq!(node.meta().output_ports().len(), 1);
    }
}
