//! Run an external file-to-file command as a DAG node.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::path::{Component, Path, PathBuf};
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
    fn into_dag_error(self) -> DagError {
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
    /// In script mode, use an interpreter such as `bash`, `python`, or `Rscript`.
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Inline script source. When set, the node writes it to a private file and
    /// invokes `program` with that file as the first argument.
    #[serde(default)]
    pub script: Option<String>,
    /// Additional inline text files made available under `$AUTONOMICS_FILES_DIR`.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// Additional environment variables passed only to this process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
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
    script: Option<String>,
    files: BTreeMap<String, String>,
    env: BTreeMap<String, String>,
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
        if let Some(script) = &spec.script
            && script.contains('\0')
        {
            return Err(RunCommandError::Invalid(
                "`script` cannot contain NUL bytes".into(),
            ));
        }
        for (path, content) in &spec.files {
            validate_workspace_relative_path(path)?;
            if content.contains('\0') {
                return Err(RunCommandError::Invalid(format!(
                    "file `{path}` cannot contain NUL bytes"
                )));
            }
        }
        for name in spec.env.keys() {
            if name.is_empty() || name.contains('=') || name.contains('\0') {
                return Err(RunCommandError::Invalid(format!(
                    "invalid environment variable name `{name}`"
                )));
            }
            let reserved = name == "AUTONOMICS_WORKDIR"
                || name == "AUTONOMICS_SCRIPT"
                || name == "AUTONOMICS_FILES_DIR"
                || name.starts_with("AUTONOMICS_INPUT")
                || name.starts_with("AUTONOMICS_OUTPUT");
            if reserved {
                return Err(RunCommandError::Invalid(format!(
                    "environment variable `{name}` is reserved by run_command"
                )));
            }
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
            script: spec.script,
            files: spec.files,
            env: spec.env,
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
        `$workdir`. The same paths are exported as `AUTONOMICS_INPUT0`, \
        `AUTONOMICS_OUTPUT0`, and `AUTONOMICS_WORKDIR`. Set `script` to run \
        an inline Python/R/Bash script; set `files` to materialize additional \
        inline script assets under `AUTONOMICS_FILES_DIR`. Relative output \
        paths resolve against `workdir`, or a unique scratch directory when no \
        workdir is given. The command must finish before `timeout_secs` and \
        every declared output must exist."
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

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let node_spec: RunCommandNodeSpec = serde_json::from_value(spec)?;
        let node = RunCommandNode::new(node_spec)
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        Ok(node.ports)
    }
}

fn validate_workspace_relative_path(path: &str) -> Result<(), RunCommandError> {
    if path.is_empty() || path.contains('\0') {
        return Err(RunCommandError::Invalid(
            "file paths in `files` cannot be empty".into(),
        ));
    }
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || !candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(RunCommandError::Invalid(format!(
            "file path `{path}` must be a safe relative path"
        )));
    }
    Ok(())
}

fn write_strictly_within(
    base: &Path,
    relative: &str,
    content: &str,
) -> Result<PathBuf, RunCommandError> {
    validate_workspace_relative_path(relative)?;
    let destination = base.join(relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            RunCommandError::Invalid(format!(
                "cannot create directory `{}`: {e}",
                parent.display()
            ))
        })?;
    }
    std::fs::write(&destination, content).map_err(|e| {
        RunCommandError::Invalid(format!(
            "cannot write file `{}`: {e}",
            destination.display()
        ))
    })?;
    Ok(destination)
}

fn is_bash_program(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .is_some_and(|name| name == std::ffi::OsStr::new("bash"))
}

#[cfg(unix)]
fn kill_process_group(pid: u32) {
    // The child is started as a process-group leader. Killing the negative
    // pid also terminates interpreter/runtime processes spawned by a script.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: u32) {}

struct ProcessGroupGuard {
    pid: Option<u32>,
    armed: bool,
}

struct ResolvedOutput {
    local_path: PathBuf,
    /// Present when the declared absolute path is addressed through the
    /// engine's virtual filesystem rather than the host filesystem.
    virtual_path: Option<String>,
}

fn virtual_path(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix("vfs://") {
        return Some(vfs::OpendalFileStorage::normalize_path(rest));
    }
    if let Some(rest) = path.strip_prefix("file://") {
        return Some(vfs::OpendalFileStorage::normalize_path(rest));
    }
    path.starts_with('/')
        .then(|| vfs::OpendalFileStorage::normalize_path(path))
}

fn staged_path(base: &Path, prefix: &str, index: usize, source: &str) -> PathBuf {
    let extension = Path::new(source)
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| !extension.is_empty())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default();
    base.join(format!("{prefix}-{index}{extension}"))
}

async fn stage_input_file(
    ctx: &NodeCtx,
    staging_dir: &Path,
    index: usize,
    file: &FileRef,
) -> Result<PathBuf, RunCommandError> {
    let destination = staged_path(staging_dir, "input", index, &file.path);
    let virtual_source = ctx.opendal.as_ref().and_then(|_| virtual_path(&file.path));

    if let (Some(storage), Some(source)) = (ctx.opendal.as_ref(), virtual_source.as_deref()) {
        let operator = storage.resolve(source);
        let key = storage.resolve_path(source);
        if let Ok(bytes) = operator.read(&key).await {
            std::fs::write(&destination, bytes.to_vec()).map_err(|e| {
                RunCommandError::Invalid(format!(
                    "cannot stage input `{}`: {e}",
                    destination.display()
                ))
            })?;
            return Ok(destination);
        }
    }

    let host_path = Path::new(&file.path);
    if host_path.is_file() {
        std::fs::copy(host_path, &destination).map_err(|e| {
            RunCommandError::Invalid(format!("cannot stage input `{}`: {e}", host_path.display()))
        })?;
        return Ok(destination);
    }

    Err(RunCommandError::Invalid(format!(
        "input file `{}` was not found on the host or virtual filesystem",
        file.path
    )))
}

async fn stage_inputs(
    ctx: &NodeCtx,
    workdir: &Path,
    inputs: &[NodeInput],
) -> Result<Vec<NodeInput>, RunCommandError> {
    let staging_dir = workdir.join(".autonomics").join("inputs");
    std::fs::create_dir_all(&staging_dir).map_err(|e| {
        RunCommandError::Invalid(format!(
            "cannot create input staging directory `{}`: {e}",
            staging_dir.display()
        ))
    })?;

    let mut staged = Vec::with_capacity(inputs.len());
    let mut index = 0usize;
    for input in inputs {
        let value = match &input.data {
            NodeValue::File(file) => {
                let path = stage_input_file(ctx, &staging_dir, index, file).await?;
                index += 1;
                NodeValue::File(FileRef::new(
                    path.to_string_lossy().into_owned(),
                    file.format.clone(),
                ))
            }
            NodeValue::FileSet(files) => {
                let mut staged_files = Vec::with_capacity(files.len());
                for file in files {
                    let path = stage_input_file(ctx, &staging_dir, index, file).await?;
                    index += 1;
                    staged_files.push(FileRef::new(
                        path.to_string_lossy().into_owned(),
                        file.format.clone(),
                    ));
                }
                NodeValue::FileSet(staged_files)
            }
            NodeValue::DataFrame(_) => {
                return Err(RunCommandError::Invalid(
                    "run_command inputs must be File or FileSet values".into(),
                ));
            }
        };
        staged.push(NodeInput {
            port: input.port,
            data: value,
        });
    }
    Ok(staged)
}

fn resolve_output(ctx: &NodeCtx, workdir: &Path, index: usize, path: &str) -> ResolvedOutput {
    let declared = Path::new(path);
    if !declared.is_absolute() {
        return ResolvedOutput {
            local_path: workdir.join(declared),
            virtual_path: None,
        };
    }

    if ctx.opendal.is_some() && virtual_path(path).is_some() {
        let extension = declared
            .extension()
            .and_then(|extension| extension.to_str())
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!(".{extension}"))
            .unwrap_or_default();
        return ResolvedOutput {
            local_path: workdir
                .join(".autonomics")
                .join("outputs")
                .join(format!("output-{index}{extension}")),
            virtual_path: Some(path.to_string()),
        };
    }

    ResolvedOutput {
        local_path: declared.to_path_buf(),
        virtual_path: None,
    }
}

impl ProcessGroupGuard {
    fn new(pid: Option<u32>) -> Self {
        Self { pid, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if self.armed
            && let Some(pid) = self.pid
        {
            kill_process_group(pid);
        }
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
            script: self.script.clone(),
            files: self.files.clone(),
            env: self.env.clone(),
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
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        ensure_program_allowed(&self.program).map_err(RunCommandError::into_dag_error)?;

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
            .map_err(RunCommandError::into_dag_error)?;

        let mut sorted_inputs = stage_inputs(ctx, &workdir, inputs).await?;
        sorted_inputs.sort_by_key(|input| input.port);
        let input_paths: Vec<String> = sorted_inputs
            .iter()
            .map(|input| input_path(&input.data))
            .collect::<Result<_, _>>()
            .map_err(RunCommandError::into_dag_error)?;

        let resolved_outputs: Vec<ResolvedOutput> = self
            .outputs
            .iter()
            .enumerate()
            .map(|(index, output)| resolve_output(ctx, &workdir, index, &output.path))
            .collect();
        for output in &resolved_outputs {
            if let Some(parent) = output.local_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| {
                        RunCommandError::Invalid(format!(
                            "cannot create output directory `{}`: {e}",
                            parent.display()
                        ))
                    })
                    .map_err(RunCommandError::into_dag_error)?;
            }
        }
        let output_paths: Vec<&PathBuf> = resolved_outputs
            .iter()
            .map(|output| &output.local_path)
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

        let mut command = Command::new(&self.program);
        if self.script.is_some() {
            let script_dir = workdir.join(".autonomics");
            std::fs::create_dir_all(&script_dir)
                .map_err(|e| {
                    RunCommandError::Invalid(format!(
                        "cannot create script directory `{}`: {e}",
                        script_dir.display()
                    ))
                })
                .map_err(RunCommandError::into_dag_error)?;
            let script_path = write_strictly_within(
                &workdir,
                ".autonomics/script",
                self.script.as_deref().expect("checked"),
            )
            .map_err(RunCommandError::into_dag_error)?;
            let files_dir = workdir.join(".autonomics").join("files");
            std::fs::create_dir_all(&files_dir)
                .map_err(|e| {
                    RunCommandError::Invalid(format!(
                        "cannot create files directory `{}`: {e}",
                        files_dir.display()
                    ))
                })
                .map_err(RunCommandError::into_dag_error)?;
            for (relative, content) in &self.files {
                write_strictly_within(&files_dir, relative, content)
                    .map_err(RunCommandError::into_dag_error)?;
            }

            let mut script_args = Vec::with_capacity(self.args.len() + 3);
            if is_bash_program(&self.program) {
                script_args.push("--noprofile".to_string());
                script_args.push("--norc".to_string());
            }
            script_args.push(script_path.to_string_lossy().into_owned());
            script_args.extend(args.iter().cloned());
            args = script_args;
            command.env("AUTONOMICS_SCRIPT", script_path);
            command.env("AUTONOMICS_FILES_DIR", files_dir);
        }

        for (index, path) in input_paths.iter().enumerate() {
            command.env(format!("AUTONOMICS_INPUT{index}"), path);
        }
        for (index, path) in output_paths.iter().enumerate() {
            command.env(
                format!("AUTONOMICS_OUTPUT{index}"),
                path.to_string_lossy().into_owned(),
            );
        }
        command.env("AUTONOMICS_WORKDIR", &workdir);
        command.env("AUTONOMICS_INPUT_COUNT", input_paths.len().to_string());
        command.env("AUTONOMICS_OUTPUT_COUNT", output_paths.len().to_string());
        for (name, value) in &self.env {
            command.env(name, value);
        }

        #[cfg(unix)]
        command.process_group(0);
        command.kill_on_drop(true);

        let mut child = command
            .args(&args)
            .current_dir(&workdir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(RunCommandError::from)?;
        let process_pid = child.id();
        let mut process_guard = ProcessGroupGuard::new(process_pid);

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
                if let Some(pid) = child.id() {
                    kill_process_group(pid);
                }
                let _ = child.wait().await;
                let waited = child.wait().await.map_err(RunCommandError::from)?;
                process_guard.disarm();
                waited
            }
        };
        if let Some(pid) = process_pid {
            // A completed interpreter does not imply that every process it
            // spawned in the node's private process group has also exited.
            kill_process_group(pid);
        }
        process_guard.disarm();

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
            .into_dag_error());
        }

        if !status.success() {
            return Err(RunCommandError::ExitStatus {
                status: status.code().unwrap_or(-1),
                stderr: stderr_text,
            }
            .into_dag_error());
        }

        let mut outputs = PortOutputs::new();
        for (index, spec) in self.outputs.iter().enumerate() {
            let resolved = &resolved_outputs[index];
            let staged_file = FileRef::local(&resolved.local_path, spec.format.clone())
                .map_err(|_| RunCommandError::MissingOutput {
                    path: resolved.local_path.to_string_lossy().into_owned(),
                })
                .map_err(RunCommandError::into_dag_error)?;
            let file = if let (Some(storage), Some(virtual_path)) =
                (ctx.opendal.as_ref(), resolved.virtual_path.as_deref())
            {
                let bytes = std::fs::read(&resolved.local_path).map_err(|e| {
                    RunCommandError::Invalid(format!(
                        "cannot read staged output `{}`: {e}",
                        resolved.local_path.display()
                    ))
                })?;
                storage
                    .check_writable(virtual_path)
                    .map_err(|e| RunCommandError::Invalid(e.to_string()))?;
                storage
                    .resolve(virtual_path)
                    .write(&storage.resolve_path(virtual_path), bytes)
                    .await
                    .map_err(|e| {
                        RunCommandError::Invalid(format!(
                            "cannot write virtual output `{virtual_path}`: {e}"
                        ))
                    })?;
                FileRef::new(virtual_path, spec.format.clone())
            } else {
                staged_file
            };
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
            script: None,
            files: BTreeMap::new(),
            env: BTreeMap::new(),
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
    async fn script_mode_exports_paths_and_materializes_files() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.txt");
        std::fs::write(&input, "hello").unwrap();
        let node_spec = RunCommandNodeSpec {
            program: "bash".into(),
            args: Vec::new(),
            script: Some(
                r#"set -Eeuo pipefail
                {
                  printf 'input=%s\n' "$AUTONOMICS_INPUT0"
                  printf 'marker=%s\n' "$CUSTOM_MARKER"
                  printf 'workdir=%s\n' "$AUTONOMICS_WORKDIR"
                  cat "$AUTONOMICS_FILES_DIR/helper.txt"
                } > "$AUTONOMICS_OUTPUT0"
                "#
                .into(),
            ),
            files: BTreeMap::from([("helper.txt".into(), "helper=ok\n".into())]),
            env: BTreeMap::from([("CUSTOM_MARKER".into(), "42".into())]),
            outputs: vec![CommandOutputSpec {
                path: "result.txt".into(),
                format: Some("txt".into()),
            }],
            workdir: Some(dir.path().to_string_lossy().into_owned()),
            timeout_secs: 10,
        };
        let mut node = RunCommandNode::new(node_spec).unwrap();

        node.execute(
            &ctx(),
            &[input_file(&input)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        let result = std::fs::read_to_string(dir.path().join("result.txt")).unwrap();
        assert!(result.contains("marker=42"));
        assert!(result.contains("helper=ok"));
        assert!(result.contains("workdir="));
    }

    #[test]
    fn factory_resolves_dynamic_output_ports_from_spec() {
        let ports = RunCommandNodeFactory
            .ports_for_spec(serde_json::json!({
                "program": "bash",
                "script": "true",
                "outputs": [
                    {"path": "one.csv", "format": "csv"},
                    {"path": "two.tsv", "format": "tsv"}
                ]
            }))
            .unwrap();

        assert_eq!(ports.output_ports().len(), 2);
        assert!(
            ports
                .output_ports()
                .iter()
                .all(|port| port.data_type == PortType::File)
        );
    }

    #[test]
    fn script_files_reject_path_escape() {
        let error = match RunCommandNode::new(RunCommandNodeSpec {
            program: "bash".into(),
            args: Vec::new(),
            script: Some("true".into()),
            files: BTreeMap::from([("../escape.txt".into(), "bad".into())]),
            env: BTreeMap::new(),
            outputs: vec![CommandOutputSpec {
                path: "result.txt".into(),
                format: Some("txt".into()),
            }],
            workdir: None,
            timeout_secs: 10,
        }) {
            Err(error) => error,
            Ok(_) => panic!("path escape must be rejected"),
        };

        assert!(error.to_string().contains("safe relative path"));
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
