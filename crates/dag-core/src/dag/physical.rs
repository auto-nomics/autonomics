//! Physical-job provenance and the synthetic coordinator nodes.
//!
//! The compile path that expanded logical graphs into physical jobs was
//! removed with the logical authoring layer (stage-model flows in
//! `dag-engine` own cardinality now). What remains is load-bearing for
//! persisted state: [`PhysicalJobRef`] (job provenance on the runtime DAG),
//! and the synthetic `GatherNode` / `DynamicFanoutNode` kinds that stored
//! manifests rebuild on restore.

use std::collections::{BTreeMap, BTreeSet};

use datafusion::prelude::DataFrame;
use serde::{Deserialize, Serialize};

use super::error::DagError;
use super::graph::{DAG, PortOutputs};
use super::history::LogicalManifest;
use super::logical::LogicalGraph;
use super::{DagNode, NodeId, NodeInput, NodePorts};
use crate::dag::node_event::NodeReporter;
use crate::value::{NodeValue, PortType};

pub type Result<T> = std::result::Result<T, DagError>;

/// Provenance linking a runnable job back to the logical node that created it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalJobRef {
    pub logical_node: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub axis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<serde_json::Value>,
}

/// Collapse node produced by the retired logical compile path; persisted
/// manifests still restore it by kind `logical_gather`.
#[derive(Clone)]
pub struct GatherNode {
    ports: NodePorts,
}

impl Default for GatherNode {
    fn default() -> Self {
        Self {
            ports: NodePorts::new()
                .set_fixed_input(false)
                .add_output_port_of_type(None, PortType::Any),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for GatherNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        let mut inputs = inputs.to_vec();
        inputs.sort_by_key(|input| input.port);
        if inputs.is_empty() {
            return Err(DagError::Schedule(
                "logical gather requires at least one input".to_string(),
            ));
        }

        let mut output = PortOutputs::new();
        if inputs.iter().all(|input| input.data.as_dataframe().is_ok()) {
            let mut gathered: Option<DataFrame> = None;
            for input in inputs {
                let next = match input.data {
                    NodeValue::DataFrame(dataframe) => dataframe,
                    _ => unreachable!("checked above"),
                };
                gathered = Some(match gathered {
                    Some(current) => current.union(next).map_err(|error| {
                        DagError::Schedule(format!("cannot gather DataFrames: {error}"))
                    })?,
                    None => next,
                });
            }
            if let Some(gathered) = gathered {
                output.insert(0, NodeValue::DataFrame(gathered));
            }
        } else if inputs.iter().all(|input| input.data.as_file().is_ok()) {
            let mut files = Vec::new();
            for input in inputs {
                files.push(match input.data {
                    NodeValue::File(file) => file,
                    _ => unreachable!("checked above"),
                });
            }
            output.insert(0, NodeValue::FileSet(files));
        } else if inputs
            .iter()
            .all(|input| matches!(input.data, NodeValue::FileSet(_)))
        {
            let mut files = Vec::new();
            for input in inputs {
                let NodeValue::FileSet(mut next) = input.data else {
                    unreachable!("checked above");
                };
                files.append(&mut next);
            }
            output.insert(0, NodeValue::FileSet(files));
        } else {
            let kinds = inputs
                .iter()
                .map(|input| input.data.data_type().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(DagError::Schedule(format!(
                "logical gather requires homogeneous inputs, got: {kinds}"
            )));
        }
        Ok(output)
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "logical_gather"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Runtime placeholder for a logical dynamic fanout.
///
/// The scheduler consumes its upstream Channel, replaces this coordinator with
/// concrete physical jobs, and rewires the coordinator's downstream edges. It
/// is never intended to reach `execute`.
#[derive(Clone)]
pub struct DynamicFanoutNode {
    ports: NodePorts,
}

impl DynamicFanoutNode {
    fn new_ports() -> NodePorts {
        NodePorts::new()
            .add_input_port_of_type(None, PortType::Channel)
            .add_output_port_of_type(None, PortType::Any)
    }
}

impl Default for DynamicFanoutNode {
    fn default() -> Self {
        Self {
            ports: Self::new_ports(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for DynamicFanoutNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        Err(DagError::Schedule(
            "dynamic fanout coordinator must be expanded by the scheduler".into(),
        ))
    }

    fn kind(&self) -> &'static str {
        "dynamic_fanout"
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl DAG {
    pub fn physical_job(&self, id: &str) -> Option<&PhysicalJobRef> {
        self.physical_jobs.get(id)
    }

    pub fn logical_node_jobs(&self, logical_node: &str) -> Vec<&PhysicalJobRef> {
        let mut jobs = self
            .physical_jobs
            .values()
            .filter(|job| job.logical_node == logical_node)
            .collect::<Vec<_>>();
        jobs.sort_by(|left, right| left.item_key.cmp(&right.item_key));
        jobs
    }

    /// Restore persisted logical/physical metadata into an installed DAG.
    ///
    /// Physical nodes and edges must already have been installed. This method
    /// validates that every persisted provenance record points at an existing
    /// physical job and logical source node before replacing the in-memory
    /// layers.
    pub fn restore_layers(
        &mut self,
        logical_graphs: Vec<LogicalGraph>,
        physical_jobs: BTreeMap<NodeId, PhysicalJobRef>,
    ) -> Result<()> {
        self.restore_execution_layers(
            LogicalManifest {
                graphs: logical_graphs,
                ..LogicalManifest::default()
            },
            physical_jobs,
        )
    }

    /// Restore logical/physical provenance and execution directives.
    pub fn restore_execution_layers(
        &mut self,
        logical: LogicalManifest,
        physical_jobs: BTreeMap<NodeId, PhysicalJobRef>,
    ) -> Result<()> {
        let mut logical_ids = BTreeSet::new();
        for graph in &logical.graphs {
            graph.validate()?;
            for node in graph.nodes() {
                if !logical_ids.insert(node.id.as_str()) {
                    return Err(DagError::DuplicateNode(node.id.clone()));
                }
            }
        }
        for (id, job) in &physical_jobs {
            if !self.nodes.contains_key(id) {
                return Err(DagError::UnknownNode(id.clone()));
            }
            if !logical_ids.contains(job.logical_node.as_str()) {
                return Err(DagError::Schedule(format!(
                    "physical job `{id}` references missing logical node `{}`",
                    job.logical_node
                )));
            }
        }
        logical.default_task_resources.validate()?;
        for (logical_node, resources) in &logical.logical_task_resources {
            resources.validate()?;
            if !logical_ids.contains(logical_node.as_str()) {
                return Err(DagError::Schedule(format!(
                    "task resources reference missing logical node `{logical_node}`"
                )));
            }
        }
        for (physical_node, resources) in &logical.physical_task_resources {
            resources.validate()?;
            if !self.nodes.contains_key(physical_node) {
                return Err(DagError::UnknownNode(physical_node.clone()));
            }
        }
        self.logical_graphs = logical.graphs;
        self.physical_jobs = physical_jobs.into_iter().collect();
        self.default_task_resources = logical.default_task_resources;
        self.logical_task_resources = logical.logical_task_resources.into_iter().collect();
        self.task_resources = logical.physical_task_resources.into_iter().collect();
        Ok(())
    }
}
