//! A source node that loads built-in test datasets by name.
//!
//! Used in integration tests and registry-based wiring checks where a
//! real file source is unnecessary. Specify the dataset via
//! `{"dataset": "iris"}`.

use async_trait::async_trait;
use datafusion::common::HashMap;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use crate::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use crate::dataset::{BuiltinDataset, get_builtin_dataset};
use crate::node_registry::registry::{NodeCtx, NodeFactory};

#[derive(Debug, Error)]
pub enum TestSourceError {
    #[error("Unknown test dataset: {name}")]
    UnknownDataset { name: String },
    #[error("Failed to load dataset: {message}")]
    LoadFailed { message: String },
}

impl From<TestSourceError> for DagError {
    fn from(e: TestSourceError) -> Self {
        DagError::Schedule(e.to_string())
    }
}

/// Spec for [`TestSourceNode`]: identifies which built-in dataset to emit.
#[derive(Debug, JsonSchema, Deserialize)]
pub struct TestSourceSpec {
    /// Dataset name, e.g. `"iris"`.
    pub dataset: String,
}

/// A source node that emits a named built-in dataset on output port 0.
///
/// No input ports; one output port. The dataset is loaded at execution time
/// via [`get_builtin_dataset`].
#[derive(Clone)]
pub struct TestSourceNode {
    meta: NodePorts,
    dataset_name: String,
}

pub struct TestSourceFactory {}

/// Static port layout: no inputs, one untyped output.
fn port_layout() -> NodePorts {
    NodePorts::new().add_output_port(None)
}

impl NodeFactory for TestSourceFactory {
    fn kind(&self) -> &'static str {
        "test_source"
    }

    fn desc(&self) -> &'static str {
        "Loads a built-in test dataset by name for integration testing."
    }

    fn doc(&self) -> &'static str {
        "A testing source node that loads a built-in dataset by name (e.g. \
        \"iris\"). No input ports; one untyped output port. Intended for \
        integration tests and registry wiring checks where a real file source \
        is unnecessary."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TestSourceSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let node_spec: TestSourceSpec = serde_json::from_value(spec)?;
        let node = TestSourceNode::new(node_spec.dataset);
        Ok(Box::new(node))
    }
}

impl TestSourceNode {
    pub fn new(dataset_name: String) -> Self {
        Self {
            meta: port_layout(),
            dataset_name,
        }
    }
}

#[async_trait]
impl DagNode for TestSourceNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "test_source"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        _inputs: &[crate::dag::NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let builtin = match self.dataset_name.as_str() {
            "iris" => BuiltinDataset::Iris,
            other => {
                return Err(TestSourceError::UnknownDataset {
                    name: other.to_string(),
                }
                .into());
            }
        };

        let df = get_builtin_dataset(builtin).await;
        let mut res: PortOutputs = HashMap::new();
        res.insert(0, df);
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_loads_iris_dataset() {
        let mut node = TestSourceNode::new("iris".into());
        let outputs = node
            .execute(&[], &crate::dag::node_event::NodeReporter::noop())
            .await
            .unwrap();
        let batches = outputs.get(&0).unwrap().clone().collect().await.unwrap();
        // Iris has 150 data rows.
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 150);
    }

    #[tokio::test]
    async fn test_unknown_dataset_errors() {
        let mut node = TestSourceNode::new("nonexistent".into());
        let err = node
            .execute(&[], &crate::dag::node_event::NodeReporter::noop())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Unknown test dataset"), "got: {msg}");
    }
}
