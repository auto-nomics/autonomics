//! Variance-homogeneity and ANOVA nodes: `hypothesize.bartlett_test`,
//! `hypothesize.levene_test`, `hypothesize.fligner_test`, `hypothesize.oneway_anova`.

use super::common::{HypoNodeError, collect_input, emit_test_row, extract_groups};
use crate::dag::DagError;
use crate::node_registry::registry::{NodeCtx, NodeFactory};
use crate::nodes::meta::{DagNode, NodeInput, NodePorts};
use async_trait::async_trait;
use hypothesize as h;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

fn default_true() -> bool {
    true
}

macro_rules! group_node {
    ($factory:ident, $node:ident, $spec:ident, $kind:expr, $desc:expr, $doc:expr, $func:expr) => {
        #[derive(Debug, Clone, Deserialize, JsonSchema)]
        pub struct $spec {
            pub value_column: String,
            pub group_column: String,
        }

        pub struct $factory;
        impl NodeFactory for $factory {
            fn kind(&self) -> &'static str {
                $kind
            }
            fn desc(&self) -> &'static str {
                $desc
            }
            fn doc(&self) -> &'static str {
                $doc
            }
            fn spec_schema(&self) -> schemars::Schema {
                schema_for!($spec)
            }
            fn ports(&self) -> NodePorts {
                NodePorts::new().add_output_port(None).add_input_port(None)
            }
            fn build(
                &self,
                spec: serde_json::Value,
                _: NodeCtx,
            ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
                let s: $spec = serde_json::from_value(spec)?;
                Ok(Box::new($node {
                    meta: NodePorts::new().add_output_port(None).add_input_port(None),
                    spec: s,
                }))
            }
        }
        #[derive(Clone)]
        struct $node {
            meta: NodePorts,
            spec: $spec,
        }
        #[async_trait]
        impl DagNode for $node {
            fn ports(&self) -> &NodePorts {
                &self.meta
            }
            fn clone_box(&self) -> Box<dyn DagNode> {
                Box::new(self.clone())
            }
            fn kind(&self) -> &'static str {
                $kind
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            async fn execute(
                &mut self,
                ctx: &NodeCtx,
                inputs: &[NodeInput],
                _r: &crate::dag::node_event::NodeReporter,
            ) -> Result<crate::dag::graph::PortOutputs, DagError> {
                let batches = collect_input(inputs).await?;
                let groups =
                    extract_groups(&batches, &self.spec.value_column, &self.spec.group_column)?;
                if groups.len() < 2 {
                    return Err(HypoNodeError::Insufficient("need ≥ 2 groups".into()).into());
                }
                let refs: Vec<&[f64]> = groups.iter().map(|(_, v)| v.as_slice()).collect();
                let result = $func(&refs).map_err(|e| HypoNodeError::Test(e.to_string()))?;
                emit_test_row(ctx, &result)
            }
        }
    };
}

group_node!(
    BartlettNodeFactory,
    BartlettNode,
    BartlettNodeSpec,
    "hypothesize.bartlett_test",
    "Bartlett test for variance homogeneity.",
    "Parametric k-sample test for equality of variances.",
    h::bartlett_test
);

group_node!(
    FlignerNodeFactory,
    FlignerNode,
    FlignerNodeSpec,
    "hypothesize.fligner_test",
    "Fligner-Killeen test for variance homogeneity.",
    "Nonparametric rank-based k-sample test for equality of variances.",
    h::fligner_test
);

// ════ levene_test ═══════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LeveneNodeSpec {
    pub value_column: String,
    pub group_column: String,
    #[serde(default = "default_levene_center")]
    pub center: String,
}
fn default_levene_center() -> String {
    "median".to_string()
}

pub struct LeveneNodeFactory;
impl NodeFactory for LeveneNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.levene_test"
    }
    fn desc(&self) -> &'static str {
        "Levene/Brown-Forsythe test for variance homogeneity."
    }
    fn doc(&self) -> &'static str {
        "ANOVA on absolute deviations from group center (mean or median)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(LeveneNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let s: LeveneNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(LeveneNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct LeveneNode {
    meta: NodePorts,
    spec: LeveneNodeSpec,
}
#[async_trait]
impl DagNode for LeveneNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.levene_test"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &crate::dag::node_event::NodeReporter,
    ) -> Result<crate::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        let groups = extract_groups(&batches, &self.spec.value_column, &self.spec.group_column)?;
        if groups.len() < 2 {
            return Err(HypoNodeError::Insufficient("need ≥ 2 groups".into()).into());
        }
        let refs: Vec<&[f64]> = groups.iter().map(|(_, v)| v.as_slice()).collect();
        let center = match self.spec.center.to_lowercase().as_str() {
            "mean" => h::Center::Mean,
            "median" => h::Center::Median,
            _ => h::Center::Median,
        };
        let result =
            h::levene_test(&refs, center).map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}

// ════ oneway_anova ══════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AnovaNodeSpec {
    pub value_column: String,
    pub group_column: String,
    #[serde(default = "default_true")]
    pub var_equal: bool,
}

pub struct AnovaNodeFactory;
impl NodeFactory for AnovaNodeFactory {
    fn kind(&self) -> &'static str {
        "hypothesize.oneway_anova"
    }
    fn desc(&self) -> &'static str {
        "One-way ANOVA (classical or Welch)."
    }
    fn doc(&self) -> &'static str {
        "Tests equality of means across k groups. var_equal=true → classical, false → Welch."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(AnovaNodeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_output_port(None).add_input_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let s: AnovaNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(AnovaNode {
            meta: NodePorts::new().add_output_port(None).add_input_port(None),
            spec: s,
        }))
    }
}
#[derive(Clone)]
struct AnovaNode {
    meta: NodePorts,
    spec: AnovaNodeSpec,
}
#[async_trait]
impl DagNode for AnovaNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "hypothesize.oneway_anova"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &crate::dag::node_event::NodeReporter,
    ) -> Result<crate::dag::graph::PortOutputs, DagError> {
        let batches = collect_input(inputs).await?;
        let groups = extract_groups(&batches, &self.spec.value_column, &self.spec.group_column)?;
        if groups.len() < 2 {
            return Err(HypoNodeError::Insufficient("need ≥ 2 groups".into()).into());
        }
        let refs: Vec<&[f64]> = groups.iter().map(|(_, v)| v.as_slice()).collect();
        let result = h::oneway_anova(&refs, self.spec.var_equal)
            .map_err(|e| HypoNodeError::Test(e.to_string()))?;
        emit_test_row(ctx, &result)
    }
}
