//! Univariate MiXeR (`fit1`) transform node.
//!
//! 接收上游 GWAS 汇总统计 `DataFrame`（含 Z-score、样本量、rsid），从
//! Iceberg 数据湖读取 LD 矩阵（`ld_matrix.eur_chr{N}`）和 allele frequency
//! （`af.eur_af`），组装成 [`mixer::data::ChromData`]，调用
//! [`mixer::fit::fit1`] 拟合 spike-and-slab 模型，输出单行结果
//! `DataFrame`（pi, sig2_beta, sig2_zero, h2, nc, nc_p9, aic, bic, loglike）。
//!
//! 当前为骨架：DAG 接口（端口/工厂/DagNode trait）已就位，数据湖读取与
//! ChromData 组装逻辑用 `TODO` 插桩占位，待后续阶段填充。

use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
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
pub enum UnivariateMixerError {
    #[error("univariate MiXeR computation not yet implemented: {0}")]
    NotImplemented(String),
    #[error("failed to read upstream sumstats: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("datalake error: {0}")]
    Datalake(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
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
#[allow(dead_code)] // execute() 待实现后启用
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
    /// `ld_matrix.{ld_table_prefix}{chrom}` 表。
    pub chromosomes: Vec<u32>,
    /// LD 矩阵表前缀（命名空间固定为 `ld_matrix`，表名 = 前缀+染色体号）。
    /// 默认 `"eur_chr"` → `iceberg.ld_matrix.eur_chr21`。
    #[serde(default = "default_ld_prefix")]
    pub ld_table_prefix: String,
    /// allele frequency 表名（命名空间固定为 `af`）。默认 `"eur_af"`。
    #[serde(default = "default_af_table")]
    pub af_table: String,
    /// 差分进化重复次数（原版 `--diffevo-fast-repeats`，默认 20）。
    #[serde(default = "default_diffevo_repeats")]
    pub diffevo_repeats: usize,
    /// r² 阈值：低于此值的 LD 对忽略（首版固定 sig2_zeroL=0，此字段预留）。
    #[serde(default = "default_r2_min")]
    pub r2_min: f64,
}

fn default_ld_prefix() -> String {
    "eur_chr".to_string()
}
fn default_af_table() -> String {
    "eur_af".to_string()
}
fn default_diffevo_repeats() -> usize {
    20
}
fn default_r2_min() -> f64 {
    0.05
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
    meta: NodeMeta,
    datalake: Arc<Datalake>,
    spec: UnivariateMixerNodeSpec,
}

impl UnivariateMixerNode {
    pub fn new(datalake: Arc<Datalake>, spec: UnivariateMixerNodeSpec) -> Self {
        let meta = NodeMeta::new()
            .add_input_port(Some(input_schema()))
            .add_output_port(Some(output_schema()));
        Self {
            meta,
            datalake,
            spec,
        }
    }
}

pub struct UnivariateMixerNodeFactory {}

impl NodeFactory for UnivariateMixerNodeFactory {
    fn kind(&self) -> &'static str {
        UNIVARIATE_MIXER_NODE_KIND
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(UnivariateMixerNodeSpec)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let config: UnivariateMixerNodeSpec = serde_json::from_value(spec)?;
        let node = UnivariateMixerNode::new(node_ctx.datalake, config);
        Ok(Box::new(node))
    }
}

#[async_trait]
impl DagNode for UnivariateMixerNode {
    fn meta(&self) -> &NodeMeta {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn node_type(&self) -> &str {
        UNIVARIATE_MIXER_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&mut self, inputs: &[NodeInput]) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(UnivariateMixerError::InvalidInput(
            "no input DataFrame".into(),
        ))?;

        // 1. 拿到注册了 Iceberg catalog 的 DataFusion 上下文。
        let ctx = self
            .datalake
            .get_ctx()
            .await
            .map_err(UnivariateMixerError::from)?;

        // 2. 把上游 sumstats 注册为临时表。
        ctx.register_table("sumstats", input.data.clone().into_view())?;

        // ============================================================
        // TODO[phase-2]: 数据湖读取 + ChromData 组装
        // ============================================================
        // for chrom in &self.spec.chromosomes {
        //     // (a) 读 LD 矩阵（COO）: id_a, id_b, unphased_r2
        //     let ld_sql = format!(
        //         "SELECT id_a, id_b, unphased_r2 FROM iceberg.ld_matrix.{}{}",
        //         self.spec.ld_table_prefix, chrom
        //     );
        //     // (b) 读 allele frequency: id, alt_freq
        //     let af_sql = format!(
        //         "SELECT id, alt_freq FROM iceberg.af.{} WHERE chrom = {}",
        //         self.spec.af_table, chrom
        //     );
        //     // (c) 建立 rsid → u32 index 映射（tag/snp 共用 index 空间）
        //     // (d) COO 三元组 (rsid_a, rsid_b, r2) → (tag_idx, snp_idx, r2)
        //     // (e) alt_freq → h = 2·f·(1−f)
        //     // (f) sumstats join rsid → z, n
        //     // (g) 组装 ChromData，收集进 Vec<ChromData>
        // }
        //
        // 当前用桩占位：数据湖读取与多染色体拟合尚未实现。
        let _ = (&ctx, &self.spec); // 暂时引用，避免未使用告警
        return Err(UnivariateMixerError::NotImplemented(
            "数据湖 LD/AF 读取与 ChromData 组装待实现（见 TODO[phase-2]）".into(),
        )
        .into());

        // ============================================================
        // 以下为拟合 + 结果打包的预期流程（占位，等 ChromData 就绪后启用）
        // ============================================================
        //
        // // 3. 跑 fit1（多染色体需扩展为聚合 cost；首版单染色体）。
        // let cfg = mixer::fit::FitConfig {
        //     diffevo_repeats: self.spec.diffevo_repeats,
        //     ..Default::default()
        // };
        // let result = mixer::fit::fit1(&chrom_data, &cfg);
        //
        // // 4. 打包单行结果 RecordBatch 并返回。
        // let batch = build_result_batch(&result)?;
        // let df = ctx.read_batch(batch)?;
        // let mut res: PortOutputs = PortOutputs::new();
        // res.insert(0, df);
        // Ok(res)
    }
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
            ld_table_prefix: default_ld_prefix(),
            af_table: default_af_table(),
            diffevo_repeats: 5,
            r2_min: 0.05,
        };
        let node = UnivariateMixerNode::new(Arc::new(Datalake::new()), spec);
        assert_eq!(node.node_type(), "univariate_mixer");
        // 一个输入端口、一个输出端口
        assert_eq!(node.meta().input_ports().len(), 1);
        assert_eq!(node.meta().output_ports().len(), 1);
    }
}
