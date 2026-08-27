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
use sha2::{Digest, Sha256};
use thiserror::Error;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::value::{FileRef, NodeValue, PortType};
use dag_core::{DataBundle, DataBundleBinding};
use dag_core::{NodeCtx, NodeFactory};

use container_runtime::{
    CachedPanel, ContainerRunRequest, ContainerRuntime, ContainerRuntimeError,
    DEFAULT_CONTAINER_WORKDIR, DEFAULT_TIMEOUT_SECS, K3sConfig, K3sRuntime, PanelCache, PanelRef,
    PodmanConfig, PodmanRuntime, PullPolicy, unique_container_name, workspace_ref,
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
    /// VFS prefix used to publish declared outputs as immutable artifacts.
    #[serde(default = "default_artifact_prefix")]
    pub artifact_prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Immutable reference-data bundles materialized into the shared panel cache.
    #[serde(default)]
    pub panels: Vec<PanelRef>,
    /// Catalog-backed panels resolved through the existing DataBundle registry.
    /// This path is used by containerized tooling without exposing object keys in DAG specs.
    #[serde(default)]
    pub panel_bundles: Vec<ContainerPanelBundleSpec>,
    /// Network profile enforced by cluster NetworkPolicies. The default is
    /// `isolated`; `cluster` and `egress` must be justified by the tool.
    #[serde(default = "default_network")]
    pub network: String,
    /// Whether the container root filesystem is read-only. `/work` remains
    /// writable, and k3s supplies tmpfs mounts for `/tmp` and `/dev/shm`.
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
    /// default runs as the control process uid/gid while enforcing non-root.
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct ContainerPanelBundleSpec {
    pub panel_id: String,
    pub mount_path: String,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn default_network() -> String {
    "isolated".into()
}

fn default_artifact_prefix() -> String {
    "/artifacts/container-command".into()
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
    artifact_prefix: String,
    timeout_secs: u64,
    panels: Vec<PanelRef>,
    panel_bundles: Vec<ResolvedPanelBundle>,
    network: String,
    read_only_rootfs: bool,
    pull_policy: PullPolicy,
    cpus: Option<f64>,
    memory: Option<String>,
    pids_limit: Option<i64>,
    shm_size: Option<String>,
    user: Option<String>,
    runtime: Arc<dyn ContainerRuntime>,
    panel_cache: Arc<PanelCache>,
}

#[derive(Clone)]
struct ResolvedPanelBundle {
    spec: ContainerPanelBundleSpec,
    bundle: DataBundle,
}

impl ContainerCommandNode {
    pub fn new(
        spec: ContainerCommandSpec,
        runtime: Arc<dyn ContainerRuntime>,
        panel_cache: Arc<PanelCache>,
    ) -> Result<Self, ContainerCommandError> {
        Self::new_with_catalog_panels(spec, runtime, panel_cache, Vec::new())
    }

    pub fn new_with_catalog_panels(
        spec: ContainerCommandSpec,
        runtime: Arc<dyn ContainerRuntime>,
        panel_cache: Arc<PanelCache>,
        panel_bundles: Vec<DataBundle>,
    ) -> Result<Self, ContainerCommandError> {
        validate(&spec)?;
        let resolved_panel_bundles = resolve_catalog_panels(&spec, &panel_bundles)?;
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
            artifact_prefix: spec.artifact_prefix,
            timeout_secs: spec.timeout_secs,
            panels: spec.panels,
            panel_bundles: resolved_panel_bundles,
            network: spec.network,
            read_only_rootfs: spec.read_only_rootfs,
            pull_policy: spec.pull_policy,
            cpus: spec.cpus,
            memory: spec.memory,
            pids_limit: spec.pids_limit,
            shm_size: spec.shm_size,
            user: spec.user,
            runtime,
            panel_cache,
        })
    }

    fn resolve_workdir(&self, workspace_root: &Path) -> Result<PathBuf, ContainerCommandError> {
        let workdir = match &self.workdir {
            Some(path) => {
                let candidate = PathBuf::from(path);
                if candidate.is_absolute() {
                    candidate
                } else {
                    workspace_root.join(candidate)
                }
            }
            None => workspace_root.join(unique_scratch_suffix()),
        };
        std::fs::create_dir_all(workspace_root).map_err(|e| {
            ContainerCommandError::Invalid(format!(
                "cannot create container workspace root `{}`: {e}",
                workspace_root.display()
            ))
        })?;
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
        if !workdir.starts_with(workspace_root) {
            return Err(ContainerCommandError::Invalid(format!(
                "workdir `{}` is outside container workspace root `{}`",
                workdir.display(),
                workspace_root.display()
            )));
        }
        Ok(workdir)
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

fn write_strictly_within(base: &Path, relative: &str, content: &str) -> Result<PathBuf, String> {
    validate_workspace_relative_path(relative)?;
    let destination = base.join(relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create directory `{}`: {e}", parent.display()))?;
    }
    std::fs::write(&destination, content)
        .map_err(|e| format!("cannot write file `{}`: {e}", destination.display()))?;
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
        NodeValue::FileSet(_) => Err("an empty FileSet cannot be bound to a command input".into()),
        NodeValue::DataFrame(_) => {
            Err("container_command inputs must be File or FileSet values".into())
        }
        NodeValue::Data(data) => Ok(data.vpath.clone()),
        NodeValue::DataSet(data) if !data.is_empty() => Ok(data
            .iter()
            .map(|data| data.vpath.as_str())
            .collect::<Vec<_>>()
            .join(",")),
        NodeValue::DataSet(_) => Err("an empty DataSet cannot be bound to a command input".into()),
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
                return Err("container_command inputs must be File or FileSet values".into());
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
    if !matches!(
        spec.network.as_str(),
        "isolated" | "none" | "cluster" | "egress"
    ) {
        return Err(ContainerCommandError::Invalid(format!(
            "network must be `isolated`, `cluster`, or `egress`; got `{}`",
            spec.network
        )));
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
        validate_workspace_relative_path(path).map_err(ContainerCommandError::Invalid)?;
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
    let mut panel_mounts = std::collections::BTreeSet::new();
    for panel in &spec.panels {
        panel.validate().map_err(ContainerCommandError::Invalid)?;
        if panel.mount_path == DEFAULT_CONTAINER_WORKDIR
            || !panel_mounts.insert(panel.mount_path.clone())
        {
            return Err(ContainerCommandError::Invalid(format!(
                "duplicate or invalid panel mount path `{}`",
                panel.mount_path
            )));
        }
    }
    for panel in &spec.panel_bundles {
        if panel.mount_path == DEFAULT_CONTAINER_WORKDIR
            || !Path::new(&panel.mount_path).is_absolute()
            || !panel_mounts.insert(panel.mount_path.clone())
        {
            return Err(ContainerCommandError::Invalid(format!(
                "duplicate or invalid catalog panel mount path `{}`",
                panel.mount_path
            )));
        }
    }
    Ok(())
}

fn resolve_catalog_panels(
    spec: &ContainerCommandSpec,
    bundles: &[DataBundle],
) -> Result<Vec<ResolvedPanelBundle>, ContainerCommandError> {
    let mut resolved = Vec::with_capacity(spec.panel_bundles.len());
    for panel in &spec.panel_bundles {
        let bundle = bundles
            .iter()
            .find(|bundle| bundle.ident == panel.panel_id)
            .ok_or_else(|| {
                ContainerCommandError::Invalid(format!(
                    "catalog panel `{}` was not resolved by the runtime DataBundle catalog",
                    panel.panel_id
                ))
            })?;
        if bundle.source.is_none() || bundle.digest.is_none() {
            return Err(ContainerCommandError::Invalid(format!(
                "catalog panel `{}` is not backed by an immutable catalog entry",
                panel.panel_id
            )));
        }
        resolved.push(ResolvedPanelBundle {
            spec: panel.clone(),
            bundle: bundle.clone(),
        });
    }
    Ok(resolved)
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
            artifact_prefix: self.artifact_prefix.clone(),
            timeout_secs: self.timeout_secs,
            panels: self.panels.clone(),
            panel_bundles: self.panel_bundles.clone(),
            network: self.network.clone(),
            read_only_rootfs: self.read_only_rootfs,
            pull_policy: self.pull_policy,
            cpus: self.cpus,
            memory: self.memory.clone(),
            pids_limit: self.pids_limit,
            shm_size: self.shm_size.clone(),
            user: self.user.clone(),
            runtime: Arc::clone(&self.runtime),
            panel_cache: Arc::clone(&self.panel_cache),
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
        let workspace_root = self.runtime.workspace_root().to_path_buf();
        let workspace_path = self
            .resolve_workdir(&workspace_root)
            .map_err(ContainerCommandError::into_dag_error)?;
        let mut panel_refs = self.panels.clone();
        panel_refs.extend(self.panel_bundles.iter().map(|panel| {
            PanelRef {
                id: panel.bundle.ident.clone(),
                digest: panel
                    .bundle
                    .digest
                    .clone()
                    .expect("validated catalog panel digest"),
                source: panel
                    .bundle
                    .source
                    .clone()
                    .expect("validated catalog panel source"),
                mount_path: panel.spec.mount_path.clone(),
            }
        }));
        let panels = materialize_panels(ctx, self.panel_cache.as_ref(), &panel_refs)
            .await
            .map_err(ContainerCommandError::Invalid)
            .map_err(ContainerCommandError::into_dag_error)?;

        let mut staged_inputs = stage_inputs(ctx, &workspace_path, inputs)
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
            .map(|path| container_path(&workspace_path, path))
            .collect::<Vec<_>>();

        let resolved_outputs = self
            .outputs
            .iter()
            .map(|output| (output, workspace_path.join(&output.path)))
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
            .map(|(spec, path)| {
                (
                    spec,
                    container_path(&workspace_path, &path.to_string_lossy()),
                )
            })
            .collect::<Vec<_>>();

        let mut command = self.command.clone();
        if let Some(script) = &self.script {
            let script_path = write_strictly_within(&workspace_path, ".autonomics/script", script)
                .map_err(ContainerCommandError::Invalid)
                .map_err(ContainerCommandError::into_dag_error)?;
            let files_dir = workspace_path.join(".autonomics/files");
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
            let script_arg = container_path(&workspace_path, &script_path.to_string_lossy());
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
            workspace: workspace_ref(&workspace_root, &workspace_path, DEFAULT_CONTAINER_WORKDIR)
                .map_err(ContainerCommandError::from)
                .map_err(ContainerCommandError::into_dag_error)?,
            env,
            panels,
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
        let request_name = request.name.clone();

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
            let file = publish_output(ctx, &self.artifact_prefix, &request_name, spec, host_path)
                .await
                .map_err(ContainerCommandError::Invalid)
                .map_err(ContainerCommandError::into_dag_error)?;
            outputs.insert_file(index as u8, file);
        }
        Ok(outputs)
    }
}

fn container_path(workspace_path: &Path, host_path: &str) -> String {
    let path = Path::new(host_path);
    let relative = path.strip_prefix(workspace_path).unwrap_or(path);
    Path::new(DEFAULT_CONTAINER_WORKDIR)
        .join(relative)
        .to_string_lossy()
        .into_owned()
}

fn unique_scratch_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!(
        "autonomics-container-command-{}-{nanos}",
        std::process::id()
    )
}

async fn materialize_panels(
    ctx: &NodeCtx,
    cache: &PanelCache,
    panels: &[PanelRef],
) -> Result<Vec<CachedPanel>, String> {
    if panels.is_empty() {
        return Ok(Vec::new());
    }
    let storage = ctx
        .opendal
        .as_ref()
        .ok_or("container panels require registered object storage")?;
    let mut cached = Vec::with_capacity(panels.len());
    for panel in panels {
        cached.push(
            cache
                .ensure(storage, panel)
                .await
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(cached)
}

async fn publish_output(
    ctx: &NodeCtx,
    artifact_prefix: &str,
    run_name: &str,
    spec: &ContainerCommandOutputSpec,
    host_path: &Path,
) -> Result<FileRef, String> {
    let storage = ctx
        .opendal
        .as_ref()
        .ok_or("container outputs require registered object storage")?;
    let prefix = vfs::OpendalFileStorage::normalize_path(artifact_prefix.trim_end_matches('/'));
    let relative = spec.path.replace('\\', "/");
    let virtual_path = format!("{prefix}/{run_name}/{relative}");
    storage
        .check_writable(&virtual_path)
        .map_err(|error| error.to_string())?;
    let object_path = storage.resolve_path(&virtual_path);
    let operator = storage.resolve(&virtual_path);
    let pending_path = format!("{object_path}.pending-{run_name}");
    let _ = operator.delete(&pending_path).await;

    let metadata = tokio::fs::metadata(host_path)
        .await
        .map_err(|error| error.to_string())?;
    let mut input = tokio::fs::File::open(host_path)
        .await
        .map_err(|error| error.to_string())?;
    let mut writer = operator
        .writer(&pending_path)
        .await
        .map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut chunk = vec![0_u8; 1024 * 1024];
    loop {
        let read = tokio::io::AsyncReadExt::read(&mut input, &mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        let bytes = chunk[..read].to_vec();
        hasher.update(&bytes);
        writer
            .write(bytes)
            .await
            .map_err(|error| error.to_string())?;
    }
    writer.close().await.map_err(|error| error.to_string())?;
    if let Err(error) = operator.rename(&pending_path, &object_path).await {
        let _ = operator.delete(&pending_path).await;
        return Err(error.to_string());
    }

    let digest = format!("sha256:{}", hex(&hasher.finalize()));
    Ok(FileRef::remote(
        format!("vfs://{virtual_path}"),
        spec.format.clone(),
        metadata.len(),
        Some(digest),
    ))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub struct ContainerCommandNodeFactory {
    pub(crate) runtime: Arc<dyn ContainerRuntime>,
    pub(crate) panel_cache: Arc<PanelCache>,
}

impl Default for ContainerCommandNodeFactory {
    fn default() -> Self {
        let infra = container_runtime::ContainerExecutionInfra::from_env();
        Self {
            runtime: infra.runtime,
            panel_cache: infra.panel_cache,
        }
    }
}

impl NodeFactory for ContainerCommandNodeFactory {
    fn kind(&self) -> &'static str {
        CONTAINER_COMMAND_KIND
    }

    fn desc(&self) -> &'static str {
        "Runs a file-to-file external command in an ephemeral OCI container."
    }

    fn data_bundles_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<Vec<DataBundleBinding>> {
        let node_spec: ContainerCommandSpec = serde_json::from_value(spec)?;
        Ok(node_spec
            .panel_bundles
            .iter()
            .enumerate()
            .map(|(index, panel)| {
                DataBundleBinding::new(format!("panel-{index}"), panel.panel_id.clone())
            })
            .collect())
    }

    fn doc(&self) -> &'static str {
        "Runs an OCI image without a shell. File inputs are staged into a private \
        workspace directory mounted at `/work`; input paths are exposed as \
        `$input0`, `$input1`, `AUTONOMICS_INPUT0`, and so on. Outputs must be \
        safe paths relative to `/work` and are exposed as `$output0`, \
        `AUTONOMICS_OUTPUT0`, and so on. The container root filesystem is \
        read-only by default, networking defaults to an isolated profile, and \
        immutable `panels` are mounted read-only from the shared panel cache. \
        Declared outputs are uploaded to VFS object storage with sha256 \
        fingerprints. Production workflows should reference images by digest."
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
        node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let node_spec: ContainerCommandSpec = serde_json::from_value(spec)?;
        let panel_bundles = node_spec
            .panel_bundles
            .iter()
            .enumerate()
            .map(|(index, _)| {
                node_ctx
                    .bound_data_bundle(&format!("panel-{index}"))
                    .cloned()
            })
            .collect::<dag_core::registry::error::Result<Vec<_>>>()?;
        let node = ContainerCommandNode::new_with_catalog_panels(
            node_spec,
            Arc::clone(&self.runtime) as Arc<dyn ContainerRuntime>,
            Arc::clone(&self.panel_cache),
            panel_bundles,
        )
        .map_err(|error| dag_core::registry::error::Error::Unknown(error.to_string()))?;
        Ok(Box::new(node))
    }

    fn ports_for_spec(
        &self,
        spec: serde_json::Value,
    ) -> dag_core::registry::error::Result<NodePorts> {
        let node_spec: ContainerCommandSpec = serde_json::from_value(spec)?;
        validate(&node_spec)
            .map_err(|e| dag_core::registry::error::Error::Unknown(e.to_string()))?;
        let mut ports = NodePorts::new()
            .set_fixed_input(false)
            .add_optional_input_port_of_type(PortType::File);
        for _ in &node_spec.outputs {
            ports = ports.add_output_port_of_type(None, PortType::File);
        }
        Ok(ports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use container_runtime::ContainerRunResult;
    use vfs::OpendalFileStorage;

    struct TestEnv {
        workspace: tempfile::TempDir,
        #[allow(dead_code)]
        objects: tempfile::TempDir,
        ctx: NodeCtx,
    }

    fn test_env() -> TestEnv {
        let workspace = tempfile::tempdir().unwrap();
        let objects = tempfile::tempdir().unwrap();
        let storage = Arc::new(OpendalFileStorage::new(objects.path()));
        let ctx = NodeCtx::new(
            datafusion::prelude::SessionContext::new().runtime_env(),
            Some(storage),
        );
        TestEnv {
            workspace,
            objects,
            ctx,
        }
    }

    fn input_file(path: &Path) -> NodeInput {
        NodeInput::file(0, FileRef::local(path, Some("txt".into())).unwrap())
    }

    #[derive(Default)]
    struct FakeRuntime {
        workspace_root: Option<PathBuf>,
        requests: Mutex<Vec<ContainerRunRequest>>,
    }

    impl FakeRuntime {
        fn new(workspace_root: &Path) -> Self {
            Self {
                workspace_root: Some(workspace_root.to_path_buf()),
                ..Default::default()
            }
        }
    }

    fn host_path(request: &ContainerRunRequest, container_path: &str) -> PathBuf {
        let relative = Path::new(container_path)
            .strip_prefix(DEFAULT_CONTAINER_WORKDIR)
            .expect("path is inside /work");
        request.workspace.host_path.join(relative)
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

        fn workspace_root(&self) -> &Path {
            self.workspace_root
                .as_deref()
                .unwrap_or_else(|| Path::new("/tmp"))
        }
    }

    /// Helper function for creating container node spec
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
            artifact_prefix: "/artifacts/test".into(),
            timeout_secs: 10,
            panels: Vec::new(),
            panel_bundles: Vec::new(),
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
        let env = test_env();
        let dir = env.workspace.path().join("run");
        std::fs::create_dir_all(&dir).unwrap();
        let input = env.workspace.path().join("input.txt");
        std::fs::write(&input, "hello-container").unwrap();
        let runtime = Arc::new(FakeRuntime::new(env.workspace.path()));
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
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        let mut node = ContainerCommandNode::new(
            node_spec,
            runtime.clone(),
            Arc::new(PanelCache::new(
                env.workspace.path().join("cache"),
                "panels",
            )),
        )
        .unwrap();

        let outputs = node
            .execute(
                &env.ctx,
                &[input_file(&input)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        let request = runtime.requests.lock().unwrap().last().unwrap().clone();
        assert!(request.command.contains(&"--input".into()));
        assert!(request.command.contains(&format!(
            "{DEFAULT_CONTAINER_WORKDIR}/.autonomics/inputs/input-0.txt"
        )));
        assert!(
            request
                .command
                .contains(&format!("{DEFAULT_CONTAINER_WORKDIR}/result.txt"))
        );
        assert_eq!(request.network, "isolated");
        assert!(request.read_only_rootfs);
        assert!(
            request
                .workspace
                .host_path
                .ends_with(dir.file_name().unwrap())
        );

        let output = dir.join("result.txt");
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "hello-container");
        let file = outputs.get(&0).unwrap().as_file().unwrap();
        assert!(file.path.starts_with("vfs:///artifacts/test/"));
        let fingerprint = file.fingerprint.as_ref().unwrap();
        assert_eq!(fingerprint.size, "hello-container".len() as u64);
        assert!(
            fingerprint
                .content_hash
                .as_deref()
                .unwrap()
                .starts_with("sha256:")
        );
        let object = env
            .ctx
            .opendal
            .as_ref()
            .unwrap()
            .resolve(file.path.strip_prefix("vfs://").unwrap())
            .read(
                &env.ctx
                    .opendal
                    .as_ref()
                    .unwrap()
                    .resolve_path(file.path.strip_prefix("vfs://").unwrap()),
            )
            .await
            .unwrap();
        assert_eq!(object.to_vec(), b"hello-container");
    }

    #[tokio::test]
    async fn script_mode_materializes_private_assets() {
        let env = test_env();
        let dir = env.workspace.path().join("script-run");
        std::fs::create_dir_all(&dir).unwrap();
        let runtime = Arc::new(FakeRuntime::new(env.workspace.path()));
        let mut node_spec = spec(
            "quay.io/example/bash",
            vec!["bash".into(), "-e".into()],
            "script-result.txt",
        );
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        node_spec.script = Some("cat \"$AUTONOMICS_INPUT0\" > \"$AUTONOMICS_OUTPUT0\"".into());
        node_spec.files.insert("helper.txt".into(), "helper".into());
        let mut node = ContainerCommandNode::new(
            node_spec,
            runtime.clone(),
            Arc::new(PanelCache::new(
                env.workspace.path().join("cache"),
                "panels",
            )),
        )
        .unwrap();

        node.execute(
            &env.ctx,
            &[],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        assert!(dir.join(".autonomics/script").is_file());
        assert!(dir.join(".autonomics/files/helper.txt").is_file());
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
            Arc::new(PanelCache::new("/tmp/autonomics-cache", "panels")),
        ) {
            Ok(_) => panic!("output path escape must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("safe relative path"));
    }

    #[test]
    fn factory_resolves_dynamic_output_ports() {
        let ports = ContainerCommandNodeFactory::default()
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
    async fn concurrent_container_nodes_publish_distinct_artifacts() {
        let env = test_env();
        let runtime = Arc::new(FakeRuntime::new(env.workspace.path()));
        let mut nodes = Vec::new();
        for run in ["a", "b"] {
            let dir = env.workspace.path().join(run);
            std::fs::create_dir_all(&dir).unwrap();
            let mut node_spec = spec("tool", vec!["tool".into()], "result.txt");
            node_spec.workdir = Some(dir.to_string_lossy().into_owned());
            nodes.push(
                ContainerCommandNode::new(
                    node_spec,
                    runtime.clone(),
                    Arc::new(PanelCache::new(
                        env.workspace.path().join("cache"),
                        "panels",
                    )),
                )
                .unwrap(),
            );
        }

        let mut second_node = nodes.pop().unwrap();
        let mut first_node = nodes.pop().unwrap();
        let first_reporter = dag_core::dag::node_event::NodeReporter::noop();
        let second_reporter = dag_core::dag::node_event::NodeReporter::noop();
        let (first, second) = tokio::join!(
            first_node.execute(&env.ctx, &[], &first_reporter,),
            second_node.execute(&env.ctx, &[], &second_reporter,)
        );
        first.unwrap();
        second.unwrap();

        assert_eq!(runtime.requests.lock().unwrap().len(), 2);
        let paths: Vec<_> = runtime
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.name.clone())
            .collect();
        assert_ne!(paths[0], paths[1]);
    }

    #[tokio::test]
    async fn catalog_panel_bundle_resolves_to_readonly_panel_ref() {
        let env = test_env();
        let panel_content = b"catalog-panel";
        let digest = format!("sha256:{}", hex(&Sha256::digest(panel_content)));
        let source = "/catalog/1000g_eur";
        let panel_root = env.objects.path().join("catalog/1000g_eur");
        std::fs::create_dir_all(panel_root.join("chr22")).unwrap();
        std::fs::write(panel_root.join("chr22/panel.txt"), panel_content).unwrap();
        std::fs::write(
            panel_root.join("manifest.json"),
            serde_json::to_vec(&container_runtime::PanelManifest {
                schema_version: 1,
                id: "1000g_eur".into(),
                version: "v3".into(),
                digest: format!("sha256:{}", "3".repeat(64)),
                files: vec![container_runtime::PanelFile {
                    path: "chr22/panel.txt".into(),
                    size: panel_content.len() as u64,
                    sha256: digest,
                }],
            })
            .unwrap(),
        )
        .unwrap();

        let mut bundle = DataBundle::new("1000g_eur", "1000G EUR", "/bundles/1000g_eur");
        bundle.source = Some(source.into());
        bundle.digest = Some(format!("sha256:{}", "3".repeat(64)));
        let dir = env.workspace.path().join("catalog-run");
        std::fs::create_dir_all(&dir).unwrap();
        let mut node_spec = spec("tool", vec!["tool".into()], "result.txt");
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        node_spec.panel_bundles = vec![ContainerPanelBundleSpec {
            panel_id: "1000g_eur".into(),
            mount_path: "/panels/1000g_eur".into(),
        }];

        let runtime = Arc::new(FakeRuntime::new(env.workspace.path()));
        let mut node = ContainerCommandNode::new_with_catalog_panels(
            node_spec,
            runtime.clone(),
            Arc::new(PanelCache::new(
                env.workspace.path().join("cache"),
                "panels",
            )),
            vec![bundle],
        )
        .unwrap();
        node.execute(
            &env.ctx,
            &[],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        let request = runtime.requests.lock().unwrap().last().unwrap().clone();
        let panel = request.panels.first().unwrap();
        assert_eq!(panel.id, "1000g_eur");
        assert!(panel.host_path.to_string_lossy().contains("1000g_eur"));
        assert_eq!(panel.mount_path, "/panels/1000g_eur");
        assert!(
            runtime
                .requests
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .panels
                .iter()
                .all(|candidate| candidate.pvc_sub_path.contains('@'))
        );
    }

    #[tokio::test]
    async fn missing_output_fails_after_successful_exit() {
        struct MissingOutputRuntime {
            workspace_root: PathBuf,
        }
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
            fn workspace_root(&self) -> &Path {
                &self.workspace_root
            }
        }

        let env = test_env();
        let dir = env.workspace.path().join("missing");
        std::fs::create_dir_all(&dir).unwrap();
        let mut node_spec = spec("tool", vec!["tool".into()], "missing.txt");
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        let mut node = ContainerCommandNode::new(
            node_spec,
            Arc::new(MissingOutputRuntime {
                workspace_root: env.workspace.path().to_path_buf(),
            }),
            Arc::new(PanelCache::new(
                env.workspace.path().join("cache"),
                "panels",
            )),
        )
        .unwrap();
        let error = node
            .execute(
                &env.ctx,
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("missing.txt"));
    }

    #[tokio::test]
    #[ignore = "requires a configured k3s cluster and a local OCI image"]
    async fn real_k3s_copies_input_to_declared_output() {
        let image = std::env::var("AUTONOMICS_CONTAINER_IT_IMAGE")
            .unwrap_or_else(|_| "docker.io/library/debian:bookworm-slim".into());
        let env = test_env();
        let mut config = K3sConfig::default();
        let dir = config
            .workspace_root
            .join(format!("k3s-it-{}", unique_scratch_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = env.workspace.path().join("input.txt");
        std::fs::write(&input, "rootless-container").unwrap();
        let mut node_spec = spec(
            &image,
            vec![
                "cp".into(),
                "--".into(),
                "$input0".into(),
                "$output0".into(),
            ],
            "result.txt",
        );
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        node_spec.pull_policy = PullPolicy::Never;
        config.panel_cache_root = env.workspace.path().join("cache");
        let runtime = K3sRuntime::new(config);
        let cache = PanelCache::new(runtime.config().panel_cache_root.clone(), "panels");
        let mut node =
            ContainerCommandNode::new(node_spec, Arc::new(runtime), Arc::new(cache)).unwrap();

        node.execute(
            &env.ctx,
            &[input_file(&input)],
            &dag_core::dag::node_event::NodeReporter::noop(),
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.join("result.txt")).unwrap(),
            "rootless-container"
        );
    }

    #[tokio::test]
    #[ignore = "requires a working rootless Podman runtime and may pull an OCI image"]
    async fn real_podman_copies_input_to_declared_output() {
        let image = std::env::var("AUTONOMICS_CONTAINER_IT_IMAGE")
            .unwrap_or_else(|_| "docker.io/library/debian:bookworm-slim".into());
        let env = test_env();
        let state = tempfile::tempdir().unwrap();
        let workspace_root = state.path().join("workspace");
        let dir = workspace_root.join("podman-it");
        std::fs::create_dir_all(&dir).unwrap();
        let input = env.workspace.path().join("input.txt");
        std::fs::write(&input, "podman-container-command").unwrap();
        let mut node_spec = spec(
            &image,
            vec![
                "cp".into(),
                "--".into(),
                "$input0".into(),
                "$output0".into(),
            ],
            "result.txt",
        );
        node_spec.workdir = Some(dir.to_string_lossy().into_owned());
        node_spec.timeout_secs = 120;
        let runtime = PodmanRuntime::new(PodmanConfig {
            program: std::env::var("AUTONOMICS_PODMAN_PROGRAM").unwrap_or_else(|_| "podman".into()),
            workspace_root: workspace_root.clone(),
            panel_cache_root: state.path().join("panels"),
        });
        let mut node = ContainerCommandNode::new(
            node_spec,
            Arc::new(runtime),
            Arc::new(PanelCache::new(state.path().join("panels"), "")),
        )
        .unwrap();

        let outputs = node
            .execute(
                &env.ctx,
                &[input_file(&input)],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.join("result.txt")).unwrap(),
            "podman-container-command"
        );
        let file = outputs.get(&0).unwrap().as_file().unwrap();
        assert!(file.path.starts_with("vfs:///artifacts/test/"));
    }

    #[derive(Clone)]
    struct CatalogSumstatsNode {
        ports: NodePorts,
        panel: String,
    }

    #[async_trait]
    impl DagNode for CatalogSumstatsNode {
        fn ports(&self) -> &NodePorts {
            &self.ports
        }

        fn clone_box(&self) -> Box<dyn DagNode> {
            Box::new(self.clone())
        }

        fn kind(&self) -> &'static str {
            "ldsc_catalog_sumstats_test_source"
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        async fn execute(
            &mut self,
            ctx: &NodeCtx,
            _inputs: &[NodeInput],
            _reporter: &dag_core::dag::node_event::NodeReporter,
        ) -> Result<PortOutputs, DagError> {
            let session = ctx.session();
            use datafusion::prelude::{ParquetReadOptions, col, lit};
            let df = session
                .read_parquet(&self.panel, ParquetReadOptions::default())
                .await?
                .select(vec![
                    col("rsid"),
                    lit(1.5_f64).alias("z"),
                    lit(100_000_f64).alias("n"),
                ])?
                .limit(0, Some(5000))?;
            let mut outputs = PortOutputs::new();
            outputs.insert(0, df);
            Ok(outputs)
        }
    }

    #[tokio::test]
    #[ignore = "requires Garage catalog data, k3s PVCs, kubeconfig, and a local Debian image"]
    async fn real_catalog_backed_containerized_ldsc_runs_through_dag() {
        let vfs_config_path = std::env::var_os("AUTONOMICS_TEST_VFS_CONFIG")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| Path::new(&home).join(".autonomics/vfs.toml"))
            })
            .expect("HOME or AUTONOMICS_TEST_VFS_CONFIG is required");
        let source = std::fs::read_to_string(vfs_config_path).unwrap();
        let mut manifest = vfs::VfsManifest::from_toml(&source).unwrap();
        let catalog_config = data_catalog::CatalogConfig::from_vfs_toml(&source).unwrap();
        let catalog_runtime = data_catalog::CatalogRuntime::load(&manifest, &catalog_config)
            .await
            .unwrap();
        manifest.mount.extend(
            data_catalog::catalog_mount_definitions(
                &manifest,
                &catalog_runtime.index,
                &catalog_config,
            )
            .unwrap(),
        );
        let mounted = Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap());
        let scratch = tempfile::tempdir().unwrap();
        let storage = Arc::new(vfs::OpendalFileStorage::with_mounts(
            scratch.path(),
            mounted.clone(),
        ));
        let session = datafusion::prelude::SessionContext::new();
        session.runtime_env().register_object_store(
            datafusion::execution::object_store::ObjectStoreUrl::parse("vfs://")
                .unwrap()
                .as_ref(),
            mounted,
        );
        let bundles = catalog_runtime.data_bundles();
        let ctx = NodeCtx::new(session.runtime_env(), Some(storage))
            .with_data_bundle_catalog(Arc::new(bundles.clone()));

        let k3s_config = K3sConfig::from_env();
        let run_suffix = unique_scratch_suffix();
        let workspace = k3s_config
            .workspace_root
            .join(format!("ldsc-catalog-{run_suffix}"));
        std::fs::create_dir_all(&workspace).unwrap();
        let runtime = Arc::new(K3sRuntime::new(k3s_config.clone()));
        let panel_cache = Arc::new(PanelCache::new(
            k3s_config.panel_cache_root.clone(),
            k3s_config.panel_pvc_prefix.clone(),
        ));
        let panel_ids = ["ldsc.hsq.cli", "ldscore.1000g_eur", "ldscore.1000g_eur_m"];
        let panel_bundles = panel_ids
            .iter()
            .map(|id| bundles.get(id).cloned().unwrap())
            .collect::<Vec<_>>();

        let mut node_spec = spec(
            &std::env::var("AUTONOMICS_CONTAINER_IT_IMAGE")
                .unwrap_or_else(|_| "docker.io/library/debian:bookworm-slim".into()),
            vec!["bash".into(), "-e".into()],
            "ldsc-result.csv",
        );
        node_spec.script = Some(
            r#"cp -- /panels/cli/ldsc-hsq-container "$AUTONOMICS_WORKDIR/ldsc-hsq-container"
chmod 700 "$AUTONOMICS_WORKDIR/ldsc-hsq-container"
"$AUTONOMICS_WORKDIR/ldsc-hsq-container" \
  --input "$AUTONOMICS_INPUT0" \
  --output "$AUTONOMICS_OUTPUT0" \
  --ld-panel /panels/ldscore \
  --m-panel /panels/ldscore_m \
  --n-blocks 20"#
                .into(),
        );
        node_spec.workdir = Some(workspace.to_string_lossy().into_owned());
        node_spec.artifact_prefix = format!("/artifacts/ldsc-catalog-e2e/{run_suffix}");
        node_spec.timeout_secs = 900;
        node_spec.pull_policy = PullPolicy::Never;
        node_spec.panel_bundles = vec![
            ContainerPanelBundleSpec {
                panel_id: "ldsc.hsq.cli".into(),
                mount_path: "/panels/cli".into(),
            },
            ContainerPanelBundleSpec {
                panel_id: "ldscore.1000g_eur".into(),
                mount_path: "/panels/ldscore".into(),
            },
            ContainerPanelBundleSpec {
                panel_id: "ldscore.1000g_eur_m".into(),
                mount_path: "/panels/ldscore_m".into(),
            },
        ];

        let sumstats_source = CatalogSumstatsNode {
            ports: NodePorts::new().add_output_port(None),
            panel: "vfs:///bundles/ldscore.1000g_eur/1000g_eur.parquet".into(),
        };
        let sink = crate::sink_file::FileSinkNode::new(
            format!("vfs:///var/lib/autonomics/k3s/workspace/ldsc-catalog-input-{run_suffix}.csv"),
            crate::sink_file::WriteFormat::Csv,
            dag_core::SinkMode::Overwrite,
        );
        let result_reader = crate::source_file::FileSourceNode::new(None, None);

        let container = ContainerCommandNode::new_with_catalog_panels(
            node_spec,
            runtime,
            panel_cache,
            panel_bundles,
        )
        .unwrap();
        let result_reader = result_reader;

        let mut dag = dag_core::dag::DAG::default();
        dag.add_node("sumstats".into(), Box::new(sumstats_source))
            .unwrap();
        dag.add_node("write_input".into(), sink.clone_box())
            .unwrap();
        dag.add_node("container_ldsc".into(), container.clone_box())
            .unwrap();
        dag.add_node("read_result".into(), result_reader.clone_box())
            .unwrap();
        dag.add_edge("sumstats", "write_input", 0, 0).unwrap();
        dag.add_edge("write_input", "container_ldsc", 0, 0).unwrap();
        dag.add_edge("container_ldsc", "read_result", 0, 0).unwrap();
        let report = dag
            .run(
                &dag_core::dag::runtime::SchedulerConfig::default(),
                &ctx,
                None,
            )
            .await
            .unwrap();
        println!("LDSC catalog DAG statuses: {:?}", report.statuses);
        for node in &report.nodes {
            println!(
                "LDSC catalog DAG node {}: {:?}, error={:?}",
                node.id, node.status, node.error
            );
        }

        let output = dag.output("read_result").unwrap();
        let rows = output
            .get(&0)
            .unwrap()
            .as_dataframe()
            .unwrap()
            .clone()
            .collect()
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].num_rows(), 1);
        let names = rows[0]
            .schema()
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect::<Vec<_>>();
        for name in [
            "h2",
            "h2_se",
            "intercept",
            "mean_chisq",
            "lambda_gc",
            "n_snp",
        ] {
            assert!(
                names.contains(&name.to_string()),
                "missing result column {name}"
            );
        }
    }
}
