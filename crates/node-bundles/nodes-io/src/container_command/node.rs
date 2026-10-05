//! The file-to-file container command node.
//!
//! Inputs are materialized into a private host scratch directory, that
//! directory is mounted at `/work`, the command runs in an ephemeral image,
//! and only declared output files become DAG values. Construction validates
//! the spec and resolves catalog panels; execution stages inputs,
//! materializes panels, runs one ephemeral container, and publishes declared
//! outputs to VFS.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use dag_core::dag::node_event::NodeReporter;
use dag_core::dag::runtime::NodeRunDetails;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::value::{FileRef, NodeValue, PortType};
use dag_core::{DataBundle, NodeCtx};

use container_runtime::gc::{acquire_panel_lock_shared, acquire_scratch_lock_shared};
use container_runtime::{
    CachedPanel, ContainerNetwork, ContainerRunRequest, ContainerRuntimeError,
    DEFAULT_CONTAINER_WORKDIR, GpuRequest, ImageReference, PanelCache, PanelRef, PodmanConnection,
    PullPolicy, keep_workspace_enabled, unique_container_name, workspace_ref,
};

use super::ContainerCommandOutputSpec;
use super::error::ContainerCommandError;
use super::utils::{
    capture_declared_output_logs, capture_input_manifest, container_path, hex, input_path,
    staged_path, unique_scratch_suffix, virtual_path, write_failure_logs, write_strictly_within,
};
use super::{ContainerCommandSpec, ContainerPanelBundleSpec, validate};

pub struct ContainerCommandNode {
    kind: &'static str,
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
    network: ContainerNetwork,
    read_only_rootfs: bool,
    pull_policy: PullPolicy,
    cpus: Option<f64>,
    memory: Option<String>,
    pids_limit: Option<i64>,
    shm_size: Option<String>,
    gpus: GpuRequest,
    user: Option<String>,
    runtime: Arc<dyn PodmanConnection>,
    panel_cache: Arc<PanelCache>,
}

#[derive(Clone)]
struct ResolvedPanelBundle {
    spec: ContainerPanelBundleSpec,
    bundle: DataBundle,
}

impl ContainerCommandNode {
    pub fn new(
        kind: &'static str,
        spec: ContainerCommandSpec,
        runtime: Arc<dyn PodmanConnection>,
        panel_cache: Arc<PanelCache>,
    ) -> Result<Self, ContainerCommandError> {
        Self::new_with_catalog_panels(kind, spec, runtime, panel_cache, Vec::new())
    }

    pub fn new_with_catalog_panels(
        kind: &'static str,
        spec: ContainerCommandSpec,
        runtime: Arc<dyn PodmanConnection>,
        panel_cache: Arc<PanelCache>,
        panel_bundles: Vec<DataBundle>,
    ) -> Result<Self, ContainerCommandError> {
        validate(&spec)?;
        let resolved_panel_bundles = resolve_catalog_panels(&spec, &panel_bundles)?;
        let mut ports = NodePorts::new()
            .set_fixed_input(false)
            .add_optional_input_port_of_type(PortType::File);
        for output in &spec.outputs {
            let label = Path::new(&output.path)
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("output");
            if let Some(format) = &output.format {
                ports = ports.add_output_port_of_type_with_label_and_format(
                    None,
                    PortType::File,
                    label,
                    format.clone(),
                );
            } else {
                ports = ports.add_output_port_of_type(None, PortType::File);
            }
        }
        Ok(Self {
            kind,
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
            network: ContainerNetwork::parse(&spec.network)
                .map_err(ContainerCommandError::Invalid)?,
            read_only_rootfs: spec.read_only_rootfs,
            pull_policy: spec.pull_policy,
            cpus: spec.cpus,
            memory: spec.memory,
            pids_limit: spec.pids_limit,
            shm_size: spec.shm_size,
            gpus: match spec.gpus.as_deref() {
                None => GpuRequest::None,
                Some(value) => GpuRequest::parse(value).map_err(ContainerCommandError::Invalid)?,
            },
            user: spec.user,
            runtime,
            panel_cache,
        })
    }

    /// Override the node's declared port layout.
    ///
    /// The default layout (`ContainerCommandNode::new*`) is the legacy
    /// `container_command` contract: a variadic, optional file input. Plugin
    /// manifests declare explicit input ports, and those must surface on the
    /// built node — not only on the factory — so DAG validation enforces
    /// them: a required input port with no edge has to fail the run
    /// (`PortDisconnected`) instead of executing with zero inputs and
    /// "succeeding" on an empty FileSet.
    pub fn with_ports(mut self, ports: NodePorts) -> Self {
        self.ports = ports;
        self
    }

    /// Resolves the host scratch directory that will be mounted at `/work`.
    ///
    /// A spec-provided `workdir` is used as-is when absolute and joined onto
    /// `workspace_root` when relative; when omitted, a unique scratch
    /// directory keeps concurrent runs isolated. Missing directories are
    /// created on demand, the path is canonicalized, and any candidate that
    /// escapes `workspace_root` (an absolute path elsewhere, `..` traversal,
    /// or a symlink pointing out) is rejected before it can be mounted into
    /// the container.
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
        // Canonicalize before the containment check below: `workdir` comes
        // from the DAG spec and is mounted into the container, so `..` or a
        // symlink must not be able to place it outside the workspace root.
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
                    "catalog panel `{}` was not resolved by the runtime bundle registry",
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
            // Blocking write on the blocking pool: a multi-GB object must
            // not pin a tokio worker for the whole duration.
            let staged = destination.clone();
            let written = tokio::task::spawn_blocking(move || {
                std::fs::write(&staged, bytes.to_vec()).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| format!("staging task failed: {e}"))
            .and_then(|result| result);
            written.map_err(|e| format!("cannot stage input `{}`: {e}", destination.display()))?;
            return Ok(destination);
        }
    }

    let host_path = Path::new(&file.path);
    if host_path.is_file() {
        let source = host_path.to_path_buf();
        let staged = destination.clone();
        let copied = tokio::task::spawn_blocking(move || {
            std::fs::copy(&source, &staged)
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("staging task failed: {e}"))
        .and_then(|result| result);
        copied.map_err(|e| format!("cannot stage input `{}`: {e}", host_path.display()))?;
        return Ok(destination);
    }

    Err(format!(
        "input file `{}` was not found on the host or virtual filesystem",
        file.path
    ))
}

pub(super) async fn stage_inputs(
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

    // Scheduler inputs follow edge insertion order, not necessarily input-port
    // order. Stage by port first so batch image FileSets, mask FileSets, and
    // manifest Files receive deterministic AUTONOMICS_INPUT* ranges.
    let mut ordered_inputs = inputs.to_vec();
    ordered_inputs.sort_by_key(|input| input.port);
    let mut staged = Vec::with_capacity(ordered_inputs.len());
    let mut index = 0usize;
    for input in &ordered_inputs {
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
            NodeValue::DataFrame(_) | NodeValue::Channel(_) => {
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

#[async_trait]
impl DagNode for ContainerCommandNode {
    fn ports(&self) -> &NodePorts {
        &self.ports
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(Self {
            kind: self.kind,
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
            network: self.network,
            read_only_rootfs: self.read_only_rootfs,
            pull_policy: self.pull_policy,
            cpus: self.cpus,
            memory: self.memory.clone(),
            pids_limit: self.pids_limit,
            shm_size: self.shm_size.clone(),
            gpus: self.gpus.clone(),
            user: self.user.clone(),
            runtime: Arc::clone(&self.runtime),
            panel_cache: Arc::clone(&self.panel_cache),
        })
    }

    fn kind(&self) -> &'static str {
        self.kind
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
            .map_err(|error| error.into_dag_error(self.kind))?;
        // Hold the scratch lock for the whole run: the GC sweeper takes the
        // same lock exclusively and therefore never reclaims a live run's
        // workspace. Released on drop, on every early return below.
        let scratch_lock = acquire_scratch_lock_shared(&workspace_path)
            .map_err(|error| {
                ContainerCommandError::Invalid(format!(
                    "cannot lock container workspace `{}`: {error}",
                    workspace_path.display()
                ))
            })
            .map_err(|error| error.into_dag_error(self.kind))?;
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
        let (panels, _panel_locks) =
            materialize_panels(ctx, self.panel_cache.as_ref(), &panel_refs)
                .await
                .map_err(ContainerCommandError::Invalid)
                .map_err(|error| error.into_dag_error(self.kind))?;

        let mut staged_inputs = stage_inputs(ctx, &workspace_path, inputs)
            .await
            .map_err(ContainerCommandError::Invalid)
            .map_err(|error| error.into_dag_error(self.kind))?;
        staged_inputs.sort_by_key(|input| input.port);
        let host_input_paths = staged_inputs
            .iter()
            .map(|input| input_path(&input.data))
            .collect::<Result<Vec<_>, _>>()
            .map_err(ContainerCommandError::Invalid)
            .map_err(|error| error.into_dag_error(self.kind))?;
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
                    .map_err(|error| error.into_dag_error(self.kind))?;
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
                .map_err(|error| error.into_dag_error(self.kind))?;
            let files_dir = workspace_path.join(".autonomics/files");
            std::fs::create_dir_all(&files_dir)
                .map_err(|e| {
                    ContainerCommandError::Invalid(format!(
                        "cannot create files directory `{}`: {e}",
                        files_dir.display()
                    ))
                })
                .map_err(|error| error.into_dag_error(self.kind))?;
            for (relative, content) in &self.files {
                write_strictly_within(&files_dir, relative, content)
                    .map_err(ContainerCommandError::Invalid)
                    .map_err(|error| error.into_dag_error(self.kind))?;
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
                .map_err(|error| error.into_dag_error(self.kind))?,
            env,
            panels,
            network: self.network,
            read_only_rootfs: self.read_only_rootfs,
            pull_policy: self.pull_policy,
            cpus: self.cpus,
            memory: self.memory.clone(),
            pids_limit: self.pids_limit,
            shm_size: self.shm_size.clone(),
            gpus: self.gpus.clone(),
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
        let result = match self.runtime.run(request).await {
            Ok(result) => result,
            Err(error) => {
                // Also surface the capped capture on the live event stream; the
                // authoritative copy travels through the node's RunReport error.
                let error = match error {
                    ContainerRuntimeError::ExitStatus {
                        exit_code,
                        stderr,
                        stdout,
                    } => {
                        write_failure_logs(&workspace_path, &stdout, &stderr);
                        // Persist the captured streams next to where the
                        // outputs would have landed, so a failed execution is
                        // as auditable as a successful one.
                        let logs = publish_run_logs(
                            ctx,
                            &self.artifact_prefix,
                            &request_name,
                            &stdout,
                            &stderr,
                            reporter,
                        )
                        .await;
                        reporter.set_run_details(run_details(
                            &self.image,
                            exit_code,
                            &request_name,
                            logs,
                        ));
                        let mut logs = capture_declared_output_logs(&resolved_outputs);
                        logs.extend(capture_input_manifest(&staged_inputs));
                        ContainerCommandError::ExitStatus {
                            exit_code,
                            stderr,
                            stdout,
                            output_logs: logs,
                        }
                    }
                    error => ContainerCommandError::Runtime(error),
                };
                reporter.error(error.diagnostic_message());
                return Err(error.into_dag_error(self.kind));
            }
        };
        // Persist the captured streams beside this run's outputs before the
        // live events below consume them — the events are best-effort and
        // disappear with the channel, the persisted copies are the audit
        // record.
        let logs = publish_run_logs(
            ctx,
            &self.artifact_prefix,
            &request_name,
            &result.stdout,
            &result.stderr,
            reporter,
        )
        .await;
        reporter.set_run_details(run_details(
            &self.image,
            result.exit_code,
            &request_name,
            logs,
        ));
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
                .into_dag_error(self.kind));
            }
            let file = publish_output(ctx, &self.artifact_prefix, &request_name, spec, host_path)
                .await
                .map_err(ContainerCommandError::Invalid)
                .map_err(|error| error.into_dag_error(self.kind))?;
            outputs.insert_file(index as u8, file);
        }

        // Every declared output now lives in object storage, so a unique
        // scratch directory has no remaining value. User-declared workdirs
        // are persistent by contract; AUTONOMICS_KEEP_WORKSPACE=1 keeps
        // scratch for debugging until the sweeper's age window expires.
        // Failed runs above return early and keep their scratch the same way.
        if self.workdir.is_none() && !keep_workspace_enabled() {
            drop(scratch_lock);
            drop(_panel_locks);
            let scratch = workspace_path.clone();
            let removed = tokio::task::spawn_blocking(move || {
                std::fs::remove_dir_all(&scratch).map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string());
            if let Err(error) = removed.and_then(|result| result) {
                reporter.warn(format!(
                    "cannot remove container scratch `{}`: {error}",
                    workspace_path.display()
                ));
            }
        }
        Ok(outputs)
    }
}

/// Materialize panels into the shared cache and lock each entry for the
/// duration of the run. The returned lock files must stay alive until
/// `PodmanConnection::run` has returned; dropping them releases the locks and
/// makes the entries eligible for the panel-cache sweeper again.
async fn materialize_panels(
    ctx: &NodeCtx,
    cache: &PanelCache,
    panels: &[PanelRef],
) -> Result<(Vec<CachedPanel>, Vec<std::fs::File>), String> {
    if panels.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let storage = ctx
        .opendal
        .as_ref()
        .ok_or("container panels require registered object storage")?;
    let mut cached = Vec::with_capacity(panels.len());
    let mut locks = Vec::with_capacity(panels.len());
    for panel in panels {
        let materialized = cache
            .ensure(storage, panel)
            .await
            .map_err(|error| error.to_string())?;
        let entry_name = materialized
            .host_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                format!(
                    "panel `{}` cache path `{}` has no entry name",
                    panel.id,
                    materialized.host_path.display()
                )
            })?
            .to_string();
        let lock = acquire_panel_lock_shared(&cache.root, &entry_name)
            .map_err(|error| format!("cannot lock panel `{}`: {error}", panel.id))?;
        locks.push(lock);
        cached.push(materialized);
    }
    Ok((cached, locks))
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

    // Hash on the blocking pool while the object-store writer streams the
    // same bytes: a single sha256 pass over a multi-GB output used to pin a
    // tokio worker for the whole upload.
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    let hash_source = host_path.to_path_buf();
    let hashing = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
        use std::io::Read;
        let mut input = std::fs::File::open(&hash_source)?;
        let mut hasher = Sha256::new();
        let mut chunk = vec![0_u8; 1024 * 1024];
        loop {
            let read = input.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            hasher.update(&chunk[..read]);
            // A send error means the uploader aborted and went away; keep
            // hashing so the digest stays correct.
            let _ = chunk_tx.blocking_send(chunk[..read].to_vec());
        }
        Ok(format!("sha256:{}", hex(&hasher.finalize())))
    });

    let mut writer = operator
        .writer(&pending_path)
        .await
        .map_err(|error| error.to_string())?;
    let mut upload_result: Result<(), String> = Ok(());
    while let Some(chunk) = chunk_rx.recv().await {
        if let Err(error) = writer.write(chunk).await {
            upload_result = Err(error.to_string());
            break;
        }
    }
    if upload_result.is_ok() {
        upload_result = writer
            .close()
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
    }
    // Release the hasher if the upload aborted early, so `hashing` can
    // finish instead of blocking on a full channel.
    drop(chunk_rx);

    let digest = match (upload_result, hashing.await) {
        (Ok(()), Ok(Ok(digest))) => digest,
        (upload, hashing) => {
            let _ = operator.delete(&pending_path).await;
            let hashing_error = match hashing {
                Ok(Err(error)) => Some(error.to_string()),
                Err(join_error) => Some(format!("hashing task failed: {join_error}")),
                Ok(Ok(_)) => None,
            };
            return Err(hashing_error
                .or(upload.err())
                .unwrap_or_else(|| "publishing the output failed".into()));
        }
    };
    if let Err(error) = operator.rename(&pending_path, &object_path).await {
        let _ = operator.delete(&pending_path).await;
        return Err(error.to_string());
    }

    Ok(FileRef::remote(
        format!("vfs://{virtual_path}"),
        spec.format.clone(),
        metadata.len(),
        Some(digest),
    ))
}

/// Persist a run's captured stdout/stderr at
/// `{artifact_prefix}/{run_name}/.autonomics-logs/`, beside the run's
/// published outputs, and return `FileRef`s (path + sha256) for the run
/// report.
///
/// Best-effort by design: missing object storage or a write failure only
/// warns — log retention must never fail an otherwise-successful execution.
/// Blank captures are skipped, matching when the live events are emitted.
async fn publish_run_logs(
    ctx: &NodeCtx,
    artifact_prefix: &str,
    run_name: &str,
    stdout: &str,
    stderr: &str,
    reporter: &NodeReporter,
) -> (Option<FileRef>, Option<FileRef>) {
    let Some(storage) = ctx.opendal.as_ref() else {
        reporter.warn("no object storage registered; run stdout/stderr not persisted");
        return (None, None);
    };
    let prefix = vfs::OpendalFileStorage::normalize_path(artifact_prefix.trim_end_matches('/'));
    let base = format!("{prefix}/{run_name}/.autonomics-logs");

    let stdout_ref = if stdout.trim().is_empty() {
        Ok(None)
    } else {
        write_log_object(storage, &format!("{base}/stdout.log"), stdout)
            .await
            .map(Some)
    };
    let stderr_ref = if stderr.trim().is_empty() {
        Ok(None)
    } else {
        write_log_object(storage, &format!("{base}/stderr.log"), stderr)
            .await
            .map(Some)
    };
    for result in [&stdout_ref, &stderr_ref] {
        if let Err(error) = result {
            reporter.warn(format!("cannot persist run log: {error}"));
        }
    }
    (stdout_ref.ok().flatten(), stderr_ref.ok().flatten())
}

/// Write one captured stream to object storage and describe it as a
/// content-addressed [`FileRef`]. The capture is already in memory (capped by
/// the runtime's output limit), so a single-shot digest suffices.
async fn write_log_object(
    storage: &vfs::OpendalFileStorage,
    virtual_path: &str,
    contents: &str,
) -> Result<FileRef, String> {
    let digest = format!("sha256:{}", hex(&Sha256::digest(contents.as_bytes())));
    let bytes = contents.as_bytes().to_vec();
    storage
        .write_bytes(virtual_path, bytes)
        .await
        .map_err(|error| error.to_string())?;
    Ok(FileRef::remote(
        format!("vfs://{virtual_path}"),
        Some("text/plain".to_string()),
        contents.len() as u64,
        Some(digest),
    ))
}

/// Assemble the execution evidence a container run reports for its node.
fn run_details(
    image: &str,
    exit_code: i32,
    run_name: &str,
    logs: (Option<FileRef>, Option<FileRef>),
) -> NodeRunDetails {
    NodeRunDetails {
        image: Some(image.to_string()),
        // References are digest-pinned on the plugin path, but the spec alone
        // does not enforce it — parse tolerantly and leave the digest unset
        // for tag-style references.
        image_digest: ImageReference::parse(image)
            .ok()
            .map(|reference| reference.digest().to_string()),
        exit_code: Some(exit_code),
        run_name: Some(run_name.to_string()),
        stdout_log: logs.0,
        stderr_log: logs.1,
        workspace: None,
        task_manifest: None,
        output_artifacts: Vec::new(),
    }
}
