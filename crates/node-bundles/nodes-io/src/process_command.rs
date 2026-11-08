//! `process`: a Nextflow-style shell task on the selected remote compute
//! target.
//!
//! This is the no-container execution path. The task payload is the engine's
//! own [`ProcessTaskSpec`] contract — an inline script or an argv, environment
//! bindings, and output glob patterns — so the scheduler dispatches it as a
//! `process` task and the compute registry routes it to whatever remote
//! backend is selected (direct SSH or Slurm). There is no local execution:
//! a task with host shell access is exactly what the local backend refuses.

use std::collections::BTreeMap;

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};
use dag_core::value::PortType;

/// Registered as `process`: the routing key every remote backend dispatches.
pub const PROCESS_COMMAND_KIND: &str = "process";

/// One declared output: files matching `pattern` (relative to the task
/// workspace) are collected as the output port `port`.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ProcessCommandOutput {
    /// Output port index; the declared ports must be exactly 0..n.
    pub port: u8,
    /// Glob relative to the task workspace, for example `out-*.txt`.
    pub pattern: String,
}

/// The spec doubles as the task payload: the engine parses the same JSON as
/// a process task, so the field names are fixed by that contract. Container
/// execution is deliberately absent — container workloads use the container
/// node kinds.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessCommandSpec {
    /// Inline shell script. `{{input.NAME}}` and `{{env.NAME}}` expand to the
    /// wired input paths and environment values. Mutually exclusive with
    /// `command`.
    pub script: Option<String>,
    /// Explicit argument vector; entries take the same placeholders as
    /// `script`. Mutually exclusive with `script`.
    #[serde(default)]
    pub command: Vec<String>,
    /// Environment bindings exported before the payload runs.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Output file contracts; at least one is required.
    pub outputs: Vec<ProcessCommandOutput>,
}

pub struct ProcessCommandNode {
    ports: NodePorts,
}

fn port_layout(spec: &ProcessCommandSpec) -> Result<NodePorts, String> {
    if spec.script.is_none() && spec.command.is_empty() {
        return Err("process requires `script` or `command`".into());
    }
    if spec.script.is_some() && !spec.command.is_empty() {
        return Err("process `script` and `command` are mutually exclusive".into());
    }
    if spec.outputs.is_empty() {
        return Err("process requires at least one output contract".into());
    }
    // The backends export these names verbatim (`command.env(name, value)`),
    // so a name that cannot exist in a POSIX environment would only fail at
    // task spawn; refuse it while the spec is still being validated.
    for name in spec.env.keys() {
        if name.is_empty()
            || name.contains('=')
            || name.chars().any(|character| character == '\0')
        {
            return Err(format!(
                "process env name `{name}` is not a valid environment variable name"
            ));
        }
    }
    let mut ports: Vec<&ProcessCommandOutput> = spec.outputs.iter().collect();
    ports.sort_by_key(|output| output.port);
    for (index, output) in ports.iter().enumerate() {
        if output.port != index as u8 {
            return Err(format!(
                "process output ports must be sequential from 0; saw {} at position {index}",
                output.port
            ));
        }
        if output.pattern.starts_with('/') || output.pattern.contains("..") {
            return Err(format!(
                "process output pattern must stay inside the task workspace: `{}`",
                output.pattern
            ));
        }
    }
    let mut layout = NodePorts::new()
        // Input port 0 is `f`: one optional file the task reads through
        // `{{input.f}}`. Additional wired ports are accepted and addressed as
        // `port_1`, `port_2`, …
        .add_optional_input_port_of_type(PortType::File);
    for _output in &ports {
        layout = layout.add_output_port_of_type(None, PortType::File);
    }
    Ok(layout)
}

impl ProcessCommandNode {
    fn new(spec: &ProcessCommandSpec) -> Result<Self, String> {
        Ok(Self {
            ports: port_layout(spec)?,
        })
    }
}

#[async_trait]
impl DagNode for ProcessCommandNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn kind(&self) -> &'static str {
        PROCESS_COMMAND_KIND
    }

    async fn execute(
        &mut self,
        _ctx: &NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        // The remote executor ignores the in-process payload entirely; the
        // local backend refuses process tasks by design. Reaching this arm
        // means neither routing decision was made, which is a configuration
        // error rather than a task failure.
        Err(DagError::Schedule(
            "process tasks have no local execution; select a remote compute target (direct SSH or Slurm)".into(),
        ))
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub struct ProcessCommandNodeFactory;

impl NodeFactory for ProcessCommandNodeFactory {
    fn kind(&self) -> &'static str {
        PROCESS_COMMAND_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs a shell script or argv as a task on the selected remote compute target."
    }

    fn doc(&self) -> &'static str {
        "A Nextflow-style process: an inline `script` (or an explicit `command` \
        argv), environment bindings, and output glob patterns. Inputs wired to \
        the node are staged into the task workspace on the remote host and \
        addressed as `{{input.f}}` for input port 0 (and `{{input.port_N}}` for \
        additional ports); `{{env.NAME}}` expands environment values. Each \
        output contract collects the files matching its pattern and publishes \
        them as a File output port with a real size+sha256 fingerprint. The \
        task runs on the selected remote compute target — a direct SSH host \
        or a Slurm cluster — and there is deliberately no local execution: \
        with the local target selected, the node is refused. Container \
        workloads use the container node kinds instead."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ProcessCommandSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_optional_input_port_of_type(PortType::File)
            .add_output_port_of_type(None, PortType::File)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: ProcessCommandSpec = serde_json::from_value(spec)?;
        let node = ProcessCommandNode::new(&spec).map_err(|error| {
            dag_core::registry::error::Error::Unknown(error)
        })?;
        Ok(Box::new(node))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let spec: ProcessCommandSpec = serde_json::from_value(spec)?;
        port_layout(&spec).map_err(dag_core::registry::error::Error::Unknown)
    }
}
