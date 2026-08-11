//! GenomicSEM DAG nodes.
//!
//! Provides structural equation modeling of GWAS summary statistics:
//!
//! - **`gsem_usermodel`** — Fit a user-specified SEM to an LDSC-derived
//!   genetic covariance matrix.
//! - **`gsem_commonfactor`** — Fit a one-factor model.
//! - **`gsem_rgmodel`** — Compute model-implied genetic correlation matrix.
//!
//! All nodes accept an upstream `DataFrame` representing the S/V covariance
//! structure (from LDSC), and produce a results `DataFrame` with parameter
//! estimates, standard errors, and model fit statistics.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use faer::Mat;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::{
    dag::{DagError, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

use genomic_sem;

// =====================================================================
// Shared helpers
// =====================================================================

/// Read a Covstruc from a "vech" formatted DataFrame.
fn read_covstruc_from_batch(
    batch: &RecordBatch,
    n_traits: usize,
) -> Result<genomic_sem::utils::Covstruc, DagError> {
    let z = n_traits * (n_traits + 1) / 2;

    let mut s_vec = Vec::with_capacity(z);
    for i in 0..z {
        let col_name = format!("s_{i}");
        let arr = batch.column_by_name(&col_name)
            .ok_or_else(|| DagError::NodeError {
                node_type: "gsem".into(),
                msg: format!("missing column '{col_name}'"),
            })?
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| DagError::NodeError {
                node_type: "gsem".into(),
                msg: format!("column '{col_name}' is not Float64"),
            })?;
        s_vec.push(arr.value(0));
    }
    let s = genomic_sem::linalg::vech_inv(&s_vec);

    let mut v_vec = Vec::with_capacity(z * z);
    for i in 0..(z * z) {
        let col_name = format!("v_{i}");
        let arr = batch.column_by_name(&col_name)
            .ok_or_else(|| DagError::NodeError {
                node_type: "gsem".into(),
                msg: format!("missing column '{col_name}'"),
            })?
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| DagError::NodeError {
                node_type: "gsem".into(),
                msg: format!("column '{col_name}' is not Float64"),
            })?;
        v_vec.push(arr.value(0));
    }
    let v = Mat::from_fn(z, z, |i, j| v_vec[i * z + j]);

    let m = batch.column_by_name("m")
        .and_then(|a| a.as_any().downcast_ref::<Float64Array>())
        .map(|a| a.value(0))
        .unwrap_or(100_000.0);

    Ok(genomic_sem::utils::Covstruc {
        v,
        s,
        i_mat: Mat::<f64>::identity(n_traits, n_traits),
        n: Mat::zeros(1, z),
        m,
        v_stand: None,
        s_stand: None,
    })
}

/// Output schema for SEM results.
fn results_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("lhs", DataType::Utf8, false),
        Field::new("op", DataType::Utf8, false),
        Field::new("rhs", DataType::Utf8, false),
        Field::new("est", DataType::Float64, false),
        Field::new("se", DataType::Float64, true),
        Field::new("p_value", DataType::Float64, true),
        Field::new("std_all", DataType::Float64, true),
        Field::new("chisq", DataType::Float64, false),
        Field::new("df", DataType::Float64, false),
    ]))
}

/// Build a results batch from SEM output.
fn build_results_batch(
    results: &[genomic_sem::usermodel::ParamResult],
    modelfit: &genomic_sem::usermodel::ModelFit,
) -> Result<RecordBatch, DagError> {
    let n = results.len();

    let lhs: Vec<&str> = results.iter().map(|r| r.lhs.as_str()).collect();
    let op: Vec<&str> = results.iter().map(|r| r.op.as_str()).collect();
    let rhs: Vec<&str> = results.iter().map(|r| r.rhs.as_str()).collect();
    let est: Vec<f64> = results.iter().map(|r| r.unstand_est).collect();
    let se: Vec<f64> = results.iter().map(|r| r.unstand_se).collect();
    let pval: Vec<f64> = results.iter().map(|r| r.p_value).collect();
    let std_all: Vec<f64> = results.iter().map(|r| r.std_all).collect();
    let chisq: Vec<f64> = vec![modelfit.chisq; n];
    let df_vals: Vec<f64> = vec![modelfit.df as f64; n];

    let batch = RecordBatch::try_new(
        results_schema(),
        vec![
            Arc::new(StringArray::from(lhs)),
            Arc::new(StringArray::from(op)),
            Arc::new(StringArray::from(rhs)),
            Arc::new(Float64Array::from(est)),
            Arc::new(Float64Array::from(se)),
            Arc::new(Float64Array::from(pval)),
            Arc::new(Float64Array::from(std_all)),
            Arc::new(Float64Array::from(chisq)),
            Arc::new(Float64Array::from(df_vals)),
        ],
    ).map_err(|e| DagError::NodeError {
        node_type: "gsem".into(),
        msg: format!("arrow error: {e}"),
    })?;

    Ok(batch)
}

// =====================================================================
// usermodel node
// =====================================================================

const GSEM_USERMODEL_NODE_KIND: &str = "gsem_usermodel";

/// Config for `gsem_usermodel` node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GsemUsermodelConfig {
    /// Lavaan-style model syntax (e.g., "F1 =~ NA*V1 + V2 + V3\nF1 ~~ 1*F1").
    pub model: String,
    /// Number of traits in the S/V covariance structure.
    pub n_traits: usize,
    /// Estimation method: "DWLS" or "ML".
    #[serde(default = "default_estimation")]
    pub estimation: String,
    /// Standardize latent variances.
    #[serde(default)]
    pub std_lv: bool,
}

fn default_estimation() -> String {
    "DWLS".to_string()
}

fn gsem_usermodel_port_layout() -> NodePorts {
    NodePorts::new()
        .add_input_port(None)
        .add_output_port(Some(results_schema()))
}

#[derive(Clone)]
pub struct GsemUsermodelNode {
    meta: NodePorts,
    config: GsemUsermodelConfig,
}

pub struct GsemUsermodelNodeFactory;

impl NodeFactory for GsemUsermodelNodeFactory {
    fn kind(&self) -> &'static str {
        GSEM_USERMODEL_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "User-specified SEM on genetic covariance matrix (GenomicSEM usermodel)."
    }

    fn doc(&self) -> &'static str {
        "Fits a structural equation model to the LDSC-derived genetic covariance matrix S \
        using DWLS or ML estimation with sandwich-corrected standard errors. \
        Specify the model in lavaan syntax (e.g., 'F1 =~ NA*V1 + V2 + V3')."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GsemUsermodelConfig)
    }

    fn ports(&self) -> NodePorts {
        gsem_usermodel_port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: GsemUsermodelConfig = serde_json::from_value(spec)?;
        Ok(Box::new(GsemUsermodelNode {
            meta: gsem_usermodel_port_layout(),
            config,
        }))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["GenomicSEM".into()]
    }
}

#[async_trait]
impl DagNode for GsemUsermodelNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        GSEM_USERMODEL_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(DagError::NodeError {
            node_type: GSEM_USERMODEL_NODE_KIND.into(),
            msg: "missing input".into(),
        })?;
        let batches: Vec<RecordBatch> = input.data.clone().collect().await
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_USERMODEL_NODE_KIND.into(),
                msg: format!("collect failed: {e}"),
            })?;
        if batches.is_empty() || batches[0].num_rows() == 0 {
            return Err(DagError::NodeError {
                node_type: GSEM_USERMODEL_NODE_KIND.into(),
                msg: "empty input".into(),
            });
        }

        let batch = &batches[0];
        let covstruc = read_covstruc_from_batch(batch, self.config.n_traits)?;

        let estimation = match self.config.estimation.as_str() {
            "ML" => genomic_sem::sem::EstimationMethod::ML,
            _ => genomic_sem::sem::EstimationMethod::DWLS,
        };

        let user_config = genomic_sem::usermodel::UserModelConfig {
            estimation,
            model: self.config.model.clone(),
            std_lv: self.config.std_lv,
            ..Default::default()
        };

        let result = genomic_sem::usermodel::usermodel(&covstruc, &user_config)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_USERMODEL_NODE_KIND.into(),
                msg: e.to_string(),
            })?;

        let output_batch = build_results_batch(&result.results, &result.modelfit)?;
        let df = node_ctx.session().read_batch(output_batch)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_USERMODEL_NODE_KIND.into(),
                msg: format!("read_batch: {e}"),
            })?;

        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// commonfactor node
// =====================================================================

const GSEM_COMMONFACTOR_NODE_KIND: &str = "gsem_commonfactor";

/// Config for `gsem_commonfactor` node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GsemCommonfactorConfig {
    /// Number of traits in the S/V covariance structure.
    pub n_traits: usize,
    /// Estimation method: "DWLS" or "ML".
    #[serde(default = "default_estimation")]
    pub estimation: String,
}

#[derive(Clone)]
pub struct GsemCommonfactorNode {
    meta: NodePorts,
    config: GsemCommonfactorConfig,
}

pub struct GsemCommonfactorNodeFactory;

impl NodeFactory for GsemCommonfactorNodeFactory {
    fn kind(&self) -> &'static str {
        GSEM_COMMONFACTOR_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Common factor model on genetic covariance matrix (GenomicSEM commonfactor)."
    }

    fn doc(&self) -> &'static str {
        "Fits a single-factor confirmatory model to the LDSC-derived genetic \
        covariance matrix. Requires at least 3 traits."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GsemCommonfactorConfig)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(Some(results_schema()))
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: GsemCommonfactorConfig = serde_json::from_value(spec)?;
        Ok(Box::new(GsemCommonfactorNode {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_output_port(Some(results_schema())),
            config,
        }))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["GenomicSEM".into()]
    }
}

#[async_trait]
impl DagNode for GsemCommonfactorNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        GSEM_COMMONFACTOR_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(DagError::NodeError {
            node_type: GSEM_COMMONFACTOR_NODE_KIND.into(),
            msg: "missing input".into(),
        })?;
        let batches: Vec<RecordBatch> = input.data.clone().collect().await
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_COMMONFACTOR_NODE_KIND.into(),
                msg: format!("collect failed: {e}"),
            })?;
        if batches.is_empty() || batches[0].num_rows() == 0 {
            return Err(DagError::NodeError {
                node_type: GSEM_COMMONFACTOR_NODE_KIND.into(),
                msg: "empty input".into(),
            });
        }

        let batch = &batches[0];
        let covstruc = read_covstruc_from_batch(batch, self.config.n_traits)?;

        let estimation = match self.config.estimation.as_str() {
            "ML" => genomic_sem::sem::EstimationMethod::ML,
            _ => genomic_sem::sem::EstimationMethod::DWLS,
        };

        let config = genomic_sem::commonfactor::CommonFactorConfig { estimation };
        let result = genomic_sem::commonfactor::commonfactor(&covstruc, &config)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_COMMONFACTOR_NODE_KIND.into(),
                msg: e.to_string(),
            })?;

        let output_batch = build_results_batch(&result.results, &result.modelfit)?;
        let df = node_ctx.session().read_batch(output_batch)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_COMMONFACTOR_NODE_KIND.into(),
                msg: format!("read_batch: {e}"),
            })?;

        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

// =====================================================================
// rgmodel node
// =====================================================================

const GSEM_RGMODEL_NODE_KIND: &str = "gsem_rgmodel";

/// Config for `gsem_rgmodel` node.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GsemRgmodelConfig {
    /// Number of traits in the S/V covariance structure.
    pub n_traits: usize,
}

fn rgmodel_output_schema(k: usize) -> SchemaRef {
    let z = k * (k + 1) / 2;
    let mut fields = Vec::new();
    for i in 0..z {
        fields.push(Field::new(&format!("r_{i}"), DataType::Float64, false));
    }
    for i in 0..z {
        fields.push(Field::new(&format!("v_r_{i}"), DataType::Float64, false));
    }
    Arc::new(Schema::new(fields))
}

#[derive(Clone)]
pub struct GsemRgmodelNode {
    meta: NodePorts,
    config: GsemRgmodelConfig,
}

pub struct GsemRgmodelNodeFactory;

impl NodeFactory for GsemRgmodelNodeFactory {
    fn kind(&self) -> &'static str {
        GSEM_RGMODEL_NODE_KIND
    }

    fn desc(&self) -> &'static str {
        "Model-implied genetic correlation matrix (GenomicSEM rgmodel)."
    }

    fn doc(&self) -> &'static str {
        "Computes the genetic correlation matrix R = cov2cor(S) and its \
        sampling covariance V_R via the delta method."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(GsemRgmodelConfig)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(None) // schema depends on n_traits at runtime
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let config: GsemRgmodelConfig = serde_json::from_value(spec)?;
        Ok(Box::new(GsemRgmodelNode {
            meta: NodePorts::new()
                .add_input_port(None)
                .add_output_port(Some(rgmodel_output_schema(config.n_traits))),
            config,
        }))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["GenomicSEM".into()]
    }
}

#[async_trait]
impl DagNode for GsemRgmodelNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        GSEM_RGMODEL_NODE_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let input = inputs.first().ok_or(DagError::NodeError {
            node_type: GSEM_RGMODEL_NODE_KIND.into(),
            msg: "missing input".into(),
        })?;
        let batches: Vec<RecordBatch> = input.data.clone().collect().await
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_RGMODEL_NODE_KIND.into(),
                msg: format!("collect failed: {e}"),
            })?;
        if batches.is_empty() || batches[0].num_rows() == 0 {
            return Err(DagError::NodeError {
                node_type: GSEM_RGMODEL_NODE_KIND.into(),
                msg: "empty input".into(),
            });
        }

        let batch = &batches[0];
        let covstruc = read_covstruc_from_batch(batch, self.config.n_traits)?;
        let result = genomic_sem::rgmodel::rgmodel(&covstruc, false)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_RGMODEL_NODE_KIND.into(),
                msg: e.to_string(),
            })?;

        let k = self.config.n_traits;
        let z = k * (k + 1) / 2;
        let r_vec = genomic_sem::linalg::vech(&result.r);

        let mut columns: Vec<Arc<dyn Array>> = Vec::new();
        for i in 0..z {
            columns.push(Arc::new(Float64Array::from(vec![r_vec[i]])));
        }
        for i in 0..z {
            columns.push(Arc::new(Float64Array::from(vec![result.v_r[(i, i)]])));
        }

        let output_batch = RecordBatch::try_new(
            rgmodel_output_schema(k),
            columns,
        ).map_err(|e| DagError::NodeError {
            node_type: GSEM_RGMODEL_NODE_KIND.into(),
            msg: format!("arrow: {e}"),
        })?;

        let df = node_ctx.session().read_batch(output_batch)
            .map_err(|e| DagError::NodeError {
                node_type: GSEM_RGMODEL_NODE_KIND.into(),
                msg: format!("read_batch: {e}"),
            })?;

        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
