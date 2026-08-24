//! File-to-file container command node.
//!
//! Inputs are materialized into a private host scratch directory, that
//! directory is mounted at `/work`, the command runs in an ephemeral image,
//! and only declared output files become DAG values.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use thiserror::Error;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::value::{FileRef, NodeValue, PortType};
use dag_core::{NodeCtx, NodeFactory};

use container_runtime::{
    ContainerMount, ContainerRunRequest, ContainerRuntime, ContainerRuntimeError,
    DEFAULT_CONTAINER_WORKDIR, DEFAULT_TIMEOUT_SECS, PodmanRuntime, PullPolicy,
    unique_container_name,
};

pub const CONTAINER_COMMAND_KIND: &str = "container_command";

#[derive(Debug, Error)]
pub enum ContainerCommandError {
    #[error("invalid container_command spec: {0}")]
    Invalid(String),
    #[error(transparent)]
    Runtime(#[from] ContainerRuntimeError),
    #[error("declared output `{path}` was not produced")]
    MissingOutput { path: String },
}

impl ContainerCommandError {
    fn into_dag_error(self) -> DagError {
        DagError::NodeError {
            node_type: CONTAINER_COMMAND_KIND.into(),
            msg: self.to_string(),
        }
    }
}

impl dag_core::dag::NodeError for ContainerCommandError {
    fn node_type(&self) -> &str {
        CONTAINER_COMMAND_KIND
    }
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ContainerCommandOutputSpec {
    /// Safe path relative to `/work`. Absolute paths and `..` are rejected.
    pub path: String,
    /// Optional format label passed downstream (`bam`, `vcf`, `csv`, ...).
    pub format: Option<String>,
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ContainerCommandSpec {
    /// OCI image reference. Prefer immutable digests for production workflows.
    pub image: String,
    /// argv passed to the image. No shell is inserted.
    pub command: Vec<String>,
    /// Optional inline script. When set, its `/work/.autonomics/script` path is
    /// inserted immediately after `command[0]` (the interpreter).
    #[serde(default)]
    pub script: Option<String>,
    /// Inline text files materialized under `/work/.autonomics/files`.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// Extra environment variables inside the workload container.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Files that must exist after a successful container run.
    pub outputs: Vec<ContainerCommandOutputSpec>,
    /// Optional persistent scratch directory. A unique scratch directory is
    /// created when omitted.
    pub workdir: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Additional bind mounts. Reference data should normally be read-only.
    #[serde(default)]
    pub mounts: Vec<ContainerMount>,
    /// Podman network. The default is isolated (`none`); `host` or a named
    /// network must be justified by the tool.
    #[serde(default = "default_network")]
    pub network: String,
    /// Whether the container root filesystem is read-only. `/work` remains
    /// writable, and Podman supplies tmpfs mounts for `/tmp` and `/dev/shm`.
    #[serde(default = "default_true")]
    pub read_only_rootfs: bool,
    #[serde(default)]
    pub pull_policy: PullPolicy,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
    #[serde(default)]
    pub shm_size: Option<String>,
    /// Advanced override for images that must run as an internal user. The
    /// default maps the invoking rootless Podman user with `keep-id`.
    #[serde(default)]
    pub user: Option<String>,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn default_network() -> String {
    "none".into()
}

fn default_true() -> bool {
    true
}

pub struct ContainerCommandNode {
    ports: NodePorts,
    image: String,
    command: Vec<String>,
    script: Option<String>,
    files: BTreeMap<String, String>,
    env: BTreeMap<String, String>,
    outputs: Vec<ContainerCommandOutputSpec>,
    workdir: Option<String>,
    timeout_secs: u64,
    mounts: Vec<ContainerMount>,
    network: String,
    read_only_rootfs: bool,
    pull_policy: PullPolicy,
    cpus: Option<f64>,
    memory: Option<String>,
    pids_limit: Option<i64>,
    shm_size: Option<String>,
    user: Option<String>,
    runtime: Arc<dyn ContainerRuntime>,
}

impl ContainerCommandNode {
    pub fn new(
        spec: ContainerCommandSpec,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Result<Self, ContainerCommandError> {
        validate(&spec)?;
        let mut ports = NodePorts::new()
            .set_fixed_input(false)
            .add_optional_input_port_of_type(PortType::File);
        for _ in &spec.outputs {
            ports = ports.add_output_port_of_type(None, PortType::File);
        }
        Ok(Self {
            ports,
            image: spec.image,
            command: spec.command,
            script: spec.script,
            files: spec.files,
            env: spec.env,
            outputs: spec.outputs,
            workdir: spec.workdir,
            timeout_secs: spec.timeout_secs,
            mounts: spec.mounts,
            network: spec.network,
            read_only_rootfs: spec.read_only_rootfs,
            pull_policy: spec.pull_policy,
            cpus: spec.cpus,
            memory: spec.memory,
            pids_limit: spec.pids_limit,
            shm_size: spec.shm_size,
            user: spec.user,
            runtime,
        })
    }

    fn resolve_workdir(&self) -> Result<PathBuf, ContainerCommandError> {
        let workdir = match &self.workdir {
            Some(path) => PathBuf::from(path),
            None => unique_scratch_dir(),
        };
        std::fs::create_dir_all(&workdir).map_err(|e| {
            ContainerCommandError::Invalid(format!(
                "cannot create workdir `{}`: {e}",
                workdir.display()
            ))
        })?;
        let workdir = workdir.canonicalize().map_err(|e| {
            ContainerCommandError::Invalid(format!(
                "cannot resolve workdir `{}`: {e}",
                workdir.display()
            ))
        })?;
        if !workdir.is_dir() {
            return Err(ContainerCommandError::Invalid(format!(
                "workdir is not a directory: {}",
                workdir.display()
            )));
        }
        Ok(workdir)
    }

    fn resolve_mounts(&self) -> Result<Vec<ContainerMount>, ContainerCommandError> {
        self.mounts
            .iter()
            .map(|mount| {
                let host_path = Path::new(&mount.host_path);
                let canonical = host_path.canonicalize().map_err(|e| {
                    ContainerCommandError::Invalid(format!(
                        "cannot resolve mount `{}`: {e}",
                        host_path.display()
                    ))
                })?;
                Ok(ContainerMount {
                    host_path: canonical.to_string_lossy().into_owned(),
                    container_path: mount.container_path.clone(),
                    writable: mount.writable,
                })
            })
            .collect()
    }
}

fn validate_workspace_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.contains('\0') {
        return Err("file paths in `files` cannot be empty".into());
    }
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || !candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(format!("file path `{path}` must be a safe relative path"));
    }
    Ok(())
}

fn write_strictly_within(
    base: &Path,
    relative: &str,
    content: &str,
) -> Result<PathBuf, String> {
    validate_workspace_relative_path(relative)?;
    let destination = base.join(relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!("cannot create directory `{}`: {e}", parent.display())
        })?;
    }
    std::fs::write(&destination, content).map_err(|e| {
        format!("cannot write file `{}`: {e}", destination.display())
    })?;
    Ok(destination)
}

fn input_path(value: &NodeValue) -> Result<String, String> {
    match value {
        NodeValue::File(file) => Ok(file.path.clone()),
        NodeValue::FileSet(files) if !files.is_empty() => Ok(files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>()
            .join(",")),
        NodeValue::FileSet(_) => Err(
            "an empty FileSet cannot be bound to a command input".into(),
        ),
        NodeValue::DataFrame(_) => Err(
            "container_command inputs must be File or FileSet values".into(),
        ),
        NodeValue::Data(data) => Ok(data.vpath.clone()),
        NodeValue::DataSet(data) if !data.is_empty() => Ok(data
            .iter()
            .map(|data| data.vpath.as_str())
            .collect::<Vec<_>>()
            .join(",")),
        NodeValue::DataSet(_) => Err(
            "an empty DataSet cannot be bound to a command input".into(),
        ),
    }
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
) -> Result<PathBuf, String> {
    let destination = staged_path(staging_dir, "input", index, &file.path);
    let virtual_source = ctx.opendal.as_ref().and_then(|_| virtual_path(&file.path));

    if let (Some(storage), Some(source)) = (ctx.opendal.as_ref(), virtual_source.as_deref()) {
        let operator = storage.resolve(source);
        let key = storage.resolve_path(source);
        if let Ok(bytes) = operator.read(&key).await {
            std::fs::write(&destination, bytes.to_vec())
                .map_err(|e| format!("cannot stage input `{}`: {e}", destination.display()))?;
            return Ok(destination);
        }
    }

    let host_path = Path::new(&file.path);
    if host_path.is_file() {
        std::fs::copy(host_path, &destination)
            .map_err(|e| format!("cannot stage input `{}`: {e}", host_path.display()))?;
        return Ok(destination);
    }

    Err(format!(
        "input file `{}` was not found on the host or virtual filesystem",
        file.path
    ))
}

async fn stage_inputs(
    ctx: &NodeCtx,
    workdir: &Path,
    inputs: &[NodeInput],
) -> Result<Vec<NodeInput>, String> {
    let staging_dir = workdir.join(".autonomics").join("inputs");
    std::fs::create_dir_all(&staging_dir).map_err(|e| {
        format!(
            "cannot create input staging directory `{}`: {e}",
            staging_dir.display()
        )
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
            NodeValue::Data(data) => {
                let file = data.to_file_ref();
                let path = stage_input_file(ctx, &staging_dir, index, &file).await?;
                index += 1;
                NodeValue::File(FileRef {
                    path: path.to_string_lossy().into_owned(),
                    format: file.format,
                    fingerprint: file.fingerprint,
                })
            }
            NodeValue::DataSet(data) => {
                let mut staged_files = Vec::with_capacity(data.len());
                for data in data {
                    let file = data.to_file_ref();
                    let path = stage_input_file(ctx, &staging_dir, index, &file).await?;
                    index += 1;
                    staged_files.push(FileRef {
                        path: path.to_string_lossy().into_owned(),
                        format: file.format,
                        fingerprint: file.fingerprint,
                    });
                }
                NodeValue::FileSet(staged_files)
            }
            NodeValue::DataFrame(_) => {
                return Err(
                    "container_command inputs must be File or FileSet values".into(),
                );
            }
        };
        staged.push(NodeInput {
            port: input.port,
            data: value,
        });
    }
    Ok(staged)
}

fn validate(spec: &ContainerCommandSpec) -> Result<(), ContainerCommandError> {
    if spec.image.trim().is_empty() {
        return Err(ContainerCommandError::Invalid(
            "`image` cannot be empty".into(),
        ));
    }
    if spec.command.is_empty() {
        return Err(ContainerCommandError::Invalid(
            "`command` cannot be empty".into(),
        ));
    }
    if spec.timeout_secs == 0 {
        return Err(ContainerCommandError::Invalid(
            "`timeout_secs` must be greater than zero".into(),
        ));
    }
    if spec.outputs.is_empty() {
        return Err(ContainerCommandError::Invalid(
            "at least one output must be declared".into(),
        ));
    }
    if spec.network.trim().is_empty() {
        return Err(ContainerCommandError::Invalid(
            "`network` cannot be empty".into(),
        ));
    }
    for output in &spec.outputs {
        validate_workspace_relative_path(&output.path).map_err(|e| {
            ContainerCommandError::Invalid(format!("invalid output `{}`: {e}", output.path))
        })?;
        if let Some(format) = &output.format
            && format.contains('\0')
        {
            return Err(ContainerCommandError::Invalid(format!(
                "output format `{format}` cannot contain NUL bytes"
            )));
        }
    }
    if let Some(script) = &spec.script
        && script.contains('\0')
    {
        return Err(ContainerCommandError::Invalid(
            "`script` cannot contain NUL bytes".into(),
        ));
    }
    for (path, content) in &spec.files {
        validate_workspace_relative_path(path)
            .map_err(ContainerCommandError::Invalid)?;
        if content.contains('\0') {
            return Err(ContainerCommandError::Invalid(format!(
                "file `{path}` cannot contain NUL bytes"
            )));
        }
    }
    for (name, value) in &spec.env {
        if name.is_empty() || name.contains('=') || name.contains('\0') || value.contains('\0') {
            return Err(ContainerCommandError::Invalid(format!(
                "invalid environment variable `{name}`"
            )));
        }
        let reserved = name == "AUTONOMICS_WORKDIR"
            || name == "AUTONOMICS_SCRIPT"
            || name == "AUTONOMICS_FILES_DIR"
            || name.starts_with("AUTONOMICS_INPUT")
            || name.starts_with("AUTONOMICS_OUTPUT");
        if reserved {
            return Err(ContainerCommandError::Invalid(format!(
                "environment variable `{name}` is reserved by container_command"
            )));
        }
    }
    if let Some(cpus) = spec.cpus
        && cpus <= 0.0
    {
        return Err(ContainerCommandError::Invalid(
            "`cpus` must be positive".into(),
        ));
    }
    if let Some(pids_limit) = spec.pids_limit
        && pids_limit <= 0
    {
        return Err(ContainerCommandError::Invalid(
            "`pids_limit` must be positive".into(),
        ));
    }
    let mut container_paths = std::collections::BTreeSet::new();
    for mount in &spec.mounts {
        let host = Path::new(&mount.host_path);
        let container = Path::new(&mount.container_path);
        if !host.is_absolute()
            || !container.is_absolute()
            || mount.host_path.contains('\0')
            || mount.container_path.contains('\0')
        {
            return Err(ContainerCommandError::Invalid(format!(
                "mount paths must be absolute: `{}` -> `{}`",
                mount.host_path, mount.container_path
            )));
        }
        if container == Path::new("/") || container == Path::new(DEFAULT_CONTAINER_WORKDIR) {
            return Err(ContainerCommandError::Invalid(
                "mounts cannot target `/` or the node workdir".into(),
            ));
        }
        if !container_paths.insert(mount.container_path.clone()) {
            return Err(ContainerCommandError::Invalid(format!(
                "duplicate container mount path `{}`",
                mount.container_path
            )));
        }
    }
    Ok(())
}

#[async_trait]
impl DagNode for ContainerCommandNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            ports: self.ports.clone(),
            image: self.image.clone(),
            command: self.command.clone(),
            script: self.script.clone(),
            files: self.files.clone(),
            env: self.env.clone(),
            outputs: self.outputs.clone(),
            workdir: self.workdir.clone(),
            timeout_secs: self.timeout_secs,
            mounts: self.mounts.clone(),
            network: self.network.clone(),
            read_only_rootfs: self.read_only_rootfs,
            pull_policy: self.pull_policy,
            cpus: self.cpus,
            memory: self.memory.clone(),
            pids_limit: self.pids_limit,
            shm_size: self.shm_size.clone(),
            user: self.user.clone(),
            runtime: Arc::clone(&self.runtime),
        })
    }

    fn kind(&self) -> &'static str {
        CONTAINER_COMMAND_KIND
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
        let host_workdir = self
            .resolve_workdir()
            .map_err(ContainerCommandError::into_dag_error)?;
        let mounts = self
            .resolve_mounts()
            .map_err(ContainerCommandError::into_dag_error)?;

        let mut staged_inputs = stage_inputs(ctx, &host_workdir, inputs)
            .await
            .map_err(ContainerCommandError::Invalid)
            .map_err(ContainerCommandError::into_dag_error)?;
        staged_inputs.sort_by_key(|input| input.port);
        let host_input_paths = staged_inputs
            .iter()
            .map(|input| input_path(&input.data))
            .collect::<Result<Vec<_>, _>>()
            .map_err(ContainerCommandError::Invalid)
            .map_err(ContainerCommandError::into_dag_error)?;
        let container_input_paths = host_input_paths
            .iter()
            .map(|path| container_path(&host_workdir, path))
            .collect::<Vec<_>>();

        let resolved_outputs = self
            .outputs
            .iter()
            .map(|output| (output, host_workdir.join(&output.path)))
            .collect::<Vec<_>>();
        for (_, path) in &resolved_outputs {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| {
                        ContainerCommandError::Invalid(format!(
                            "cannot create output directory `{}`: {e}",
                            parent.display()
                        ))
                    })
                    .map_err(ContainerCommandError::into_dag_error)?;
            }
        }
        let container_output_paths = resolved_outputs
            .iter()
            .map(|(spec, path)| (spec, container_path(&host_workdir, &path.to_string_lossy())))
            .collect::<Vec<_>>();

        let mut command = self.command.clone();
        if let Some(script) = &self.script {
            let script_path = write_strictly_within(&host_workdir, ".autonomics/script", script)
                .map_err(ContainerCommandError::Invalid)
                .map_err(ContainerCommandError::into_dag_error)?;
            let files_dir = host_workdir.join(".autonomics/files");
            std::fs::create_dir_all(&files_dir)
                .map_err(|e| {
                    ContainerCommandError::Invalid(format!(
                        "cannot create files directory `{}`: {e}",
                        files_dir.display()
                    ))
                })
                .map_err(ContainerCommandError::into_dag_error)?;
            for (relative, content) in &self.files {
                write_strictly_within(&files_dir, relative, content)
                    .map_err(ContainerCommandError::Invalid)
                    .map_err(ContainerCommandError::into_dag_error)?;
            }
            let script_arg = container_path(&host_workdir, &script_path.to_string_lossy());
            command.insert(1, script_arg);
        }

        let replacements = |index: usize| -> Vec<(String, String)> {
            let mut vars = Vec::new();
            if let Some(path) = container_input_paths.get(index) {
                vars.push(("$input".to_string(), path.clone()));
            }
            if let Some((_, path)) = container_output_paths.get(index) {
                vars.push(("$output".to_string(), path.clone()));
            }
            vars
        };
        let count = container_input_paths
            .len()
            .max(container_output_paths.len());
        for item in &mut command {
            for index in (0..count).rev() {
                for (prefix, value) in replacements(index) {
                    *item = item.replace(&format!("{prefix}{index}"), &value);
                }
            }
            *item = item.replace("$workdir", DEFAULT_CONTAINER_WORKDIR);
        }

        let mut env = self.env.clone().into_iter().collect::<Vec<_>>();
        for (index, path) in container_input_paths.iter().enumerate() {
            env.push((format!("AUTONOMICS_INPUT{index}"), path.clone()));
        }
        for (index, (_, path)) in container_output_paths.iter().enumerate() {
            env.push((format!("AUTONOMICS_OUTPUT{index}"), path.clone()));
        }
        env.push((
            "AUTONOMICS_WORKDIR".into(),
            DEFAULT_CONTAINER_WORKDIR.into(),
        ));
        env.push((
            "AUTONOMICS_INPUT_COUNT".into(),
            container_input_paths.len().to_string(),
        ));
        env.push((
            "AUTONOMICS_OUTPUT_COUNT".into(),
            container_output_paths.len().to_string(),
        ));
        if self.script.is_some() {
            env.push((
                "AUTONOMICS_SCRIPT".into(),
                format!("{DEFAULT_CONTAINER_WORKDIR}/.autonomics/script"),
            ));
            env.push((
                "AUTONOMICS_FILES_DIR".into(),
                format!("{DEFAULT_CONTAINER_WORKDIR}/.autonomics/files"),
            ));
        }

        let request = ContainerRunRequest {
            image: self.image.clone(),
            command,
            host_workdir,
            container_workdir: DEFAULT_CONTAINER_WORKDIR.into(),
            env,
            mounts,
            network: self.network.clone(),
            read_only_rootfs: self.read_only_rootfs,
            pull_policy: self.pull_policy,
            cpus: self.cpus,
            memory: self.memory.clone(),
            pids_limit: self.pids_limit,
            shm_size: self.shm_size.clone(),
            user: self.user.clone(),
            timeout_secs: self.timeout_secs,
            name: unique_container_name(),
        };

        reporter.info(format!(
            "starting container {} via {}",
            request.image,
            self.runtime.name()
        ));
        let result = self
            .runtime
            .run(request)
            .await
            .map_err(ContainerCommandError::from)
            .map_err(ContainerCommandError::into_dag_error)?;
        if !result.stdout.trim().is_empty() {
            reporter.info(result.stdout);
        }
        if !result.stderr.trim().is_empty() {
            reporter.warn(result.stderr);
        }

        let mut outputs = PortOutputs::new();
        for (index, (spec, host_path)) in resolved_outputs.iter().enumerate() {
            if !host_path.is_file() {
                return Err(ContainerCommandError::MissingOutput {
                    path: host_path.to_string_lossy().into_owned(),
                }
                .into_dag_error());
            }
            let file = FileRef::local(host_path, spec.format.clone())
                .map_err(|_| ContainerCommandError::MissingOutput {
                    path: host_path.to_string_lossy().into_owned(),
                })
                .map_err(ContainerCommandError::into_dag_error)?;
            outputs.insert_file(index as u8, file);
        }
        Ok(outputs)
    }
}

fn container_path(host_workdir: &Path, host_path: &str) -> String {
    let path = Path::new(host_path);
    let relative = path.strip_prefix(host_workdir).unwrap_or(path);
    Path::new(DEFAULT_CONTAINER_WORKDIR)
        .join(relative)
        .to_string_lossy()
        .into_owned()
}

fn unique_scratch_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "autonomics-container-command-{}-{nanos}",
        std::process::id()
    ))
}

pub struct ContainerCommandNodeFactory;

impl NodeFactory for ContainerCommandNodeFactory {
    fn kind(&self) -> &'static str {
        CONTAINER_COMMAND_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs a file-to-file external command in an ephemeral Podman container."
    }

    fn doc(&self) -> &'static str {
        "Runs an OCI image without a shell. File inputs are staged into a private \
        host scratch directory mounted at `/work`; input paths are exposed as \
        `$input0`, `$input1`, `AUTONOMICS_INPUT0`, and so on. Outputs must be \
        safe paths relative to `/work` and are exposed as `$output0`, \
        `AUTONOMICS_OUTPUT0`, and so on. The container root filesystem is \
        read-only by default, networking defaults to `none`, and the invoking \
        rootless Podman user is mapped with `--userns=keep-id`. Use \
        `mounts` for explicit read-only reference data. Production workflows \
        should reference images by digest."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ContainerCommandSpec)
    }

    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .set_fixed_input(false)
            .add_optional_input_port_of_type(PortType::File)
            .add_output_port_of_type(None, PortType::File)
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: ContainerCommandSpec = serde_json::from_value(spec)?;
        let node = ContainerCommandNode::new(node_spec, Arc::new(PodmanRuntime::from_env()))
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        Ok(Box::new(node))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let node_spec: ContainerCommandSpec = serde_json::from_value(spec)?;
        let node = ContainerCommandNode::new(node_spec, Arc::new(PodmanRuntime::from_env()))
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        Ok(node.ports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use container_runtime::ContainerRunResult;

    fn ctx() -> NodeCtx {
        NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            None,
        )
    }

    fn input_file(path: &Path) -> NodeInput {
        NodeInput::file(0, FileRef::local(path, Some("txt".into())).unwrap())
    }

    #[derive(Default)]
    struct FakeRuntime {
        requests: Mutex<Vec<ContainerRunRequest>>,
    }

    fn host_path(request: &ContainerRunRequest, container_path: &str) -> PathBuf {
        let relative = Path::new(container_path)
            .strip_prefix(DEFAULT_CONTAINER_WORKDIR)
            .expect("path is inside /work");
        request.host_workdir.join(relative)
    }

    #[async_trait]
    impl ContainerRuntime for FakeRuntime {
        async fn run(
            &self,
            request: ContainerRunRequest,
        ) -> Result<ContainerRunResult, ContainerRuntimeError> {
            let output = request
                .env
                .iter()
                .find(|(key, _)| key == "AUTONOMICS_OUTPUT0")
                .map(|(_, value)| value.clone())
                .ok_or_else(|| ContainerRuntimeError::Invalid("missing output binding".into()))?;
            let input = request
                .env
                .iter()
                .find(|(key, _)| key == "AUTONOMICS_INPUT0")
                .map(|(_, value)| value.clone());
            let host_output = host_path(&request, &output);
            if let Some(input) = input {
                let host_input = host_path(&request, &input);
                std::fs::copy(host_input, host_output).unwrap();
            } else {
                std::fs::write(host_output, "container-result").unwrap();
            }
            self.requests.lock().unwrap().push(request);
            Ok(ContainerRunResult {
                exit_code: 0,
                stdout: "container stdout".into(),
                stderr: String::new(),
            })
        }

        fn name(&self) -> &'static str {
            "fake"
        }
    }

    fn spec(image: &str, command: Vec<String>, output: &str) -> ContainerCommandSpec {
        ContainerCommandSpec {
            image: image.into(),
            command,
            script: None,
            files: BTreeMap::new(),
            env: BTreeMap::new(),
            outputs: vec![ContainerCommandOutputSpec {
                path: output.into(),
                format: Some("txt".into()),
            }],
            workdir: None,
            timeout_secs: 10,
            mounts: Vec::new(),
            network: default_network(),
            read_only_rootfs: true,
            pull_policy: PullPolicy::Missing,
            cpus: None,
            memory: None,
            pids_limit: None,
            shm_size: None,
            user: None,
        }
    }

    #[tokio::test]
    async fn stages_inputs_and_collects_declared_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.txt");
        std::fs::write(&input, "hello-container").unwrap();
        let runtime = Arc::new(FakeRuntime::default());
        let mut node_spec = spec(
            "quay.io/example/tool@sha256:abcdef",
            vec![
                "tool".into(),
                "--input".into(),
                "$input0".into(),
                "--output".into(),
                "$output0".into(),
            ],
            "result.txt",
        );
        node_spec.workdir = Some(dir.path().to_string_lossy().into_owned());
        let mut node = ContainerCommandNode::new(node_spec, runtime.clone()).unwrap();

        let outputs = node
            .execute(
                &ctx(),
                &[input_file(&input)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let requests = runtime.requests.lock().unwrap();
        let request = requests.last().unwrap();
        assert!(request.command.contains(&"--input".into()));
        assert!(request.command.contains(&format!(
            "{DEFAULT_CONTAINER_WORKDIR}/.autonomics/inputs/input-0.txt"
        )));
        assert!(
            request
                .command
                .contains(&format!("{DEFAULT_CONTAINER_WORKDIR}/result.txt"))
        );
        assert_eq!(request.network, "none");
        assert!(request.read_only_rootfs);
        assert!(
            request
                .host_workdir
                .ends_with(dir.path().file_name().unwrap())
        );

        let output = dir.path().join("result.txt");
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "hello-container");
        assert_eq!(
            outputs.get(&0).unwrap().as_file().unwrap().path,
            output.canonicalize().unwrap().to_string_lossy()
        );
    }

    #[tokio::test]
    async fn script_mode_materializes_private_assets() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Arc::new(FakeRuntime::default());
        let mut node_spec = spec(
            "quay.io/example/bash",
            vec!["bash".into(), "-e".into()],
            "script-result.txt",
        );
        node_spec.workdir = Some(dir.path().to_string_lossy().into_owned());
        node_spec.script = Some("cat \"$AUTONOMICS_INPUT0\" > \"$AUTONOMICS_OUTPUT0\"".into());
        node_spec.files.insert("helper.txt".into(), "helper".into());
        let mut node = ContainerCommandNode::new(node_spec, runtime.clone()).unwrap();

        node.execute(
            &ctx(),
            &[],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        assert!(dir.path().join(".autonomics/script").is_file());
        assert!(dir.path().join(".autonomics/files/helper.txt").is_file());
        let requests = runtime.requests.lock().unwrap();
        assert!(
            requests
                .last()
                .unwrap()
                .command
                .contains(&format!("{DEFAULT_CONTAINER_WORKDIR}/.autonomics/script"))
        );
    }

    #[test]
    fn rejects_output_path_escape() {
        let error = match ContainerCommandNode::new(
            spec("tool", vec!["tool".into()], "../escape.txt"),
            Arc::new(FakeRuntime::default()),
        ) {
            Ok(_) => panic!("output path escape must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("safe relative path"));
    }

    #[test]
    fn factory_resolves_dynamic_output_ports() {
        let ports = ContainerCommandNodeFactory
            .ports_for_spec(serde_json::json!({
                "image": "quay.io/example/tool:1",
                "command": ["tool"],
                "outputs": [
                    {"path": "one.txt", "format": "txt"},
                    {"path": "two.csv", "format": "csv"}
                ]
            }))
            .unwrap();

        assert_eq!(ports.output_ports().len(), 2);
    }

    #[tokio::test]
    async fn missing_output_fails_after_successful_exit() {
        struct MissingOutputRuntime;
        #[async_trait]
        impl ContainerRuntime for MissingOutputRuntime {
            async fn run(
                &self,
                _request: ContainerRunRequest,
            ) -> Result<ContainerRunResult, ContainerRuntimeError> {
                Ok(ContainerRunResult {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let mut node_spec = spec("tool", vec!["tool".into()], "missing.txt");
        node_spec.workdir = Some(dir.path().to_string_lossy().into_owned());
        let mut node =
            ContainerCommandNode::new(node_spec, Arc::new(MissingOutputRuntime)).unwrap();
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
    #[ignore = "requires a working rootless Podman runtime and a local OCI image"]
    async fn real_podman_copies_input_to_declared_output() {
        let image = std::env::var("AUTONOMICS_CONTAINER_IT_IMAGE")
            .unwrap_or_else(|_| "docker.io/library/debian:bookworm-slim".into());
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.txt");
        std::fs::write(&input, "rootless-container").unwrap();
        let mut node_spec = spec(
            &image,
            vec![
                "bash".into(),
                "-lc".into(),
                "cp -- \"$AUTONOMICS_INPUT0\" \"$AUTONOMICS_OUTPUT0\"".into(),
            ],
            "result.txt",
        );
        node_spec.workdir = Some(dir.path().to_string_lossy().into_owned());
        node_spec.pull_policy = PullPolicy::Never;
        let mut node =
            ContainerCommandNode::new(node_spec, Arc::new(PodmanRuntime::from_env())).unwrap();

        node.execute(
            &ctx(),
            &[input_file(&input)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("result.txt")).unwrap(),
            "rootless-container"
        );
    }
}
