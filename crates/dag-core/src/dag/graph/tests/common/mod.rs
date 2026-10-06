//! Shared fixtures for the graph module's tests: minimal node payloads,
//! a recording task executor, and small DAG builders.

use std::sync::Arc;

use crate::dag::error::DagError;
use crate::dag::graph::{DAG, PortOutputs};
use crate::dag::node_event::NodeReporter;
use crate::dag::{
    DagNode, LocalTaskExecutor, NodeInput, NodePorts, TaskExecution, TaskExecutor, TaskSubmission,
};
use crate::value::{FileRef, PortType};

// ── executors ──────────────────────────────────────────────────────────

pub(super) struct RecordingTaskExecutor {
    pub(super) submissions: Arc<std::sync::Mutex<Vec<TaskSubmission>>>,
}

#[async_trait::async_trait]
impl TaskExecutor for RecordingTaskExecutor {
    fn name(&self) -> &'static str {
        "recording"
    }

    async fn run(&self, execution: TaskExecution) -> crate::dag::node_event::JobResult {
        self.submissions
            .lock()
            .unwrap()
            .push(execution.submission.clone());
        let local_executor = LocalTaskExecutor::default();
        local_executor.run(execution).await
    }
}

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

/// A dynamic-fanout job body: publishes `/{{value}}.txt` on port 0.
#[derive(Clone)]
pub(super) struct DynamicItemNode {
    value: String,
    ports: NodePorts,
}

impl DynamicItemNode {
    pub(super) fn new(value: String) -> Self {
        Self {
            value,
            ports: NodePorts::new().add_output_port_of_type(None, PortType::Any),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for DynamicItemNode {
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
        outputs.insert(
            0,
            FileRef::new(format!("/{}.txt", self.value), Some("txt".into())),
        );
        Ok(outputs)
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "dynamic_item"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Sleeps for a configured duration and produces no output.
#[derive(Clone)]
pub(super) struct SleepingNode {
    duration: std::time::Duration,
    ports: NodePorts,
}

impl SleepingNode {
    pub(super) fn new(duration: std::time::Duration) -> Self {
        Self {
            duration,
            ports: NodePorts::new(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for SleepingNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        tokio::time::sleep(self.duration).await;
        Ok(PortOutputs::new())
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "sleeping_test_node"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Default)]
pub(super) struct ResourceTrace {
    pub(super) active: usize,
    pub(super) max_active: usize,
}

/// Tracks how many sibling executions are in flight at once.
#[derive(Clone)]
pub(super) struct ResourceTrackingNode {
    trace: Arc<std::sync::Mutex<ResourceTrace>>,
    ports: NodePorts,
}

impl ResourceTrackingNode {
    pub(super) fn new(trace: Arc<std::sync::Mutex<ResourceTrace>>) -> Self {
        Self {
            trace,
            ports: NodePorts::new(),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for ResourceTrackingNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        {
            let mut trace = self.trace.lock().unwrap();
            trace.active += 1;
            trace.max_active = trace.max_active.max(trace.active);
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        self.trace.lock().unwrap().active -= 1;
        Ok(PortOutputs::new())
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "resource_tracking_test_node"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Publishes a local file reference on port 0.
#[derive(Clone)]
pub(super) struct FileSourceNode {
    path: std::path::PathBuf,
    ports: NodePorts,
}

impl FileSourceNode {
    pub(super) fn new(path: std::path::PathBuf) -> Self {
        Self {
            path,
            ports: NodePorts::new().add_output_port_of_type(None, PortType::File),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for FileSourceNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        let file = FileRef::local(&self.path, Some("text".into()))?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, file);
        Ok(outputs)
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "file_source_test_node"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Records the paths of received file inputs into a shared vec.
#[derive(Clone)]
pub(super) struct InputPathProbeNode {
    paths: Arc<std::sync::Mutex<Vec<String>>>,
    ports: NodePorts,
}

impl InputPathProbeNode {
    pub(super) fn new(paths: Arc<std::sync::Mutex<Vec<String>>>) -> Self {
        Self {
            paths,
            ports: NodePorts::new().add_input_port_of_type(None, PortType::File),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for InputPathProbeNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        let mut paths = self.paths.lock().unwrap();
        for input in inputs {
            paths.push(input.data.as_file()?.path.clone());
        }
        Ok(PortOutputs::new())
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "input_path_probe_test_node"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A no-op node with caller-supplied port topology (for schema/port tests).
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

/// Declares a DataFrame output port but executes to an empty `PortOutputs`.
#[derive(Clone)]
pub(super) struct MisdeclaredOutputNode {
    ports: NodePorts,
}

impl Default for MisdeclaredOutputNode {
    fn default() -> Self {
        Self {
            ports: NodePorts::new().add_output_port(None),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for MisdeclaredOutputNode {
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
        outputs.insert_file(0, FileRef::new("/tmp/runtime-type-mismatch", None));
        Ok(outputs)
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "misdeclared_output"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
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

/// Writes and publishes two distinctly-formatted files on ports 0 and 1.
#[derive(Clone)]
pub(super) struct MultiFileOutputNode {
    paths: [std::path::PathBuf; 2],
    ports: NodePorts,
}

impl MultiFileOutputNode {
    pub(super) fn new(paths: [std::path::PathBuf; 2]) -> Self {
        Self {
            paths,
            ports: NodePorts::new()
                .add_output_port_of_type_with_label_and_format(
                    None,
                    PortType::File,
                    "sumstats",
                    "sumstats_gz",
                )
                .add_output_port_of_type_with_label_and_format(
                    None,
                    PortType::File,
                    "log",
                    "ldsc_log",
                ),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for MultiFileOutputNode {
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
        for (port, (path, format)) in [
            (&self.paths[0], "sumstats_gz"),
            (&self.paths[1], "ldsc_log"),
        ]
        .into_iter()
        .enumerate()
        {
            std::fs::write(path, format).map_err(|error| DagError::Schedule(error.to_string()))?;
            let file = FileRef::local(path, Some(format.into()))
                .map_err(|error| DagError::Schedule(error.to_string()))?;
            outputs.insert_file(port as u8, file);
        }
        Ok(outputs)
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "multi_file_output"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A node that reports execution evidence through the reporter side
/// channel, then optionally fails — mirroring how container nodes attach
/// image/exit-code/log evidence before returning.
#[derive(Clone)]
pub(super) struct DetailedNode {
    fail: bool,
    ports: NodePorts,
}

impl DetailedNode {
    pub(super) fn new(fail: bool) -> Self {
        Self {
            fail,
            ports: NodePorts::new().add_output_port_of_type(None, PortType::File),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for DetailedNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[NodeInput],
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        reporter.info("detailed node started");
        reporter.error("detailed node error stream");
        reporter.set_run_details(crate::dag::runtime::NodeRunDetails {
            image: Some("localhost/test@sha256:abc".into()),
            image_digest: Some("sha256:abc".into()),
            exit_code: Some(if self.fail { 42 } else { 0 }),
            run_name: Some("autonomics-container-command-1-1".into()),
            stdout_log: None,
            stderr_log: None,
            output_artifacts: Vec::new(),
            output_artifacts_by_port: Default::default(),
            workspace: None,
            task_manifest: None,
        });
        if self.fail {
            return Err(DagError::Schedule("intentional failure".into()));
        }
        let mut outputs = PortOutputs::new();
        outputs.insert(
            0,
            crate::value::NodeValue::File(FileRef {
                path: "/tmp/out.csv".into(),
                format: Some("csv".into()),
                fingerprint: None,
            }),
        );
        Ok(outputs)
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "detailed"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Writes a fresh run counter into its output file on every execution.
#[derive(Clone)]
pub(super) struct FileOutputNode {
    path: std::path::PathBuf,
    pub(super) runs: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ports: NodePorts,
}

impl FileOutputNode {
    pub(super) fn new(path: std::path::PathBuf) -> Self {
        Self {
            path,
            runs: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ports: NodePorts::new().add_output_port_of_type(None, PortType::File),
        }
    }
}

#[async_trait::async_trait]
impl DagNode for FileOutputNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    async fn execute(
        &mut self,
        _ctx: &crate::registry::NodeCtx,
        _inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        let run = self.runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        std::fs::write(&self.path, format!("run {run}"))
            .map_err(|e| DagError::Schedule(e.to_string()))?;
        let file =
            FileRef::local(&self.path, None).map_err(|e| DagError::Schedule(e.to_string()))?;
        let mut outputs = PortOutputs::new();
        outputs.insert_file(0, file);
        Ok(outputs)
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        "file_output"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// An [`EchoNode`] that counts how many times `execute` was called.
/// Shared counter lets tests assert exactly which nodes were skipped.
#[derive(Clone)]
pub(super) struct CountingEcho {
    meta: NodePorts,
    counter: Arc<std::sync::atomic::AtomicUsize>,
}

impl CountingEcho {
    pub(super) fn new(counter: Arc<std::sync::atomic::AtomicUsize>) -> Self {
        Self {
            meta: NodePorts::new()
                .add_output_port(None)
                .set_fixed_input(false),
            counter,
        }
    }
}

#[async_trait::async_trait]
impl DagNode for CountingEcho {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        "counting_echo"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &crate::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        self.counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut out: PortOutputs = PortOutputs::new();
        if inputs.is_empty() {
            // Source mode: publish a placeholder DataFrame on every
            // declared output port (mirroring EchoNode) so edges from
            // this node actually carry values — the identity chain that
            // drives incremental reuse rides on values, not topology.
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
                out.insert(port.index, crate::value::NodeValue::DataFrame(df));
            }
            return Ok(out);
        }
        for inp in inputs {
            out.insert(inp.port, inp.data.clone());
        }
        Ok(out)
    }
}

/// A counting node that reports a plugin identity, standing in for a
/// manifest-plugin node whose implementation lives outside kind + spec.
#[derive(Clone)]
pub(super) struct PluginBackedEcho {
    meta: NodePorts,
    counter: Arc<std::sync::atomic::AtomicUsize>,
    identity: crate::fingerprint::PluginIdentity,
}

impl PluginBackedEcho {
    pub(super) fn new(
        counter: Arc<std::sync::atomic::AtomicUsize>,
        identity: crate::fingerprint::PluginIdentity,
    ) -> Self {
        Self {
            meta: NodePorts::new().add_output_port(None),
            counter,
            identity,
        }
    }
}

#[async_trait::async_trait]
impl DagNode for PluginBackedEcho {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "plugin_echo"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn plugin_identity(&self) -> Option<&crate::fingerprint::PluginIdentity> {
        Some(&self.identity)
    }
    async fn execute(
        &mut self,
        ctx: &crate::registry::NodeCtx,
        inputs: &[NodeInput],
        _reporter: &NodeReporter,
    ) -> std::result::Result<PortOutputs, DagError> {
        self.counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut out: PortOutputs = PortOutputs::new();
        if inputs.is_empty() {
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
            out.insert(0, crate::value::NodeValue::DataFrame(df));
        }
        Ok(out)
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

/// A minimal `NodeCtx` for graph tests that only exercise scheduling with
/// ctx-ignoring nodes. The real engine ingredients are wired by
/// `DataEngine`; here a bare `RuntimeEnv` is sufficient.
pub(super) fn test_ctx() -> crate::registry::NodeCtx {
    use datafusion::prelude::SessionContext;
    crate::registry::NodeCtx::new(SessionContext::new().runtime_env(), None)
}

pub(super) fn make_schema(cols: &[(&str, arrow_schema::DataType)]) -> arrow_schema::SchemaRef {
    std::sync::Arc::new(arrow_schema::Schema::new(
        cols.iter()
            .map(|(n, t)| arrow_schema::Field::new(*n, t.clone(), true))
            .collect::<Vec<_>>(),
    ))
}

/// Shorthand to read an atomic counter's current value.
pub(super) fn cnt(ctr: &Arc<std::sync::atomic::AtomicUsize>) -> usize {
    ctr.load(std::sync::atomic::Ordering::SeqCst)
}

pub(super) fn incremental_cfg() -> crate::dag::runtime::SchedulerConfig {
    crate::dag::runtime::SchedulerConfig {
        incremental: true,
        ..crate::dag::runtime::SchedulerConfig::default()
    }
}
