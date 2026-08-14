//! A pass-through / echo node: copies every upstream input to its output
//! unchanged. Useful as a no-op placeholder in DAG topology tests and
//! integration wiring checks.

use async_trait::async_trait;
use datafusion::common::HashMap;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::{
    dag::{DagError, DagNode, NodeInput, NodePorts, graph::PortOutputs},
    registry::{NodeCtx, NodeFactory},
};

/// A no-op transform node that echoes each upstream [`DataFrame`] to its
/// corresponding output port.
///
/// Every input arriving on port `N` is emitted on output port `N` with the
/// same [`DataFrame`], making it a transparent pass-through. Input port count
/// is not fixed (`set_fixed_input(false)`), so the scheduler accepts any
/// wiring.
#[derive(Clone)]
pub struct EchoNode {
    meta: NodePorts,
}

#[derive(Debug, JsonSchema, Deserialize)]
pub struct EchoNodeSpec {}

pub struct EchoNodeFactory {}

/// Static port layout: variadic input + variadic output.
fn port_layout() -> NodePorts {
    NodePorts::new()
        .add_output_port(None)
        .set_fixed_input(false)
}

impl NodeFactory for EchoNodeFactory {
    fn kind(&self) -> &'static str {
        "echo"
    }

    fn desc(&self) -> &'static str {
        "No-op pass-through: echoes each input DataFrame to the corresponding output port."
    }

    fn doc(&self) -> &'static str {
        "A no-op pass-through node that copies every upstream DataFrame to its \
        corresponding output port unchanged. Supports variadic inputs and outputs. \
        Useful as a placeholder in DAG topology tests and integration wiring checks."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(EchoNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let _spec: EchoNodeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(EchoNode::default()))
    }
}

impl Default for EchoNode {
    fn default() -> Self {
        Self {
            meta: port_layout(),
        }
    }
}

impl EchoNode {
    /// Create an [`EchoNode`] with a custom [`NodePorts`] layout.
    ///
    /// Useful when a test needs specific input/output port counts.
    pub fn from_ports(ports: NodePorts) -> Self {
        Self { meta: ports }
    }
}

#[async_trait]
impl DagNode for EchoNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "echo"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        _ctx: &dag_core::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let mut out: PortOutputs = HashMap::new();
        for inp in inputs {
            out.insert(inp.port, inp.data.clone());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    fn node_ctx() -> dag_core::registry::NodeCtx {
        dag_core::registry::NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            opendal: None,
            resources: std::sync::Arc::new(dag_core::resource_catalog::ResourceCatalog::new(
                std::path::PathBuf::from("."),
            )),
            global_sem: None,
        }
    }
    use super::*;

    use arrow_array::Int32Array;
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::prelude::SessionContext;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_echo_single_input() {
        let ctx = SessionContext::new();
        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Int32, false)]));
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![Arc::new(Int32Array::from(vec![1, 2, 3]))],
        )
        .unwrap();
        let df = ctx.read_batch(batch).unwrap();

        let mut node = EchoNode::default();
        let outputs = node
            .execute(
                &node_ctx(),
                &[NodeInput {
                    port: 0,
                    data: df.clone(),
                }],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let result = outputs.get(&0).unwrap().clone().collect().await.unwrap();
        assert_eq!(result[0].num_rows(), 3);
    }

    #[tokio::test]
    async fn test_echo_multi_input() {
        let ctx = SessionContext::new();
        let schema = Arc::new(Schema::new(vec![Field::new("v", DataType::Int32, false)]));

        let batch1 = arrow_array::RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int32Array::from(vec![10]))],
        )
        .unwrap();
        let batch2 = arrow_array::RecordBatch::try_new(
            schema,
            vec![Arc::new(Int32Array::from(vec![20, 30]))],
        )
        .unwrap();

        let df1 = ctx.read_batch(batch1).unwrap();
        let df2 = ctx.read_batch(batch2).unwrap();

        let mut node = EchoNode::default();
        let outputs = node
            .execute(
                &node_ctx(),
                &[
                    NodeInput { port: 0, data: df1 },
                    NodeInput { port: 1, data: df2 },
                ],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        assert_eq!(outputs.len(), 2);
        let r0 = outputs.get(&0).unwrap().clone().collect().await.unwrap();
        assert_eq!(r0[0].num_rows(), 1);
        let r1 = outputs.get(&1).unwrap().clone().collect().await.unwrap();
        assert_eq!(r1[0].num_rows(), 2);
    }
}
