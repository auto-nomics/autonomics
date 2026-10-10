//! The file-to-file container command node.
//!
//! Each task executes in a persistent, content-addressed work directory
//! mounted at `/work`: inputs are staged into it (symlinks to their sources
//! by default, with source parents bind-mounted read-only), the command runs
//! in an ephemeral image, and declared outputs are collected from the work
//! dir as DAG values — they stay there and travel downstream by reference.
//! An optional `artifact_prefix` additionally publishes them to VFS.
//! Construction validates the spec and resolves catalog panels.

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

use container_runtime::gc::{
    acquire_panel_lock_shared, acquire_scratch_lock_shared, acquire_workdir_lock_exclusive,
};
use container_runtime::{
    CachedPanel, ContainerNetwork, ContainerRunRequest, ContainerRuntimeError,
    DEFAULT_CONTAINER_WORKDIR, GpuRequest, ImageReference, InputMount, PanelCache, PanelRef,
    PodmanConnection, PullPolicy, unique_container_name, workspace_ref,
};

use super::ContainerCommandOutputSpec;
use super::StageInMode;
use super::error::ContainerCommandError;
use super::utils::{
    capture_declared_output_logs, capture_input_manifest, container_path, contains_glob, hex,
    input_path, literal_prefix, mount_safe_parent, resolve_output, staged_path, virtual_path,
    write_failure_logs, write_strictly_within,
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
    stage_in_mode: StageInMode,
    artifact_prefix: Option<String>,
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
    /// Identity of the plugin family this node was built from, when it was
    /// (the manifest-factory path). Participates in the execution
    /// fingerprint so plugin edits invalidate cached outputs (WO-R09).
    /// `None` for plain `container_command` nodes, whose whole behavior is
    /// already captured by their spec.
    plugin_identity: Option<dag_core::fingerprint::PluginIdentity>,
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
            stage_in_mode: spec.stage_in_mode,
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
            plugin_identity: None,
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

    /// Attach the identity of the plugin family this node was built from
    /// (WO-R09). Only the manifest-factory path sets this; the identity then
    /// rides on the node payload into the scheduler, where it joins the
    /// execution fingerprint — so editing the family's manifest, script, or
    /// image invalidates cached incremental outputs.
    pub fn with_plugin_identity(mut self, identity: dag_core::fingerprint::PluginIdentity) -> Self {
        self.plugin_identity = Some(identity);
        self
    }

    /// Content-addressed identity of this task: everything that determines
    /// the output bytes, hashed to the stable work-dir name.
    ///
    /// Resource knobs (`cpus`/`memory`/`timeout_secs`/`user`/...) and
    /// `artifact_prefix` deliberately do not participate — re-tuning them
    /// reuses the same work dir. Inputs participate through their
    /// `FileRef` identity (path + fingerprint), in the same port order the
    /// scheduler stages them.
    fn work_dir_identity(&self, inputs: &[NodeInput]) -> String {
        use sha2::Digest;

        let mut hasher = sha2::Sha256::new();
        let feed = |hasher: &mut sha2::Sha256, part: &str| {
            hasher.update(part.as_bytes());
            hasher.update([0x1e]);
        };
        feed(&mut hasher, self.kind);
        feed(&mut hasher, &self.image);
        feed(&mut hasher, &self.command.join("\x1e"));
        feed(&mut hasher, self.script.as_deref().unwrap_or(""));
        for (name, content) in &self.files {
            feed(&mut hasher, name);
            feed(&mut hasher, content);
        }
        for (name, value) in &self.env {
            feed(&mut hasher, name);
            feed(&mut hasher, value);
        }
        for output in &self.outputs {
            feed(&mut hasher, &output.path);
            feed(&mut hasher, output.format.as_deref().unwrap_or(""));
        }
        feed(
            &mut hasher,
            match self.stage_in_mode {
                StageInMode::Symlink => "symlink",
                StageInMode::Copy => "copy",
            },
        );
        for panel in &self.panels {
            feed(&mut hasher, &panel.id);
            feed(&mut hasher, &panel.digest);
            feed(&mut hasher, &panel.mount_path);
        }
        for panel in &self.panel_bundles {
            feed(&mut hasher, &panel.spec.panel_id);
            feed(&mut hasher, &panel.spec.mount_path);
            feed(&mut hasher, panel.bundle.digest.as_deref().unwrap_or(""));
            feed(&mut hasher, panel.bundle.source.as_deref().unwrap_or(""));
        }
        if let Some(identity) = &self.plugin_identity {
            feed(&mut hasher, &identity.manifest_sha256);
            feed(&mut hasher, identity.script_sha256.as_deref().unwrap_or(""));
            feed(&mut hasher, &identity.image_reference);
            for panel in &identity.panels {
                feed(&mut hasher, &format!("{panel:?}"));
            }
        }
        let mut ordered = inputs.to_vec();
        ordered.sort_by_key(|input| input.port);
        for input in &ordered {
            feed(&mut hasher, &input.port.to_string());
            match &input.data {
                NodeValue::File(file) => feed_file(&mut hasher, file),
                NodeValue::FileSet(files) => {
                    for file in files {
                        feed_file(&mut hasher, file);
                    }
                }
                NodeValue::DataFrame(_) | NodeValue::Channel(_) => {
                    feed(&mut hasher, "<unsupported>")
                }
            }
        }
        hex_digest(&hasher)
    }

    /// Resolves the host work directory that will be mounted at `/work`.
    ///
    /// When the spec omits `workdir`, a persistent content-addressed
    /// directory `{hash[0..2]}/{hash[2..]}` under the workspace root is derived from
    /// [`Self::work_dir_identity`] — identical re-runs of the same task
    /// reuse it (Nextflow-style work dir). A spec-provided `workdir` is an
    /// explicit override (used as-is when absolute, joined onto
    /// `workspace_root` when relative). Missing directories are created on
    /// demand, the path is canonicalized, and any candidate that escapes
    /// `workspace_root` (an absolute path elsewhere, `..` traversal, or a
    /// symlink pointing out) is rejected before it can be mounted into the
    /// container.
    fn resolve_workdir(
        &self,
        workspace_root: &Path,
        identity: &str,
    ) -> Result<PathBuf, ContainerCommandError> {
        let workdir = match &self.workdir {
            Some(path) => {
                let candidate = PathBuf::from(path);
                if candidate.is_absolute() {
                    candidate
                } else {
                    workspace_root.join(candidate)
                }
            }
            None => workspace_root.join(&identity[..2]).join(&identity[2..]),
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

fn feed_file(hasher: &mut sha2::Sha256, file: &FileRef) {
    use sha2::Digest;

    hasher.update(file.path.as_bytes());
    hasher.update([0x1f]);
    match &file.fingerprint {
        Some(fingerprint) => {
            hasher.update(fingerprint.size.to_le_bytes());
            hasher.update(fingerprint.mtime_ns.to_le_bytes());
            hasher.update(fingerprint.content_hash.as_deref().unwrap_or("").as_bytes());
            hasher.update([u8::from(fingerprint.immutable_remote)]);
        }
        None => hasher.update([0]),
    }
    hasher.update([0x1f]);
}

fn hex_digest(hasher: &sha2::Sha256) -> String {
    use sha2::Digest;

    hasher
        .clone()
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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

/// Stage one input file into the work dir staging area.
///
/// Returns the staged path plus, when the input was staged as a symlink, the
/// source parent directory that must be bind-mounted read-only into the
/// container so the symlink resolves inside it.
///
/// Resolution order: `vfs://` objects resolve through the VFS mount table to
/// a host file on a local backend; bare host paths are used directly (host
/// existence wins over the VFS — the root mount would otherwise claim every
/// absolute path into the data dir). Remote-backend objects, `copy` mode, and
/// unsafe source locations (tmpfs and friends) fall back to byte copies.
async fn stage_input_file(
    ctx: &NodeCtx,
    staging_dir: &Path,
    index: usize,
    file: &FileRef,
    mode: StageInMode,
) -> Result<(PathBuf, Option<PathBuf>), String> {
    let destination = staged_path(staging_dir, "input", index, &file.path);

    let host_source: Option<PathBuf> =
        if let Some(virtual_source) = file.path.strip_prefix("vfs://") {
            ctx.opendal
                .as_ref()
                .and_then(|storage| storage.local_host_path(virtual_source))
                .filter(|path| path.is_file())
        } else {
            let bare = file.path.strip_prefix("file://").unwrap_or(&file.path);
            let direct = PathBuf::from(bare);
            if direct.is_file() {
                Some(direct)
            } else {
                ctx.opendal
                    .as_ref()
                    .and_then(|storage| storage.local_host_path(bare))
                    .filter(|path| path.is_file())
            }
        };

    if let Some(source) = host_source {
        let parent = source.parent().map(Path::to_path_buf);
        let symlink_ok =
            mode == StageInMode::Symlink && parent.as_deref().is_some_and(mount_safe_parent);
        if symlink_ok {
            let staged = destination.clone();
            let source_display = source.display().to_string();
            let moved_source = source.clone();
            let linked = tokio::task::spawn_blocking(move || {
                // Replace any pre-existing link/file from a previous run in
                // the same work dir before linking.
                match std::fs::remove_file(&staged) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(&moved_source, &staged).map_err(|e| e.to_string())
                }
                #[cfg(not(unix))]
                {
                    Err("symlink staging is unsupported on this platform".to_string())
                }
            })
            .await
            .map_err(|e| format!("staging task failed: {e}"))
            .and_then(|result| result);
            linked.map_err(|e| {
                format!(
                    "cannot symlink-stage input `{source_display}` -> `{}`: {e}",
                    destination.display()
                )
            })?;
            return Ok((destination, parent));
        }

        // Copy mode, or the source parent is not mount-safe.
        let staged = destination.clone();
        let copied = tokio::task::spawn_blocking(move || {
            std::fs::copy(&source, &staged)
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("staging task failed: {e}"))
        .and_then(|result| result);
        copied.map_err(|e| format!("cannot stage input `{}`: {e}", file.path))?;
        return Ok((destination, None));
    }

    // No host file: remote-backend VFS object — transfer the bytes.
    if let Some(storage) = ctx.opendal.as_ref()
        && let Some(virtual_source) = virtual_path(&file.path)
    {
        let operator = storage.resolve(&virtual_source);
        let key = storage.resolve_path(&virtual_source);
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
            return Ok((destination, None));
        }
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
    mode: StageInMode,
) -> Result<(Vec<NodeInput>, Vec<InputMount>), String> {
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
    let mut mount_dirs = std::collections::BTreeSet::new();
    for input in &ordered_inputs {
        let value = match &input.data {
            NodeValue::File(file) => {
                let (path, mount) = stage_input_file(ctx, &staging_dir, index, file, mode).await?;
                index += 1;
                if let Some(dir) = mount
                    && dir != workdir
                {
                    mount_dirs.insert(dir);
                }
                NodeValue::File(FileRef::new(
                    path.to_string_lossy().into_owned(),
                    file.format.clone(),
                ))
            }
            NodeValue::FileSet(files) => {
                let mut staged_files = Vec::with_capacity(files.len());
                for file in files {
                    let (path, mount) =
                        stage_input_file(ctx, &staging_dir, index, file, mode).await?;
                    index += 1;
                    if let Some(dir) = mount
                        && dir != workdir
                    {
                        mount_dirs.insert(dir);
                    }
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
    let input_mounts = mount_dirs
        .into_iter()
        .map(|host_dir| InputMount { host_dir })
        .collect();
    Ok((staged, input_mounts))
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
            stage_in_mode: self.stage_in_mode,
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
            plugin_identity: self.plugin_identity.clone(),
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

    fn plugin_identity(&self) -> Option<&dag_core::fingerprint::PluginIdentity> {
        self.plugin_identity.as_ref()
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        // Canonicalize the root before any use: workdirs are canonicalized
        // (create + resolve symlinks), so containment checks, workspace_ref
        // mapping, and the VFS identity mount must all share the canonical
        // root — a symlinked state directory otherwise makes every workdir
        // look like it escapes the root.
        let workspace_root = container_runtime::canonicalize_workspace_root(
            self.runtime.workspace_root().to_path_buf(),
        );
        let identity = self.work_dir_identity(inputs);
        let workspace_path = self
            .resolve_workdir(&workspace_root, &identity)
            .map_err(|error| error.into_dag_error(self.kind))?;
        let is_hash_workdir = self.workdir.is_none();
        // Hold the work-dir lock for the whole run: the GC sweep takes the
        // same lock exclusively and therefore never reclaims a live run's
        // directory. Hash work dirs take it exclusively (concurrent identical
        // tasks serialize) and are wiped of stale contents first; explicit
        // user workdirs keep the shared lock and their contents.
        let workdir_lock = if is_hash_workdir {
            let lock = acquire_workdir_lock_exclusive(&workspace_path).map_err(|error| {
                ContainerCommandError::Invalid(format!(
                    "cannot lock container work dir `{}`: {error}",
                    workspace_path.display()
                ))
            });
            lock.map_err(|error| error.into_dag_error(self.kind))?
        } else {
            let lock = acquire_scratch_lock_shared(&workspace_path).map_err(|error| {
                ContainerCommandError::Invalid(format!(
                    "cannot lock container workspace `{}`: {error}",
                    workspace_path.display()
                ))
            });
            lock.map_err(|error| error.into_dag_error(self.kind))?
        };
        if is_hash_workdir {
            wipe_workdir_contents(&workspace_path)
                .await
                .map_err(ContainerCommandError::Invalid)
                .map_err(|error| error.into_dag_error(self.kind))?;
        }
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

        let (mut staged_inputs, input_mounts) =
            stage_inputs(ctx, &workspace_path, inputs, self.stage_in_mode)
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

        // Literal outputs resolve to concrete paths before the run; glob
        // outputs cannot (the files do not exist yet), so they substitute
        // their pattern relative to `/work` and only the literal directory
        // prefix is pre-created.
        let declared_outputs = self
            .outputs
            .iter()
            .map(|output| {
                let container_ref = if contains_glob(&output.path) {
                    format!("{DEFAULT_CONTAINER_WORKDIR}/{}", output.path)
                } else {
                    container_path(
                        &workspace_path,
                        &workspace_path.join(&output.path).to_string_lossy(),
                    )
                };
                (output, container_ref)
            })
            .collect::<Vec<_>>();
        for (spec, _) in &declared_outputs {
            let mkdir_target = if contains_glob(&spec.path) {
                literal_prefix(&spec.path)
            } else {
                workspace_path.join(&spec.path).parent().map(|parent| {
                    parent
                        .strip_prefix(&workspace_path)
                        .unwrap_or(parent)
                        .to_string_lossy()
                        .into_owned()
                })
            };
            if let Some(relative) = mkdir_target
                && !relative.is_empty()
            {
                std::fs::create_dir_all(workspace_path.join(&relative))
                    .map_err(|e| {
                        ContainerCommandError::Invalid(format!(
                            "cannot create output directory `{relative}`: {e}"
                        ))
                    })
                    .map_err(|error| error.into_dag_error(self.kind))?;
            }
        }
        // Glob outputs substitute their `/work`-relative pattern into
        // `$outputN` / `AUTONOMICS_OUTPUTn` — the pattern cannot be resolved
        // before the container runs.
        let container_output_paths: Vec<String> = declared_outputs
            .iter()
            .map(|(_, reference)| reference.clone())
            .collect();

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
            if let Some(path) = container_output_paths.get(index) {
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
        for (index, path) in container_output_paths.iter().enumerate() {
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
            input_mounts,
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
                        // Persist the captured streams in the work dir so a
                        // failed execution is as auditable as a successful
                        // one; the persisted copies are the audit record.
                        let logs =
                            write_run_logs(&workspace_path, &stdout, &stderr, reporter).await;
                        reporter.set_run_details(run_details(
                            &self.image,
                            exit_code,
                            &request_name,
                            &workspace_path,
                            logs,
                        ));
                        let literal_outputs = declared_outputs
                            .iter()
                            .filter(|(spec, _)| !contains_glob(&spec.path))
                            .map(|(spec, _)| (*spec, workspace_path.join(&spec.path)))
                            .collect::<Vec<_>>();
                        let mut logs = capture_declared_output_logs(&literal_outputs);
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
        // Persist the captured streams in the work dir before the live
        // events below consume them — the events are best-effort and
        // disappear with the channel, the persisted copies are the audit
        // record.
        let logs = write_run_logs(&workspace_path, &result.stdout, &result.stderr, reporter).await;
        reporter.set_run_details(run_details(
            &self.image,
            result.exit_code,
            &request_name,
            &workspace_path,
            logs,
        ));
        if !result.stdout.trim().is_empty() {
            reporter.info(result.stdout);
        }
        if !result.stderr.trim().is_empty() {
            reporter.warn(result.stderr);
        }

        // Collect declared outputs from the work dir — literal paths keep
        // the exactly-one-file contract, glob patterns must match exactly
        // one file. Outputs travel downstream as work-dir `FileRef`s (mtime
        // fingerprints, existence-checked on incremental reuse); an optional
        // `artifact_prefix` additionally publishes them to VFS.
        let mut outputs = PortOutputs::new();
        for (index, (spec, _)) in declared_outputs.iter().enumerate() {
            let host_path = resolve_output(&spec.path, &workspace_path)
                .map_err(|error| error.into_dag_error(self.kind))?;
            if let Some(prefix) = &self.artifact_prefix {
                publish_output(ctx, prefix, &request_name, spec, &host_path)
                    .await
                    .map_err(ContainerCommandError::Invalid)
                    .map_err(|error| error.into_dag_error(self.kind))?;
            }
            let file = FileRef::local(&host_path, spec.format.clone())
                .map_err(|error| {
                    ContainerCommandError::Invalid(format!(
                        "cannot fingerprint output `{}`: {error}",
                        host_path.display()
                    ))
                })
                .map_err(|error| error.into_dag_error(self.kind))?;
            outputs.insert_file(index as u8, file);
        }

        // The work dir is persistent by design (it is the cache of this
        // task's outputs); nothing to clean up here. The GC sweep reclaims
        // stale work dirs only through the opt-in age knob, and never while
        // the lock held above is alive.
        drop(workdir_lock);
        drop(_panel_locks);
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

/// Persist a run's captured stdout/stderr in the work dir at
/// `.autonomics/logs/{stdout,stderr}.log` and return `FileRef`s for the run
/// report.
///
/// Best-effort by design: a write failure only warns — log retention must
/// never fail an otherwise-successful execution. Blank captures are skipped,
/// matching when the live events are emitted.
async fn write_run_logs(
    workdir: &Path,
    stdout: &str,
    stderr: &str,
    reporter: &NodeReporter,
) -> (Option<FileRef>, Option<FileRef>) {
    let log_dir = workdir.join(".autonomics").join("logs");
    let write = |name: &str, contents: &str| -> Option<FileRef> {
        if contents.trim().is_empty() {
            return None;
        }
        std::fs::create_dir_all(&log_dir).ok()?;
        let path = log_dir.join(name);
        std::fs::write(&path, contents).ok()?;
        FileRef::local(&path, Some("text/plain".into())).ok()
    };
    let result = (write("stdout.log", stdout), write("stderr.log", stderr));
    if result.0.is_none() && !stdout.trim().is_empty()
        || result.1.is_none() && !stderr.trim().is_empty()
    {
        reporter.warn("cannot persist run logs in the work dir");
    }
    result
}

/// Assemble the execution evidence a container run reports for its node.
fn run_details(
    image: &str,
    exit_code: i32,
    run_name: &str,
    workdir: &Path,
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
        workspace: Some(workdir.to_string_lossy().into_owned()),
        task_manifest: None,
        output_artifacts: Vec::new(),
        output_artifacts_by_port: Default::default(),
    }
}

/// Remove every entry in a hash work dir except the lock file, so a re-run
/// into the same directory can never observe stale outputs from a previous
/// (possibly crashed) attempt.
async fn wipe_workdir_contents(workdir: &Path) -> Result<(), String> {
    let dir = workdir.to_path_buf();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_name() == ".autonomics-lock" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| format!("workdir wipe task failed: {error}"))
    .and_then(|result| {
        result.map_err(|error| format!("cannot wipe work dir `{}`: {error}", workdir.display()))
    })
}
