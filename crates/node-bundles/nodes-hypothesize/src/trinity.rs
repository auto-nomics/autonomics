//! Likelihood trinity nodes (Group B): `hypothesize.wald_test`, `hypothesize.lrt`,
//! `hypothesize.score_test`. These read scalars from the spec, not from columns.

use super::common::{HypoNodeError, emit_test_row};
use dag_core::dag::DagError;
use dag_core::dag::graph::PortOutputs;
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use arrow_array::{Float64Array, Int32Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use hypothesize as h;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use std::sync::Arc;

fn one_port() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

// ════ wald_test ═════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WaldNodeSpec {
    pub estimate: f64,
    pub se: Option<f64>,
    pub vcov: Option<Vec<Vec<f64>>>,
    pub estimate_vec: Option<Vec<f64>>,
    pub null_value: Option<Vec<f64>>,
    #[serde(default)]
    pub null_scalar: Option<f64>,
}

pub struct WaldNodeFactory;
impl NodeFactory for WaldNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.wald_test"
    }
    fn desc(&self) -> &'static str {
        "Wald test (univariate or multivariate)."
    }
    fn doc(&self) -> &'static str {
        "Tests H₀: θ = θ₀. Univariate (estimate + se) or multivariate (estimate_vec + vcov)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(WaldNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        one_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: WaldNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(WaldNode {
            meta: one_port(),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct WaldNode {
    meta: NodePorts,
    spec: WaldNodeSpec,
}
#[async_trait]
impl DagNode for WaldNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.wald_test"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        let result =
            if let (Some(vcov), Some(est_vec)) = (&self.spec.vcov, &self.spec.estimate_vec) {
                let default_null = vec![0.0_f64; est_vec.len()];
                let null = self.spec.null_value.as_deref().unwrap_or(&default_null);
                h::wald_multi(est_vec, vcov, null)
            } else {
                let se = self
                    .spec
                    .se
                    .ok_or_else(|| HypoNodeError::Spec("wald_test needs se or vcov".into()))?;
                h::wald_uni(self.spec.estimate, se, self.spec.null_scalar.unwrap_or(0.0))
            }
            .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ lrt ═══════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LrtNodeSpec {
    pub null_loglik: f64,
    pub alt_loglik: f64,
    pub dof: u32,
}

pub struct LrtNodeFactory;
impl NodeFactory for LrtNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.lrt"
    }
    fn desc(&self) -> &'static str {
        "Likelihood ratio test."
    }
    fn doc(&self) -> &'static str {
        "Tests nested model comparison via -2(ℓ₀ - ℓ₁) ~ χ²(dof)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LrtNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        one_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: LrtNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LrtNode {
            meta: one_port(),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct LrtNode {
    meta: NodePorts,
    spec: LrtNodeSpec,
}
#[async_trait]
impl DagNode for LrtNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.lrt"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        let result = h::lrt(
            self.spec.null_loglik,
            self.spec.alt_loglik,
            Some(self.spec.dof),
        )
        .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ score_test ════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ScoreNodeSpec {
    pub score: f64,
    pub fisher_info: f64,
    pub null_value: Option<f64>,
}

pub struct ScoreNodeFactory;
impl NodeFactory for ScoreNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.score_test"
    }
    fn desc(&self) -> &'static str {
        "Score (Lagrange multiplier) test."
    }
    fn doc(&self) -> &'static str {
        "Tests H₀ using score and Fisher information at the null."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ScoreNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        one_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: ScoreNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ScoreNode {
            meta: one_port(),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct ScoreNode {
    meta: NodePorts,
    spec: ScoreNodeSpec,
}
#[async_trait]
impl DagNode for ScoreNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.score_test"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        let result = h::score_uni(self.spec.score, self.spec.fisher_info, self.spec.null_value)
            .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ invert_test (Group D) ═════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct InvertNodeSpec {
    pub estimate: f64,
    pub se: f64,
    /// Grid of candidate null values to test.
    pub grid: Vec<f64>,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
}
fn default_alpha() -> f64 {
    0.05
}

pub struct InvertNodeFactory;
impl NodeFactory for InvertNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.invert_test"
    }
    fn desc(&self) -> &'static str {
        "Invert a Wald test into a confidence set."
    }
    fn doc(&self) -> &'static str {
        "Grid-search inversion: finds all null values not rejected at level alpha."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(InvertNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: InvertNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(InvertNode {
            meta: NodePorts::new().add_output_port(None),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct InvertNode {
    meta: NodePorts,
    spec: InvertNodeSpec,
}
#[async_trait]
impl DagNode for InvertNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.invert_test"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let est = self.spec.estimate;
        let se = self.spec.se;
        let cs = h::invert_test(
            move |theta| h::wald_uni(est, se, theta).unwrap(),
            &self.spec.grid,
            self.spec.alpha,
        );
        let schema = Arc::new(Schema::new(vec![
            Field::new("lower", DataType::Float64, false),
            Field::new("upper", DataType::Float64, false),
            Field::new("alpha", DataType::Float64, false),
            Field::new("level", DataType::Float64, false),
            Field::new("n_grid_in_set", DataType::Int32, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(vec![cs.lower()])),
                Arc::new(Float64Array::from(vec![cs.upper()])),
                Arc::new(Float64Array::from(vec![cs.alpha])),
                Arc::new(Float64Array::from(vec![cs.level])),
                Arc::new(Int32Array::from(vec![cs.set.len() as i32])),
            ],
        )
        .map_err(|e| HypoNodeError::ReadBatch(e.to_string()))?;
        let session = ctx.session();
        let df = session
            .read_batch(batch)
            .map_err(|e| HypoNodeError::ReadBatch(e.to_string()))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}
