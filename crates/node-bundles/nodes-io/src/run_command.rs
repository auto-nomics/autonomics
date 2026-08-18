//! Run an external file-to-file command as a DAG node.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;
use tokio::{io::AsyncReadExt, process::Command};

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::value::{FileRef, NodeValue, PortType};
use dag_core::{NodeCtx, NodeFactory};

const DEFAULT_TIMEOUT_SECS: u64 = 3600;
const MAX_CAPTURED_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum RunCommandError {
    #[error("invalid run_command spec: {0}")]
    Invalid(String),
    #[error("program `{program}` is not allowed by AUTONOMICS_ALLOWED_PROGRAMS")]
    ProgramNotAllowed { program: String },
    #[error("command exited with status {status}: {stderr}")]
    ExitStatus { status: i32, stderr: String },
    #[error("command was killed before completing within {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
    #[error("command could not be started: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("declared output `{path}` was not produced")]
    MissingOutput { path: String },
}

impl RunCommandError {
    fn to_dag_error(self) -> DagError {
        match self {
            Self::Spawn(source) => {
                DagError::Schedule(format!("run_command failed to start: {source}"))
            }
            other => DagError::NodeError {
                node_type: "run_command".into(),
                msg: other.to_string(),
            },
        }
    }
}

impl dag_core::dag::NodeError for RunCommandError {
    fn node_type(&self) -> &str {
        "run_command"
    }
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct CommandOutputSpec {
    /// Relative paths resolve against `workdir`; absolute paths are used as-is.
    pub path: String,
    pub format: Option<String>,
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct RunCommandNodeSpec {
    /// Executable passed directly to the operating system. No shell is used.
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<CommandOutputSpec>,
    /// Working directory. Relative output paths resolve against it. When
    /// omitted, the node creates a unique scratch directory.
    pub workdir: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub struct RunCommandNode {
    ports: NodePorts,
    program: String,
    args: Vec<String>,
    outputs: Vec<CommandOutputSpec>,
    workdir: Option<String>,
    timeout_secs: u64,
}

impl RunCommandNode {
    fn new(spec: RunCommandNodeSpec) -> Result<Self, RunCommandError> {
        if spec.program.trim().is_empty() {
            return Err(RunCommandError::Invalid("`program` cannot be empty".into()));
        }
        if spec.timeout_secs == 0 {
            return Err(RunCommandError::Invalid(
                "`timeout_secs` must be greater than zero".into(),
            ));
        }
        if spec.outputs.is_empty() {
            return Err(RunCommandError::Invalid(
                "at least one output must be declared".into(),
            ));
        }
        let mut ports = NodePorts::new()
            .set_fixed_input(false)
            .add_optional_input_port_of_type(PortType::File);
        for _ in &spec.outputs {
            ports = ports.add_output_port_of_type(None, PortType::File);
        }
        Ok(Self {
            ports,
            program: spec.program,
            args: spec.args,
            outputs: spec.outputs,
            workdir: spec.workdir,
            timeout_secs: spec.timeout_secs,
        })
    }
}

pub struct RunCommandNodeFactory;

fn port_layout() -> NodePorts {
    NodePorts::new()
        .set_fixed_input(false)
        .add_optional_input_port_of_type(PortType::File)
        .add_output_port_of_type(None, PortType::File)
}

impl NodeFactory for RunCommandNodeFactory {
    fn kind(&self) -> &'static str {
        "run_command"
    }

    fn desc(&self) -> &'static str {
        "Runs a file-to-file external command with typed File inputs and outputs."
    }

    fn doc(&self) -> &'static str {
        "Runs an external program without a shell. File inputs are bound to \
        `$input0`, `$input1`, and so on; declared output paths are bound to \
        `$output0`, `$output1`, and the working directory is bound to \
        `$workdir`. Relative output paths resolve against `workdir`, or a \
        unique scratch directory when no workdir is given. The command must \
        finish before `timeout_secs` and every declared output must exist."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RunCommandNodeSpec)
    }

    fn ports(&self) -> NodePorts {
        port_layout()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: RunCommandNodeSpec = serde_json::from_value(spec)?;
        let node = RunCommandNode::new(node_spec)
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        Ok(Box::new(node))
    }
}

fn input_path(value: &NodeValue) -> Result<String, RunCommandError> {
    match value {
        NodeValue::File(file) => Ok(file.path.clone()),
        NodeValue::FileSet(files) if !files.is_empty() => Ok(files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>()
            .join(",")),
        NodeValue::FileSet(_) => Err(RunCommandError::Invalid(
            "an empty FileSet cannot be bound to a command input".into(),
        )),
        NodeValue::DataFrame(_) => Err(RunCommandError::Invalid(
            "run_command inputs must be File or FileSet values".into(),
        )),
    }
}

fn unique_scratch_dir() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "autonomics-run-command-{}-{nanos}",
        std::process::id()
    ))
}

async fn capture<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        if bytes.len() < MAX_CAPTURED_OUTPUT_BYTES {
            let remaining = MAX_CAPTURED_OUTPUT_BYTES.saturating_sub(bytes.len());
            bytes.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    }
    Ok(bytes)
}

fn ensure_program_allowed(program: &str) -> Result<(), RunCommandError> {
    let Some(allowlist) = std::env::var("AUTONOMICS_ALLOWED_PROGRAMS").ok() else {
        return Ok(());
    };
    let allowed = allowlist
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .any(|item| item == program);
    if allowed {
        Ok(())
    } else {
        Err(RunCommandError::ProgramNotAllowed {
            program: program.to_string(),
        })
    }
}

fn lossy_prefix(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).to_string();
    if bytes.len() >= MAX_CAPTURED_OUTPUT_BYTES {
        format!("{text}\n[output truncated at 64 KiB]")
    } else {
        text
    }
}

#[async_trait]
impl DagNode for RunCommandNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            program: self.program.clone(),
            args: self.args.clone(),
            outputs: self.outputs.clone(),
            workdir: self.workdir.clone(),
            timeout_secs: self.timeout_secs,
        })
    }

    fn kind(&self) -> &'static str {
        "run_command"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        _ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        ensure_program_allowed(&self.program).map_err(RunCommandError::to_dag_error)?;

        let workdir = match &self.workdir {
            Some(path) => std::path::PathBuf::from(path),
            None => unique_scratch_dir(),
        };
        std::fs::create_dir_all(&workdir)
            .map_err(|e| {
                RunCommandError::Invalid(format!(
                    "cannot create workdir `{}`: {e}",
                    workdir.display()
                ))
            })
            .map_err(RunCommandError::to_dag_error)?;

        let mut sorted_inputs = inputs.to_vec();
        sorted_inputs.sort_by_key(|input| input.port);
        let input_paths: Vec<String> = sorted_inputs
            .iter()
            .map(|input| input_path(&input.data))
            .collect::<Result<_, _>>()
            .map_err(RunCommandError::to_dag_error)?;

        let output_paths: Vec<std::path::PathBuf> = self
            .outputs
            .iter()
            .map(|output| {
                let path = std::path::Path::new(&output.path);
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    workdir.join(path)
                }
            })
            .collect();

        let replacements = |index: usize| -> Vec<(String, String)> {
            let mut vars = Vec::new();
            if let Some(path) = input_paths.get(index) {
                vars.push(("$input".to_string(), path.clone()));
            }
            if let Some(path) = output_paths.get(index) {
                vars.push(("$output".to_string(), path.to_string_lossy().into_owned()));
            }
            vars
        };

        let mut args = Vec::with_capacity(self.args.len());
        for arg in &self.args {
            let mut rendered = arg.clone();
            for index in 0..input_paths.len().max(output_paths.len()) {
                for (prefix, value) in replacements(index) {
                    rendered = rendered.replace(&format!("{prefix}{index}"), &value);
                }
            }
            rendered = rendered.replace("$workdir", &workdir.to_string_lossy());
            args.push(rendered);
        }

        let mut child = Command::new(&self.program)
            .args(&args)
            .current_dir(&workdir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(RunCommandError::from)?;

        let mut stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let stdout_task = tokio::spawn(async move { capture(&mut stdout).await });
        let stderr_task = tokio::spawn(async move { capture(&mut stderr).await });

        let wait_result = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            child.wait(),
        )
        .await;
        let timed_out = wait_result.is_err();
        let status = match wait_result {
            Ok(status) => status.map_err(RunCommandError::from)?,
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                child.wait().await.map_err(RunCommandError::from)?
            }
        };

        let stdout_bytes = stdout_task
            .await
            .unwrap_or_else(|e| Err(std::io::Error::other(e)))
            .unwrap_or_default();
        let stderr_bytes = stderr_task
            .await
            .unwrap_or_else(|e| Err(std::io::Error::other(e)))
            .unwrap_or_default();
        let stdout_text = lossy_prefix(&stdout_bytes);
        let stderr_text = lossy_prefix(&stderr_bytes);

        if !stdout_text.trim().is_empty() {
            reporter.info(stdout_text);
        }
        if !stderr_text.trim().is_empty() {
            reporter.warn(stderr_text.clone());
        }

        if timed_out {
            return Err(RunCommandError::Timeout {
                timeout_secs: self.timeout_secs,
            }
            .to_dag_error());
        }

        if !status.success() {
            return Err(RunCommandError::ExitStatus {
                status: status.code().unwrap_or(-1),
                stderr: stderr_text,
            }
            .to_dag_error());
        }

        let mut outputs = PortOutputs::new();
        for (index, spec) in self.outputs.iter().enumerate() {
            let path = &output_paths[index];
            let file = FileRef::local(path, spec.format.clone())
                .map_err(|_| RunCommandError::MissingOutput {
                    path: path.to_string_lossy().into_owned(),
                })
                .map_err(RunCommandError::to_dag_error)?;
            outputs.insert_file(index as u8, file);
        }
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn ctx() -> NodeCtx {
        NodeCtx {
            runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
            opendal: None,
            global_sem: None,
        }
    }

    fn input_file(path: &std::path::Path) -> NodeInput {
        NodeInput::file(0, FileRef::local(path, Some("txt".into())).unwrap())
    }

    fn spec(program: &str, args: &[&str], output: &str) -> RunCommandNodeSpec {
        RunCommandNodeSpec {
            program: program.into(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            outputs: vec![CommandOutputSpec {
                path: output.into(),
                format: Some("txt".into()),
            }],
            workdir: None,
            timeout_secs: 10,
        }
    }

    #[tokio::test]
    async fn copies_file_input_to_declared_output() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.txt");
        std::fs::write(&input, "hello").unwrap();
        let output = dir.path().join("output.txt");
        let mut node_spec = spec("cp", &["$input0", "$output0"], "output.txt");
        node_spec.workdir = Some(dir.path().to_string_lossy().into_owned());
        let mut node = RunCommandNode::new(node_spec).unwrap();

        let outputs = node
            .execute(
                &ctx(),
                &[input_file(&input)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        assert!(output.is_file());
        assert_eq!(
            outputs.get(&0).unwrap().as_file().unwrap().path,
            output.to_string_lossy()
        );
    }

    #[tokio::test]
    async fn dag_bridge_runs_dataframe_file_command_and_back() {
        use crate::sink_file::{FileSinkNode, WriteFormat};
        use crate::source_file::{FileFormat, FileSourceNode};
        use dag_core::SinkMode;
        use dag_core::dag::graph::DAG;
        use dag_core::dag::runtime::SchedulerConfig;

        let dir = tempfile::tempdir().unwrap();
        let input_path = dir.path().join("input.csv");
        let intermediate_path = dir.path().join("intermediate.csv");
        let final_path = dir.path().join("final.csv");
        std::fs::write(&input_path, "id\n1\n2\n").unwrap();

        let mut command_spec = spec("cp", &["$input0", "$output0"], "final.csv");
        command_spec.workdir = Some(dir.path().to_string_lossy().into_owned());

        let mut dag = DAG::default();
        dag.add_node(
            "src".into(),
            Box::new(FileSourceNode::new(
                Some(input_path.to_string_lossy().into_owned()),
                Some(FileFormat::Csv),
            )),
        )
        .unwrap();
        dag.add_node(
            "sink".into(),
            Box::new(FileSinkNode::new(
                intermediate_path.to_string_lossy().into_owned(),
                WriteFormat::Csv,
                SinkMode::Overwrite,
            )),
        )
        .unwrap();
        dag.add_node(
            "copy".into(),
            Box::new(RunCommandNode::new(command_spec).unwrap()),
        )
        .unwrap();
        dag.add_node(
            "read".into(),
            Box::new(FileSourceNode::new(None, Some(FileFormat::Csv))),
        )
        .unwrap();

        dag.add_edge("src", "sink", 0, 0).unwrap();
        dag.add_edge("sink", "copy", 0, 0).unwrap();
        dag.add_edge("copy", "read", 0, 0).unwrap();

        let report = dag
            .run(&SchedulerConfig::default(), &ctx(), None)
            .await
            .unwrap();
        assert!(report.ok, "DAG failed: {:?}", report.errors);
        let outputs = dag.output("read").unwrap();
        assert_eq!(
            outputs.dataframe(0).unwrap().clone().count().await.unwrap(),
            2
        );
        assert!(final_path.is_file());
    }

    #[tokio::test]
    async fn missing_declared_output_fails() {
        let mut node = RunCommandNode::new(spec("true", &[], "missing.txt")).unwrap();
        let error = node
            .execute(
                &ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing.txt"));
    }

    #[tokio::test]
    async fn non_zero_exit_fails() {
        let mut node = RunCommandNode::new(spec("false", &[], "out.txt")).unwrap();
        let error = node
            .execute(
                &ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("exited with status"));
    }

    #[tokio::test]
    async fn timeout_fails() {
        let mut node = RunCommandNode::new(spec("sleep", &["10"], "out.txt")).unwrap();
        node.timeout_secs = 1;
        let error = node
            .execute(
                &ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("within 1s"));
    }
}
