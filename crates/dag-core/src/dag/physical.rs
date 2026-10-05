//! Expanded physical execution graph produced from a logical workflow graph.
//!
//! A physical graph is intentionally close to the existing scheduler: every
//! [`PhysicalNode`] is one runnable job, every [`PhysicalEdge`] routes exactly
//! one output, and node ids are stable across runs. Logical provenance is
//! retained in [`PhysicalJobRef`] so reports and snapshots can explain why a
//! job exists.

use std::collections::{BTreeMap, BTreeSet};

use datafusion::prelude::DataFrame;
use serde::{Deserialize, Serialize};

use super::error::DagError;
use super::graph::{DAG, PortOutputs};
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

/// One concrete runtime job after logical expansion.
pub struct PhysicalNode {
    pub id: NodeId,
    pub job: PhysicalJobRef,
    pub kind: String,
    pub spec: serde_json::Value,
    pub node: Box<dyn DagNode>,
}

/// One concrete runtime edge after logical expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalEdge {
    pub from: NodeId,
    pub from_port: u8,
    pub to: NodeId,
    pub to_port: u8,
}

/// The scheduler-facing graph produced by logical expansion.
pub struct PhysicalGraph {
    nodes: Vec<PhysicalNode>,
    edges: Vec<PhysicalEdge>,
}

impl PhysicalGraph {
    pub(crate) fn new(nodes: Vec<PhysicalNode>, edges: Vec<PhysicalEdge>) -> Self {
        Self { nodes, edges }
    }

    pub fn nodes(&self) -> &[PhysicalNode] {
        &self.nodes
    }

    pub fn edges(&self) -> &[PhysicalEdge] {
        &self.edges
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn job_refs(&self) -> BTreeMap<NodeId, PhysicalJobRef> {
        self.nodes
            .iter()
            .map(|node| (node.id.clone(), node.job.clone()))
            .collect()
    }
}

/// Summary returned after installing a physical graph into a scheduler DAG.
#[derive(Debug, Clone, Serialize)]
pub struct PhysicalInstallReport {
    pub node_count: usize,
    pub edge_count: usize,
    pub jobs: BTreeMap<NodeId, PhysicalJobRef>,
}

/// Built-in logical gather operation.
///
/// Scatter jobs are joined into one DataFrame or one flattened FileSet. This
/// is a physical operator rather than a registry node: its input cardinality
/// is known only after logical expansion.
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
    /// Install a logical graph together with its already-expanded physical form.
    pub fn install_compiled_graph(
        &mut self,
        logical: LogicalGraph,
        graph: PhysicalGraph,
    ) -> Result<PhysicalInstallReport> {
        logical.validate()?;
        let jobs = graph.job_refs();
        let logical_ids = logical
            .nodes()
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        for job in jobs.values() {
            if !logical_ids.contains(job.logical_node.as_str()) {
                return Err(DagError::Schedule(format!(
                    "physical job provenance references missing logical node `{}`",
                    job.logical_node
                )));
            }
        }
        for existing_graph in &self.logical_graphs {
            for node in existing_graph.nodes() {
                if logical_ids.contains(node.id.as_str()) {
                    return Err(DagError::DuplicateNode(node.id.clone()));
                }
            }
        }

        let installed = self.install_physical_graph(graph)?;
        self.logical_graphs.push(logical);
        Ok(installed)
    }

    /// Install an already-expanded physical graph and retain its provenance.
    pub fn install_physical_graph(
        &mut self,
        graph: PhysicalGraph,
    ) -> Result<PhysicalInstallReport> {
        for node in graph.nodes() {
            if self.nodes.contains_key(&node.id) {
                return Err(DagError::DuplicateNode(node.id.clone()));
            }
        }

        let jobs = graph.job_refs();
        let edge_count = graph.edge_count();
        for node in graph.nodes {
            self.add_node_with_spec(node.id, node.node, node.kind, node.spec)?;
        }
        for edge in graph.edges {
            self.add_edge(edge.from, edge.to, edge.from_port, edge.to_port)?;
        }
        self.physical_jobs.extend(jobs.clone());

        let node_count = jobs.len();
        Ok(PhysicalInstallReport {
            node_count,
            edge_count,
            jobs,
        })
    }

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
        let mut logical_ids = BTreeSet::new();
        for graph in &logical_graphs {
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
        self.logical_graphs = logical_graphs;
        self.physical_jobs = physical_jobs.into_iter().collect();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileRef;
    use crate::dag::LogicalNode;

    #[derive(Clone)]
    struct SourceNode {
        meta: NodePorts,
    }

    impl Default for SourceNode {
        fn default() -> Self {
            Self {
                meta: NodePorts::new().add_output_port(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl DagNode for SourceNode {
        fn ports(&self) -> &NodePorts {
            &self.meta
        }

        async fn execute(
            &mut self,
            _ctx: &crate::registry::NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &crate::dag::node_event::NodeReporter,
        ) -> Result<PortOutputs> {
            Ok(PortOutputs::new())
        }

        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }

        fn kind(&self) -> &'static str {
            "test_source"
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    async fn execute_gather(inputs: Vec<NodeInput>) -> Result<PortOutputs> {
        let ctx = crate::registry::NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        );
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(1);
        let reporter = NodeReporter::new("gather", event_tx);
        GatherNode::default()
            .execute(&ctx, &inputs, &reporter)
            .await
    }

    #[tokio::test]
    async fn gathers_files_in_port_order() {
        let inputs = vec![
            NodeInput {
                port: 1,
                data: NodeValue::File(FileRef::new("/second.csv", Some("csv".into()))),
            },
            NodeInput {
                port: 0,
                data: NodeValue::File(FileRef::new("/first.csv", Some("csv".into()))),
            },
        ];
        let output = execute_gather(inputs).await.unwrap();
        let NodeValue::FileSet(files) = &output[&0] else {
            panic!("gather should produce a FileSet");
        };
        assert_eq!(files[0].path, "/first.csv");
        assert_eq!(files[1].path, "/second.csv");
    }

    #[tokio::test]
    async fn flattens_file_sets_in_port_order() {
        let inputs = vec![
            NodeInput {
                port: 2,
                data: NodeValue::FileSet(vec![FileRef::new("/third.csv", None)]),
            },
            NodeInput {
                port: 1,
                data: NodeValue::FileSet(vec![
                    FileRef::new("/first.csv", None),
                    FileRef::new("/second.csv", None),
                ]),
            },
        ];
        let output = execute_gather(inputs).await.unwrap();
        let NodeValue::FileSet(files) = &output[&0] else {
            panic!("gather should produce a flattened FileSet");
        };
        let paths = files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["/first.csv", "/second.csv", "/third.csv"]);
    }

    #[tokio::test]
    async fn rejects_mixed_file_payloads() {
        let inputs = vec![
            NodeInput {
                port: 0,
                data: NodeValue::File(FileRef::new("/first.csv", None)),
            },
            NodeInput {
                port: 1,
                data: NodeValue::FileSet(vec![FileRef::new("/second.csv", None)]),
            },
        ];
        let error = execute_gather(inputs).await.unwrap_err();
        assert!(error.to_string().contains("homogeneous inputs"));
    }

    #[test]
    fn deleting_one_scatter_job_keeps_source_graph_while_siblings_remain() {
        let logical = LogicalGraph::builder()
            .add_node(LogicalNode::for_each(
                "read",
                "test_source",
                serde_json::json!({"path": "{{item.path}}"}),
                "sample",
                vec![
                    serde_json::json!({"key": "a", "path": "/a.csv"}),
                    serde_json::json!({"key": "b", "path": "/b.csv"}),
                ],
            ))
            .build();
        let physical = logical
            .compile(|_, _| Ok(Box::new(SourceNode::default()) as Box<dyn DagNode>))
            .unwrap();
        let mut dag = DAG::default();
        dag.install_compiled_graph(logical, physical).unwrap();

        dag.delete_node("read#sample=a").unwrap();
        let manifest = dag.to_manifest();
        manifest.validate_layers().unwrap();
        dag.restore_layers(manifest.logical.graphs, manifest.physical_jobs)
            .unwrap();
        assert_eq!(dag.logical_graphs().len(), 1);
        assert_eq!(
            dag.physical_job("read#sample=b")
                .unwrap()
                .item_key
                .as_deref(),
            Some("b")
        );

        dag.delete_node("read#sample=b").unwrap();
        let manifest = dag.to_manifest();
        manifest.validate_layers().unwrap();
        assert!(manifest.logical.graphs.is_empty());
        assert!(manifest.physical_jobs.is_empty());
    }
}
