//! Shared fixtures for the graph module's tests: minimal node payloads and
//! small DAG builders. (The scheduler-era executors and fanout/resource
//! fixtures left with the scheduler removal in 953049e3.)

use crate::dag::error::DagError;
use crate::dag::graph::{DAG, PortOutputs};
use crate::dag::node_event::NodeReporter;
use crate::dag::{DagNode, NodeInput, NodePorts};
use crate::value::{FileRef, PortType};

// ── node fixtures ──────────────────────────────────────────────────────

/// Minimal echo node for graph tests — passes through inputs unchanged.
#[derive(Clone)]
pub(super) struct EchoNode {
    meta: NodePorts,
}

impl Default for EchoNode {
    fn default() -> Self {
        Self {
            meta: NodePorts::new()
                .add_output_port(None)
                .set_fixed_input(false),
        }
    }
}

impl EchoNode {
    pub(super) fn from_ports(ports: NodePorts) -> Self {
        Self { meta: ports }
    }
}

#[async_trait::async_trait]
impl DagNode for EchoNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    async fn execute(
        &mut self,
        ctx: &crate::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        let mut outputs = PortOutputs::new();
        if inputs.is_empty() {
            // Source mode: a declared output port must publish a value,
            // otherwise downstream dispatch fails with
            // MissingUpstreamOutput. Emit a one-column placeholder.
            for port in self.meta.output_ports().iter() {
                let batch = arrow_array::RecordBatch::try_from_iter([(
                    "value",
                    std::sync::Arc::new(arrow_array::Int64Array::from(Vec::<i64>::new()))
                        as std::sync::Arc<dyn arrow_array::Array>,
                )])
                .map_err(|e| DagError::Schedule(e.to_string()))?;
                let df = ctx
                    .session()
                    .read_batch(batch)
                    .map_err(|e| DagError::Schedule(e.to_string()))?;
                outputs.insert(port.index, crate::value::NodeValue::DataFrame(df));
            }
            return Ok(outputs);
        }
        for inp in inputs {
            outputs.insert(inp.port, inp.data.clone());
        }
        Ok(outputs)
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
}

/// Node that reports arbitrary static ports.
#[derive(Clone)]
pub(super) struct PortedNode(pub(super) NodePorts);

#[async_trait::async_trait]
impl DagNode for PortedNode {
    fn ports(&self) -> &NodePorts {
        &self.0
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "ported"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        Ok(PortOutputs::new())
    }
}

/// Declares a sink path statically — the shape `dataframe_to_file` and
/// other file sinks expose through `sink_path`.
#[derive(Clone)]
pub(super) struct DeclaredSinkNode {
    path: String,
    ports: NodePorts,
}

impl DeclaredSinkNode {
    pub(super) fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            ports: NodePorts::new().add_output_port_of_type(None, PortType::File),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for DeclaredSinkNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, FileRef::new(self.path.clone(), None));
        Ok(outputs)
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "declared_sink"
    }

    fn sink_path(&self) -> Option<&str> {
        Some(&self.path)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Reads a file by configured path — the `file_reference` shape.
#[derive(Clone)]
pub(super) struct PathReaderNode {
    path: String,
    ports: NodePorts,
}

impl PathReaderNode {
    pub(super) fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            ports: NodePorts::new().add_output_port_of_type(None, PortType::File),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for PathReaderNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        Ok(PortOutputs::new())
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "path_reader"
    }

    fn referenced_file_paths(&self) -> Vec<String> {
        vec![self.path.clone()]
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ── helpers ────────────────────────────────────────────────────────────

pub(super) fn add(dag: &mut DAG, id: &str) {
    dag.add_node(id.into(), Box::new(EchoNode::default()))
        .unwrap();
}

/// A diamond DAG: a→b, a→c, b→d, c→d (all default ports).
pub(super) fn get_diamond_dag() -> DAG {
    let mut dag = DAG::default();
    for id in ["a", "b", "c", "d"] {
        add(&mut dag, id);
    }

    dag.add_edge("a", "b", 0, 0).unwrap();
    dag.add_edge("a", "c", 0, 0).unwrap();
    dag.add_edge("b", "d", 0, 0).unwrap();
    dag.add_edge("c", "d", 0, 0).unwrap();
    dag
}

pub(super) fn make_schema(cols: &[(&str, arrow_schema::DataType)]) -> arrow_schema::SchemaRef {
    std::sync::Arc::new(arrow_schema::Schema::new(
        cols.iter()
            .map(|(n, t)| arrow_schema::Field::new(*n, t.clone(), true))
            .collect::<Vec<_>>(),
    ))
}
