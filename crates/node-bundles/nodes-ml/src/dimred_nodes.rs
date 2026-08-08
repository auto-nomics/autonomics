//! Dimensionality reduction DAG nodes — PCA, ICA, t-SNE, NMF, Truncated SVD.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use super::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

async fn collect_batches(inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or(DagError::NodeError {
        node_type: "ml_dimred".into(), msg: "no input".into()
    })?;
    input.data.clone().collect().await.map_err(|e| DagError::NodeError {
        node_type: "ml_dimred".into(), msg: format!("collect: {e}"),
    })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx.session().read_batch(batch).map_err(|e| DagError::NodeError {
        node_type: "ml_dimred".into(), msg: format!("read_batch: {e}"),
    })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

/// Build an output RecordBatch from embedding coordinates + original data.
fn build_embedding_output(
    batches: &[RecordBatch],
    embedding: &[Vec<f64>],
    prefix: &str,
) -> Result<RecordBatch, DagError> {
    let schema = batches.first().ok_or(DagError::NodeError {
        node_type: "ml_dimred".into(), msg: "no input rows".into()
    })?.schema();
    let mut fields: Vec<Arc<Field>> = schema.fields().iter().cloned().collect();
    let mut arrays: Vec<Arc<dyn Array>> = (0..schema.fields().len())
        .map(|i| batches.first().unwrap().column(i).clone())
        .collect();

    let n_dims = embedding.first().map(|r| r.len()).unwrap_or(0);
    for d in 0..n_dims {
        let col_data: Vec<f64> = embedding.iter().map(|row| row[d]).collect();
        fields.push(Arc::new(Field::new(format!("{prefix}_{d}"), DataType::Float64, true)));
        arrays.push(Arc::new(Float64Array::from(col_data)));
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError {
        node_type: "ml_dimred".into(), msg: format!("build output: {e}"),
    })
}

/// Build a summary table from key-value float metrics.
fn build_summary_output(
    rows: &[(String, f64)],
    node_type: &str,
) -> Result<RecordBatch, DagError> {
    let (names, values): (Vec<String>, Vec<f64>) = rows.iter().cloned().unzip();
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("component", DataType::Utf8, false),
            Field::new("value", DataType::Float64, false),
        ])),
        vec![
            Arc::new(arrow_array::StringArray::from(names)),
            Arc::new(Float64Array::from(values)),
        ],
    ).map_err(|e| DagError::NodeError { node_type: node_type.into(), msg: e.to_string() })
}

// ═══════════════════════════════════════════════════════════════════════
// PCA
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PcaSpec {
    pub features: Vec<String>,
    pub n_components: usize,
}

pub struct PcaFactory;
impl NodeFactory for PcaFactory {
    fn kind(&self) -> &'static str { "ml_pca" }
    fn desc(&self) -> &'static str { "Principal Component Analysis (PCA)." }
    fn doc(&self) -> &'static str { "PCA: linear dimensionality reduction via SVD of the covariance matrix. Outputs projected data (pc_0, pc_1, …) appended to the input table." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(PcaSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: PcaSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PcaNode { features: s.features, n_components: s.n_components, meta: self.ports() }))
    }
}

#[derive(Clone)]
struct PcaNode { features: Vec<String>, n_components: usize, meta: NodePorts }

#[async_trait]
impl DagNode for PcaNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "ml_pca" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data = common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
            node_type: "ml_pca".into(), msg: e.to_string()
        })?;
        let model = ml::dimred::pca(&data, self.n_components).map_err(|e| DagError::NodeError {
            node_type: "ml_pca".into(), msg: e.to_string()
        })?;
        let transformed = ml::dimred::pca_transform(&model, &data);
        let (nrows, ncols) = transformed.shape();
        let embedding: Vec<Vec<f64>> = (0..nrows).map(|i| (0..ncols).map(|j| transformed[(i, j)]).collect()).collect();
        let batch = build_embedding_output(&batches, &embedding, "pc")?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// FastICA
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct IcaSpec {
    pub features: Vec<String>,
    pub n_components: usize,
}

pub struct IcaFactory;
impl NodeFactory for IcaFactory {
    fn kind(&self) -> &'static str { "ml_ica" }
    fn desc(&self) -> &'static str { "Independent Component Analysis (FastICA)." }
    fn doc(&self) -> &'static str { "FastICA: separates multivariate signal into additive independent components. Outputs ic_0, ic_1, … appended to input table." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(IcaSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: IcaSpec = serde_json::from_value(spec)?;
        Ok(Box::new(IcaNode { features: s.features, n_components: s.n_components, meta: self.ports() }))
    }
}

#[derive(Clone)]
struct IcaNode { features: Vec<String>, n_components: usize, meta: NodePorts }

#[async_trait]
impl DagNode for IcaNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "ml_ica" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data = common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
            node_type: "ml_ica".into(), msg: e.to_string()
        })?;
        let model = ml::dimred::fast_ica(&data, self.n_components).map_err(|e| DagError::NodeError {
            node_type: "ml_ica".into(), msg: e.to_string()
        })?;
        let batch = build_embedding_output(&batches, &model.components, "ic")?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// t-SNE
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TsneSpec {
    pub features: Vec<String>,
    #[serde(default = "d_tsne_dims")]
    pub n_components: usize,
    #[serde(default = "d_tsne_perplexity")]
    pub perplexity: f64,
    #[serde(default = "d_tsne_iter")]
    pub max_iter: usize,
    #[serde(default = "d_tsne_threshold")]
    pub approx_threshold: f64,
}
fn d_tsne_dims() -> usize { 2 }
fn d_tsne_perplexity() -> f64 { 5.0 }
fn d_tsne_iter() -> usize { 1000 }
fn d_tsne_threshold() -> f64 { 350.0 }

pub struct TsneFactory;
impl NodeFactory for TsneFactory {
    fn kind(&self) -> &'static str { "ml_tsne" }
    fn desc(&self) -> &'static str { "t-SNE non-linear dimensionality reduction." }
    fn doc(&self) -> &'static str { "t-SNE: maps high-dimensional data to 2D/3D for visualisation via Barnes-Hut t-SNE. Outputs tsne_0, tsne_1 columns." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(TsneSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: TsneSpec = serde_json::from_value(spec)?;
        Ok(Box::new(TsneNode {
            features: s.features, n_components: s.n_components,
            perplexity: s.perplexity, max_iter: s.max_iter,
            approx_threshold: s.approx_threshold, meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct TsneNode {
    features: Vec<String>, n_components: usize, perplexity: f64,
    max_iter: usize, approx_threshold: f64, meta: NodePorts,
}

#[async_trait]
impl DagNode for TsneNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "ml_tsne" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data = common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
            node_type: "ml_tsne".into(), msg: e.to_string()
        })?;
        let opts = ml::dimred::TsneOptions {
            embedding_size: self.n_components,
            perplexity: self.perplexity,
            max_iter: self.max_iter,
            approx_threshold: self.approx_threshold,
            ..Default::default()
        };
        let model = ml::dimred::tsne(&data, &opts).map_err(|e| DagError::NodeError {
            node_type: "ml_tsne".into(), msg: e.to_string()
        })?;
        let batch = build_embedding_output(&batches, &model.embedding, "tsne")?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// NMF
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct NmfSpec {
    pub features: Vec<String>,
    pub n_components: usize,
    #[serde(default = "d_nmf_iter")]
    pub max_iter: usize,
    #[serde(default = "d_nmf_tol")]
    pub tol: f64,
    #[serde(default = "d_nmf_seed")]
    pub seed: u64,
}
fn d_nmf_iter() -> usize { 200 }
fn d_nmf_tol() -> f64 { 1e-4 }
fn d_nmf_seed() -> u64 { 42 }

pub struct NmfFactory;
impl NodeFactory for NmfFactory {
    fn kind(&self) -> &'static str { "ml_nmf" }
    fn desc(&self) -> &'static str { "Non-negative Matrix Factorization (NMF)." }
    fn doc(&self) -> &'static str { "NMF: factorises a non-negative matrix V ≈ W·H. Outputs W (basis weights) as nmf_0, nmf_1, … columns." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(NmfSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: NmfSpec = serde_json::from_value(spec)?;
        Ok(Box::new(NmfNode {
            features: s.features, n_components: s.n_components,
            max_iter: s.max_iter, tol: s.tol, seed: s.seed, meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct NmfNode {
    features: Vec<String>, n_components: usize,
    max_iter: usize, tol: f64, seed: u64, meta: NodePorts,
}

#[async_trait]
impl DagNode for NmfNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "ml_nmf" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data = common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
            node_type: "ml_nmf".into(), msg: e.to_string()
        })?;
        let model = ml::dimred::nmf(&data, self.n_components, self.max_iter, self.tol, self.seed)
            .map_err(|e| DagError::NodeError { node_type: "ml_nmf".into(), msg: e.to_string() })?;
        let batch = build_embedding_output(&batches, &model.w, "nmf")?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Truncated SVD / LSA
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TruncatedSvdSpec {
    pub features: Vec<String>,
    pub n_components: usize,
}

pub struct TruncatedSvdFactory;
impl NodeFactory for TruncatedSvdFactory {
    fn kind(&self) -> &'static str { "ml_truncated_svd" }
    fn desc(&self) -> &'static str { "Truncated SVD (Latent Semantic Analysis)." }
    fn doc(&self) -> &'static str { "TruncatedSVD: dimensionality reduction via thin SVD without centering. Suitable for sparse text/count data. Outputs lsa_0, lsa_1, … columns." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(TruncatedSvdSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: TruncatedSvdSpec = serde_json::from_value(spec)?;
        Ok(Box::new(TruncatedSvdNode { features: s.features, n_components: s.n_components, meta: self.ports() }))
    }
}

#[derive(Clone)]
struct TruncatedSvdNode { features: Vec<String>, n_components: usize, meta: NodePorts }

#[async_trait]
impl DagNode for TruncatedSvdNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "ml_truncated_svd" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data = common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
            node_type: "ml_truncated_svd".into(), msg: e.to_string()
        })?;
        let model = ml::dimred::truncated_svd(&data, self.n_components).map_err(|e| DagError::NodeError {
            node_type: "ml_truncated_svd".into(), msg: e.to_string()
        })?;
        // Project: X @ V_k (components^T) → n × n_components
        let (nrows, ncols) = data.shape();
        let embedding: Vec<Vec<f64>> = (0..nrows).map(|i| {
            (0..model.n_components).map(|k| {
                let mut val = 0.0;
                for j in 0..ncols {
                    val += data[(i, j)] * model.components[k][j];
                }
                val
            }).collect()
        }).collect();
        let batch = build_embedding_output(&batches, &embedding, "lsa")?;
        emit_batch(ctx, batch)
    }
}
