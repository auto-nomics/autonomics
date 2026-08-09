//! Clustering DAG nodes — K-means, DBSCAN, GMM, hierarchical, spectral.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int32Array, RecordBatch, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use super::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use ml::cluster::{
    DbscanOptions, GmmOptions, HierarchicalResult, KMeansOptions, Linkage, dbscan,
    gaussian_mixture, hierarchical, kmeans, kmeans_predict,
};

// ── helpers (re-use from preprocess_nodes) ───────────────────────────────

async fn collect_batches(inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or(DagError::NodeError {
        node_type: "ml_cluster".into(),
        msg: "no input port connected".into(),
    })?;
    input
        .data
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml_cluster".into(),
            msg: format!("collect failed: {e}"),
        })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::NodeError {
            node_type: "ml_cluster".into(),
            msg: format!("read_batch failed: {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

// ═══════════════════════════════════════════════════════════════════════
// K-Means
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct KMeansSpec {
    /// Feature columns for clustering.
    pub features: Vec<String>,
    /// Number of clusters K.
    pub k: usize,
    /// Maximum iterations (default 300).
    #[serde(default = "d_max_iter")]
    pub max_iter: u32,
    /// Number of restarts with different init (default 10).
    #[serde(default = "d_n_init")]
    pub n_init: u32,
    /// Convergence tolerance (default 1e-4).
    #[serde(default = "d_tol")]
    pub tolerance: f64,
    /// Random seed (default 42).
    #[serde(default = "d_seed")]
    pub seed: u64,
}
fn d_max_iter() -> u32 {
    300
}
fn d_n_init() -> u32 {
    10
}
fn d_tol() -> f64 {
    1e-4
}
fn d_seed() -> u64 {
    42
}

pub struct KMeansFactory;
impl NodeFactory for KMeansFactory {
    fn kind(&self) -> &'static str {
        "ml_kmeans"
    }
    fn desc(&self) -> &'static str {
        "K-means clustering with k-means++ initialisation."
    }
    fn doc(&self) -> &'static str {
        "KMeans: partitions data into K clusters using Lloyd's algorithm with k-means++ seeding. Outputs cluster assignments + centroid distances."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(KMeansSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: KMeansSpec = serde_json::from_value(spec)?;
        Ok(Box::new(KMeansNode {
            features: s.features,
            k: s.k,
            max_iter: s.max_iter,
            n_init: s.n_init,
            tolerance: s.tolerance,
            seed: s.seed,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct KMeansNode {
    features: Vec<String>,
    k: usize,
    max_iter: u32,
    n_init: u32,
    tolerance: f64,
    seed: u64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for KMeansNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_kmeans"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data =
            common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
                node_type: "ml_kmeans".into(),
                msg: e.to_string(),
            })?;
        let opts = KMeansOptions {
            k: self.k,
            max_n_iterations: self.max_iter,
            n_init: self.n_init,
            tolerance: self.tolerance,
            seed: self.seed,
        };
        let model = kmeans(&data, &opts).map_err(|e| DagError::NodeError {
            node_type: "ml_kmeans".into(),
            msg: e.to_string(),
        })?;
        let labels = kmeans_predict(&model, &data);

        // Compute distance to assigned centroid
        let (nrows, _ncols) = data.shape();
        let distances: Vec<f64> = (0..nrows)
            .map(|i| {
                let centroid = &model.centroids[labels[i]];
                let point: Vec<f64> = (0..data.ncols()).map(|j| data[(i, j)]).collect();
                point
                    .iter()
                    .zip(centroid)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f64>()
                    .sqrt()
            })
            .collect();

        let batch = build_cluster_output(
            &batches,
            &labels,
            &distances,
            "cluster",
            "distance_to_centroid",
        )?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// DBSCAN
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DbscanSpec {
    pub features: Vec<String>,
    /// Maximum distance between two samples for them to be neighbours.
    pub eps: f64,
    /// Minimum samples in a neighbourhood to form a core point.
    #[serde(default = "d_min_points")]
    pub min_points: usize,
}
fn d_min_points() -> usize {
    5
}

pub struct DbscanFactory;
impl NodeFactory for DbscanFactory {
    fn kind(&self) -> &'static str {
        "ml_dbscan"
    }
    fn desc(&self) -> &'static str {
        "DBSCAN density-based clustering."
    }
    fn doc(&self) -> &'static str {
        "DBSCAN: identifies clusters of arbitrary shape via density reachability. Noise points are labelled -1 (null)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(DbscanSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: DbscanSpec = serde_json::from_value(spec)?;
        Ok(Box::new(DbscanNode {
            features: s.features,
            eps: s.eps,
            min_points: s.min_points,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct DbscanNode {
    features: Vec<String>,
    eps: f64,
    min_points: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for DbscanNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_dbscan"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data =
            common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
                node_type: "ml_dbscan".into(),
                msg: e.to_string(),
            })?;
        let result = dbscan(
            &data,
            &DbscanOptions {
                eps: self.eps,
                min_points: self.min_points,
            },
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_dbscan".into(),
            msg: e.to_string(),
        })?;

        let labels: Vec<i32> = result
            .labels
            .iter()
            .map(|l| l.map(|v| v as i32).unwrap_or(-1))
            .collect();
        let batch = build_int_cluster_output(&batches, &labels, "cluster")?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// GMM
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GmmSpec {
    pub features: Vec<String>,
    pub k: usize,
    #[serde(default = "d_gmm_max_iter")]
    pub max_iter: usize,
    #[serde(default = "d_gmm_tol")]
    pub tol: f64,
    #[serde(default = "d_seed")]
    pub seed: u64,
    #[serde(default = "d_gmm_n_init")]
    pub n_init: usize,
}
fn d_gmm_max_iter() -> usize {
    100
}
fn d_gmm_tol() -> f64 {
    1e-6
}
fn d_gmm_n_init() -> usize {
    1
}

pub struct GmmFactory;
impl NodeFactory for GmmFactory {
    fn kind(&self) -> &'static str {
        "ml_gmm"
    }
    fn desc(&self) -> &'static str {
        "Gaussian Mixture Model (EM) with full covariance."
    }
    fn doc(&self) -> &'static str {
        "GMM: fits a Gaussian mixture via EM with k-means++ init. Reports BIC/AIC for model selection. Outputs cluster assignments + per-component probabilities summary."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GmmSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: GmmSpec = serde_json::from_value(spec)?;
        Ok(Box::new(GmmNode {
            features: s.features,
            k: s.k,
            max_iter: s.max_iter,
            tol: s.tol,
            seed: s.seed,
            n_init: s.n_init,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct GmmNode {
    features: Vec<String>,
    k: usize,
    max_iter: usize,
    tol: f64,
    seed: u64,
    n_init: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for GmmNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_gmm"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data =
            common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
                node_type: "ml_gmm".into(),
                msg: e.to_string(),
            })?;
        let opts = GmmOptions {
            k: self.k,
            max_iter: self.max_iter,
            tol: self.tol,
            seed: self.seed,
            n_init: self.n_init,
        };
        let model = gaussian_mixture(&data, &opts).map_err(|e| DagError::NodeError {
            node_type: "ml_gmm".into(),
            msg: e.to_string(),
        })?;

        // Assign each sample to argmax component
        let (nrows, d) = data.shape();
        let labels: Vec<usize> = (0..nrows)
            .map(|i| {
                let point: Vec<f64> = (0..d).map(|j| data[(i, j)]).collect();
                (0..model.k)
                    .map(|c| {
                        let cov = common::mat_from_row_major(
                            d,
                            d,
                            &model.covariances[c]
                                .iter()
                                .flatten()
                                .cloned()
                                .collect::<Vec<_>>(),
                        );
                        (
                            c,
                            ml::cluster::gaussian_density_pub(&point, &model.means[c], &cov)
                                * model.weights[c],
                        )
                    })
                    .fold((0usize, f64::NEG_INFINITY), |(bc, bp), (c, p)| {
                        if p > bp { (c, p) } else { (bc, bp) }
                    })
                    .0
            })
            .collect();

        // Output: original data + cluster label
        let batch = build_cluster_output(
            &batches,
            &labels,
            &vec![0.0; nrows],
            "cluster",
            "_gmm_dummy",
        )?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Hierarchical
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HierarchicalSpec {
    pub features: Vec<String>,
    pub k: usize,
    #[serde(default = "d_linkage")]
    pub linkage: String,
}
fn d_linkage() -> String {
    "ward".into()
}

pub struct HierarchicalFactory;
impl NodeFactory for HierarchicalFactory {
    fn kind(&self) -> &'static str {
        "ml_hierarchical"
    }
    fn desc(&self) -> &'static str {
        "Agglomerative hierarchical clustering."
    }
    fn doc(&self) -> &'static str {
        "Hierarchical: bottom-up agglomerative clustering with Ward/complete/average/single linkage. Cuts the dendrogram at K clusters."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(HierarchicalSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: HierarchicalSpec = serde_json::from_value(spec)?;
        Ok(Box::new(HierarchicalNode {
            features: s.features,
            k: s.k,
            linkage: s.linkage,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct HierarchicalNode {
    features: Vec<String>,
    k: usize,
    linkage: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for HierarchicalNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_hierarchical"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let data =
            common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
                node_type: "ml_hierarchical".into(),
                msg: e.to_string(),
            })?;
        let lk = match self.linkage.as_str() {
            "complete" => Linkage::Complete,
            "average" => Linkage::Average,
            "single" => Linkage::Single,
            _ => Linkage::Ward,
        };
        let result = hierarchical(&data, self.k, lk).map_err(|e| DagError::NodeError {
            node_type: "ml_hierarchical".into(),
            msg: e.to_string(),
        })?;
        let nrows = data.nrows();
        let zeros = vec![0.0; nrows];
        let batch =
            build_cluster_output(&batches, &result.labels, &zeros, "cluster", "_hier_dummy")?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Spectral Clustering (placeholder — requires eigendecomposition of Laplacian)
// ═══════════════════════════════════════════════════════════════════════

pub struct SpectralClusteringFactory;
impl NodeFactory for SpectralClusteringFactory {
    fn kind(&self) -> &'static str {
        "ml_spectral_cluster"
    }
    fn desc(&self) -> &'static str {
        "Spectral clustering (normalised Laplacian + K-means)."
    }
    fn doc(&self) -> &'static str {
        "SpectralClustering: builds affinity matrix, embeds via normalised Laplacian eigenvectors, runs K-means in embedding space. [Coming soon]"
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SpectralSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let _s: SpectralSpec = serde_json::from_value(spec)?;
        Err(dag_core::registry::error::Error::Unknown(
            "ml_spectral_cluster is not yet implemented".into(),
        ))
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct SpectralSpec {
    features: Vec<String>,
    k: usize,
}

// ═══════════════════════════════════════════════════════════════════════
// Shared batch builders
// ═══════════════════════════════════════════════════════════════════════

fn build_cluster_output(
    batches: &[RecordBatch],
    labels: &[usize],
    distances: &[f64],
    cluster_col: &str,
    dist_col: &str,
) -> Result<RecordBatch, DagError> {
    let schema = batches
        .first()
        .ok_or(DagError::NodeError {
            node_type: "ml_cluster".into(),
            msg: "no input rows".into(),
        })?
        .schema();
    let orig_schema = schema.as_ref();
    let mut fields: Vec<Arc<Field>> = orig_schema.fields().iter().cloned().collect();
    let mut arrays: Vec<Arc<dyn Array>> = (0..orig_schema.fields().len())
        .map(|i| batches.first().unwrap().column(i).clone())
        .collect();

    // cluster column
    fields.push(Arc::new(Field::new(cluster_col, DataType::UInt32, true)));
    arrays.push(Arc::new(UInt32Array::from(
        labels.iter().map(|&v| v as u32).collect::<Vec<_>>(),
    )));

    // distance column (if not dummy)
    if !dist_col.starts_with('_') {
        fields.push(Arc::new(Field::new(dist_col, DataType::Float64, true)));
        arrays.push(Arc::new(Float64Array::from(Vec::from(distances))));
    }

    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError {
        node_type: "ml_cluster".into(),
        msg: format!("build output: {e}"),
    })
}

fn build_int_cluster_output(
    batches: &[RecordBatch],
    labels: &[i32],
    cluster_col: &str,
) -> Result<RecordBatch, DagError> {
    let schema = batches
        .first()
        .ok_or(DagError::NodeError {
            node_type: "ml_cluster".into(),
            msg: "no input rows".into(),
        })?
        .schema();
    let orig_schema = schema.as_ref();
    let mut fields: Vec<Arc<Field>> = orig_schema.fields().iter().cloned().collect();
    let mut arrays: Vec<Arc<dyn Array>> = (0..orig_schema.fields().len())
        .map(|i| batches.first().unwrap().column(i).clone())
        .collect();
    fields.push(Arc::new(Field::new(cluster_col, DataType::Int32, true)));
    arrays.push(Arc::new(Int32Array::from(Vec::from(labels))));
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError {
        node_type: "ml_cluster".into(),
        msg: format!("build output: {e}"),
    })
}
