//! Ordinary typed-node execution in an independent, pinned worker process.
//!
//! This is deliberately separate from the signed research worker. It conveys
//! no scientific authorization and cannot consume formal runtime projections.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arrow::ipc::writer::FileWriter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{TaskDispatch, TaskExecution, TaskExecutor, TaskSpec};
use crate::dag::node_event::{JobResult, NodeReporter};
use crate::dag::{DagError, NodeInput};
use crate::registry::NodeRegistry;
use crate::value::{FileRef, NodeValue};

fn failure(message: impl Into<String>) -> DagError {
    DagError::Schedule(message.into())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Explicit private Podman backend for an ordinary worker. No daemon/formal
/// backend is selected by this record and no host image store is reused.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum TypedPodmanResourceControl {
    #[default]
    DelegatedCgroups,
    SlurmAllocation {
        swap_policy: TypedSlurmSwapPolicy,
    },
}

impl<'de> Deserialize<'de> for TypedPodmanResourceControl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // A serde unit variant ignores extra map fields even with
        // deny_unknown_fields. Decode an empty struct variant instead, then
        // preserve the public enum/API and its existing serialized shape.
        #[derive(Deserialize)]
        #[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            DelegatedCgroups {},
            SlurmAllocation { swap_policy: TypedSlurmSwapPolicy },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::DelegatedCgroups {} => Self::DelegatedCgroups,
            Wire::SlurmAllocation { swap_policy } => Self::SlurmAllocation { swap_policy },
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedSlurmSwapPolicy {
    Inherit,
    Disabled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedPodmanUserNamespace {
    #[default]
    KeepId,
    SingleUidRoot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedPodmanSettings {
    pub executable: PathBuf,
    pub executable_sha256: String,
    pub image_catalog: Option<PathBuf>,
    pub image_catalog_sha256: Option<String>,
    pub panel_cache_root: Option<PathBuf>,
    pub oci_runtime: Option<PathBuf>,
    pub oci_runtime_sha256: Option<String>,
    #[serde(default)]
    pub resource_control: TypedPodmanResourceControl,
    #[serde(default)]
    pub user_namespace: TypedPodmanUserNamespace,
}

impl TypedPodmanSettings {
    pub fn validate(&self) -> Result<(), DagError> {
        if !self.executable.is_absolute() || !valid_sha256(&self.executable_sha256) {
            return Err(failure(
                "typed Podman requires an absolute executable and SHA256 pin",
            ));
        }
        match (&self.image_catalog, &self.image_catalog_sha256) {
            (None, None) => (),
            (Some(path), Some(hash)) if path.is_absolute() && valid_sha256(hash) => (),
            _ => {
                return Err(failure(
                    "typed Podman image catalog requires an absolute path and SHA256 pin",
                ));
            }
        }
        match (&self.oci_runtime, &self.oci_runtime_sha256) {
            (None, None)
                if matches!(
                    self.resource_control,
                    TypedPodmanResourceControl::DelegatedCgroups
                ) => {}
            (Some(path), Some(hash)) if path.is_absolute() && valid_sha256(hash) => (),
            _ => {
                return Err(failure(
                    "typed Podman OCI runtime requires an absolute path and SHA256 pin; Slurm allocation requires this pair",
                ));
            }
        }
        if self
            .panel_cache_root
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err(failure("typed Podman panel cache must be absolute"));
        }
        Ok(())
    }
}

/// Explicit operator-owned credential and pinned network-policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedWorkerEnvironment {
    pub python: PathBuf,
    pub file: PathBuf,
    /// Public policy identity; credentials remain solely in the private file.
    #[serde(default)]
    pub network_policy_sha256: Option<String>,
}

/// Operator-owned cluster deployment pins. No paths are inferred from the
/// coordinator HOME, and absent plugin configuration loads no ambient plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedWorkerSettings {
    pub executable: PathBuf,
    pub executable_sha256: String,
    pub expected_source_revision: String,
    pub plugins_root: Option<PathBuf>,
    pub plugins_sha256: Option<String>,
    pub apptainer_catalog: Option<PathBuf>,
    pub apptainer_executable: Option<PathBuf>,
    #[serde(default)]
    pub podman: Option<TypedPodmanSettings>,
    pub worker_vfs_manifest: Option<PathBuf>,
    pub worker_vfs_sha256: Option<String>,
    /// Operator-owned 0600 file. Allowlisted API credentials and an optional
    /// network allowlist path/SHA256 pair; values never enter task arguments.
    #[serde(default)]
    pub native_environment: Option<TypedWorkerEnvironment>,
}

impl TypedWorkerSettings {
    pub fn validate(&self) -> Result<(), DagError> {
        if let Some(environment) = &self.native_environment {
            if environment
                .network_policy_sha256
                .as_ref()
                .is_some_and(|pin| !valid_sha256(pin))
            {
                return Err(failure("native worker network policy requires SHA256"));
            }
            for path in [&environment.python, &environment.file] {
                if !path.is_absolute()
                    || path == Path::new("/")
                    || path.to_string_lossy().chars().any(char::is_control)
                    || path
                        .to_string_lossy()
                        .split('/')
                        .skip(1)
                        .any(|p| p.is_empty() || p == "." || p == "..")
                {
                    return Err(failure(
                        "native worker environment requires canonical absolute paths",
                    ));
                }
            }
        }
        if !self.executable.is_absolute() || !valid_sha256(&self.executable_sha256) {
            return Err(failure(
                "typed worker requires an absolute executable and SHA256 pin",
            ));
        }
        if self.expected_source_revision.trim().is_empty()
            || self.expected_source_revision == "unknown"
        {
            return Err(failure("typed worker requires a known source revision"));
        }
        match (&self.plugins_root, &self.plugins_sha256) {
            (None, None) => (),
            (Some(root), Some(hash)) if root.is_absolute() && valid_sha256(hash) => (),
            _ => {
                return Err(failure(
                    "typed worker plugins require an absolute root and SHA256 tree pin",
                ));
            }
        }
        match (&self.apptainer_catalog, &self.apptainer_executable) {
            (None, None) => (),
            (Some(catalog), Some(executable))
                if catalog.is_absolute() && executable.is_absolute() => {}
            _ => {
                return Err(failure(
                    "typed Apptainer requires absolute catalog and executable paths",
                ));
            }
        }
        if let Some(podman) = &self.podman {
            podman.validate()?;
            if self.apptainer_catalog.is_some() || self.apptainer_executable.is_some() {
                return Err(failure(
                    "typed Podman and Apptainer backends are mutually exclusive",
                ));
            }
        }
        if self.plugins_root.is_some() && self.podman.is_none() && self.apptainer_catalog.is_none()
        {
            return Err(failure(
                "ordinary plugins require an explicit Apptainer or pinned Podman backend",
            ));
        }
        match (&self.worker_vfs_manifest, &self.worker_vfs_sha256) {
            (None, None) => (),
            (Some(path), Some(hash)) if path.is_absolute() && valid_sha256(hash) => (),
            _ => {
                return Err(failure(
                    "typed worker VFS requires an absolute manifest and SHA256 pin",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedWorkerEnvelope {
    pub protocol: u32,
    pub worker: TypedWorkerSettings,
    pub task: TaskSpec,
    pub input_files: BTreeMap<u8, Vec<FileRef>>,
    /// Spec JSON pointer -> transport-only input name. These are not node ports.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub spec_files: BTreeMap<String, String>,
    /// Declared directory publishers, transported back before VFS publication.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub output_directories: BTreeMap<String, u8>,
    pub bundles: Vec<crate::DataBundle>,
    pub plugin_identity: Option<crate::fingerprint::PluginIdentity>,
    /// Filled exclusively from the coordinator's execution storage, never task
    /// spec fields. Older envelopes are accepted only without a worker VFS.
    #[serde(default)]
    pub original_vfs_principal: Option<vfs::permission::VfsPrincipal>,
}

impl TypedWorkerEnvelope {
    pub fn vfs_principal(&self) -> Result<Option<vfs::permission::VfsPrincipal>, DagError> {
        if self.worker.worker_vfs_manifest.is_some() && self.original_vfs_principal.is_none() {
            return Err(failure(
                "worker VFS requires the original coordinator VFS principal",
            ));
        }
        if let Some(principal) = &self.original_vfs_principal {
            principal.validate_transport().map_err(failure)?;
        }
        Ok(self.original_vfs_principal.clone())
    }
}

/// Wraps the existing real process transport, preserving its resource limits,
/// submission identity, cancellation, staging, output typing and receipts.
pub struct TypedRemoteTaskExecutor {
    remote: Arc<dyn TaskExecutor>,
    settings: TypedWorkerSettings,
}

impl TypedRemoteTaskExecutor {
    pub fn new(
        remote: Arc<dyn TaskExecutor>,
        settings: TypedWorkerSettings,
    ) -> Result<Self, DagError> {
        settings.validate()?;
        Ok(Self { remote, settings })
    }

    fn prepare(&self, execution: &mut TaskExecution) -> Result<(), DagError> {
        if execution.engine_ctx.runtime_output_projection.is_some() {
            return Err(failure(
                "formal runtime projections require the signed research worker",
            ));
        }
        lower_worker_task(execution, &self.settings, None)
    }
}

/// Transport lowering only. Scientific admission belongs to the private
/// producer; this internal compiler cannot submit or authorize a task.
pub(super) fn lower_worker_task(
    execution: &mut TaskExecution,
    settings: &TypedWorkerSettings,
    binding: Option<&super::private_native::PrivateNativeBinding>,
) -> Result<(), DagError> {
    execution
        .node
        .prepare_task_inputs(&mut execution.submission.inputs)?;
    if let Some(reason) = execution.node.remote_execution_blocker() {
        return Err(failure(format!("remote_service_unavailable: {reason}")));
    }
    settings.validate()?;
    if matches!(
        execution.node.kind(),
        "http_fetch" | "http_head" | "file_download"
    ) && execution.node.plugin_identity().is_none()
        && settings
            .native_environment
            .as_ref()
            .and_then(|environment| environment.network_policy_sha256.as_ref())
            .is_none()
    {
        return Err(failure(
            "worker_network_policy_unconfigured: enroll an approved policy SHA256 and private environment before remote HTTP execution",
        ));
    }
    if let Some(binding) = binding {
        binding.validate_for_task(execution, settings)?;
    }
    let required = execution.node.task_resources()?;
    required.validate()?;
    let requested = &mut execution.submission.resources;
    requested.validate()?;
    if requested
        .cpus
        .zip(required.cpus)
        .is_some_and(|(provided, minimum)| provided < minimum)
        || requested
            .memory_bytes
            .zip(required.memory_bytes)
            .is_some_and(|(provided, minimum)| provided < minimum)
    {
        return Err(failure(
            "declared task CPU/memory budget is smaller than the compiled node request",
        ));
    }
    requested.cpus = requested.cpus.or(required.cpus);
    requested.memory_bytes = requested.memory_bytes.or(required.memory_bytes);
    // A caller may intentionally impose a shorter outer timeout. The
    // node's own timeout supplies the default, never expands that budget.
    requested.max_duration_ms = requested.max_duration_ms.or(required.max_duration_ms);
    if execution.submission.spec.kind == "process" {
        return Ok(());
    }
    if crate::source_revision() != settings.expected_source_revision {
        return Err(failure(format!(
            "typed worker source pin differs from coordinator source revision: worker={}, coordinator={}; deploy the measured matching worker and enroll a new target revision; preserve historical pins",
            settings.expected_source_revision,
            crate::source_revision(),
        )));
    }
    // Use the actual runtime input type, including physical scatter inputs.
    for binding in &mut execution.submission.spec.inputs {
        if let Some(input) = execution
            .submission
            .inputs
            .iter()
            .find(|input| input.port == binding.port)
        {
            binding.payload = match &input.data {
                NodeValue::Structured(_) => "structured_channel".into(),
                _ => input.data.data_type().to_string(),
            };
        }
    }
    if execution.submission.spec.kind == "logical_gather" {
        let payload =
            gather_output_payload(execution.submission.inputs.iter().map(
                |input| match &input.data {
                    NodeValue::DataFrame(_) => "dataframe",
                    NodeValue::File(_) => "file",
                    NodeValue::FileSet(_) => "file_set",
                    NodeValue::Channel(_) => "channel",
                    NodeValue::Structured(_) => "structured_channel",
                },
            ))?;
        if execution.submission.spec.outputs.len() != 1
            || execution.submission.spec.outputs[0].port != 0
        {
            return Err(failure("logical gather requires output port zero only"));
        }
        execution.submission.spec.outputs[0].payload = payload.into();
    } else if execution
        .submission
        .spec
        .outputs
        .iter()
        .any(|output| output.payload == "any")
    {
        return Err(failure("remote typed output requires a concrete contract"));
    }
    let original_vfs_principal = execution
        .engine_ctx
        .opendal
        .as_ref()
        .map(|storage| storage.principal().clone());
    let mut envelope = TypedWorkerEnvelope {
        protocol: 1,
        original_vfs_principal,
        worker: settings.clone(),
        task: execution.submission.spec.clone(),
        spec_files: BTreeMap::new(),
        output_directories: BTreeMap::new(),
        plugin_identity: execution.node.plugin_identity().cloned(),
        bundles: execution
            .engine_ctx
            .bundle_registry
            .iter()
            .map(|(_, bundle)| bundle.clone())
            .collect(),
        input_files: execution
            .submission
            .inputs
            .iter()
            .filter_map(|input| match &input.data {
                NodeValue::File(file) => Some((input.port, vec![file.clone()])),
                NodeValue::FileSet(files) => Some((input.port, files.clone())),
                NodeValue::Structured(value) => {
                    Some((input.port, value.artifacts().into_iter().cloned().collect()))
                }
                _ => None,
            })
            .collect(),
    };
    envelope.vfs_principal()?;
    if binding.is_none() {
        for (pointer, port) in execution.node.file_output_directory_spec_pointers() {
            let path = envelope
                .task
                .spec
                .pointer(pointer)
                .and_then(serde_json::Value::as_str)
                .filter(|path| !path.trim().is_empty())
                .ok_or_else(|| failure("output directory must name a nonempty path"))?;
            let Some(storage) = execution.engine_ctx.opendal.as_ref() else {
                continue;
            };
            let path = if let Some(path) = vfs::uri::virtual_path(path) {
                path.to_owned()
            } else if !path.contains("://") && storage.is_mounted(path) {
                path.to_owned()
            } else {
                continue;
            };
            let path = vfs::OpendalFileStorage::normalize_path(&path);
            storage
                .check_writable(&path)
                .map_err(|e| failure(e.to_string()))?;
            if !envelope
                .task
                .outputs
                .iter()
                .any(|out| out.port == port && out.payload == "file_set")
                || envelope
                    .output_directories
                    .values()
                    .any(|existing| *existing == port)
            {
                return Err(failure(
                    "directory publication requires a unique FileSet port",
                ));
            }
            envelope.output_directories.insert(pointer.into(), port);
            *envelope.task.spec.pointer_mut(pointer).unwrap() =
                serde_json::json!(vfs::uri::uri(path));
        }
    }
    for pointer in execution
        .node
        .file_input_spec_pointers(&execution.submission.inputs)
    {
        let path = envelope
            .task
            .spec
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| {
                failure(format!(
                    "file input spec pointer `{pointer}` must name a nonempty path"
                ))
            })?;
        let port = (0..=u8::MAX)
            .find(|port| {
                !execution
                    .submission
                    .spec
                    .inputs
                    .iter()
                    .any(|input| input.port == *port)
            })
            .ok_or_else(|| failure("no transport input slot available for spec file"))?;
        let name = format!("_autonomics_spec_file_{port}");
        if execution
            .submission
            .spec
            .inputs
            .iter()
            .any(|input| input.name == name)
            || envelope
                .spec_files
                .insert(pointer.into(), name.clone())
                .is_some()
        {
            return Err(failure("duplicate spec file transport binding"));
        }
        let source = if vfs::uri::is_uri(path) {
            path.to_owned()
        } else if !path.starts_with("file://")
            && execution
                .engine_ctx
                .opendal
                .as_ref()
                .is_some_and(|storage| storage.is_mounted(path))
        {
            vfs::uri::uri(path)
        } else {
            path.to_owned()
        };
        let format = Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_owned);
        execution
            .submission
            .inputs
            .push(NodeInput::file(port, FileRef::new(&source, format)));
        execution
            .submission
            .spec
            .inputs
            .push(super::TaskInputBinding {
                name,
                port,
                source: super::TaskInputSource::SpecFile {
                    pointer: pointer.into(),
                },
                payload: "file".into(),
                path: Some(source),
                fingerprint: None,
                staged_paths: vec![],
            });
    }
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let executable = quote(&settings.executable.to_string_lossy());
    let hash = quote(&settings.executable_sha256.to_ascii_lowercase());
    let invocation = if let Some(environment) = &settings.native_environment {
        let policy = environment
            .network_policy_sha256
            .as_ref()
            .map(|pin| format!("--network-policy-sha256 {} ", quote(pin)))
            .unwrap_or_default();
        format!(
            "{} -I -B -c {} {} {policy}{executable}",
            quote(&environment.python.to_string_lossy()),
            quote(include_str!("typed_worker_env.py")),
            quote(&environment.file.to_string_lossy())
        )
    } else {
        executable.clone()
    };
    let entry = if binding.is_some() {
        "delegated-native-compute"
    } else {
        "task-worker --dispatch"
    };
    let script = format!(
        "set -euo pipefail\nactual=$(sha256sum -- {executable})\n[ \"${{actual%% *}}\" = {hash} ] || {{ echo 'typed worker executable SHA256 mismatch' >&2; exit 126; }}\nexec {invocation} {entry} \"$AUTONOMICS_WORKSPACE/task.json\"\n"
    );
    let outputs = execution.submission.spec.outputs.iter().map(|binding| {
            let optional = execution.node.ports().output_port(binding.port).is_some_and(|port| !port.required);
            serde_json::json!({"port": binding.port, "pattern": format!("typed-outputs/port-{}/*", binding.port), "optional": optional})
        }).collect::<Vec<_>>();
    execution.submission.spec.kind = "process".into();
    execution.submission.spec.spec = serde_json::json!({
        "script": script, "outputs": outputs, "typed_dispatch": envelope,
    });
    if let Some(binding) = binding {
        let value = execution.submission.spec.spec.as_object_mut().unwrap();
        let envelope = value.remove("typed_dispatch").unwrap();
        value.insert("private_typed_dispatch".into(), envelope);
        value.insert(
            "private_native_context".into(),
            serde_json::to_value(binding).map_err(|e| failure(e.to_string()))?,
        );
        value["outputs"].as_array_mut().unwrap().push(serde_json::json!({"port":binding.metadata_port,"pattern":format!("typed-outputs/port-{}/*",binding.metadata_port)}));
        execution
            .submission
            .spec
            .outputs
            .push(super::TaskOutputBinding {
                name: "private-native-publication".into(),
                port: binding.metadata_port,
                payload: "file".into(),
                artifact_paths: vec![],
            });
    }
    Ok(())
}

fn rejected(execution: TaskExecution, error: DagError) -> JobResult {
    JobResult::Failed {
        id: execution.submission.spec.id,
        error,
        duration: Duration::ZERO,
        details: execution.reporter.take_run_details(),
    }
}

#[async_trait::async_trait]
impl TaskExecutor for TypedRemoteTaskExecutor {
    fn requires_cancel_join(&self) -> bool {
        self.remote.requires_cancel_join()
    }
    fn name(&self) -> &'static str {
        "typed-remote"
    }

    async fn run(&self, mut execution: TaskExecution) -> JobResult {
        if let Err(error) = self.prepare(&mut execution) {
            return rejected(execution, error);
        }
        self.remote.run(execution).await
    }
}

fn gather_output_payload<'a>(
    payloads: impl IntoIterator<Item = &'a str>,
) -> Result<&'static str, DagError> {
    let mut payloads = payloads.into_iter();
    let first = payloads
        .next()
        .ok_or_else(|| failure("remote logical gather requires an input"))?;
    if payloads.any(|payload| payload != first) {
        return Err(failure("remote logical gather requires homogeneous inputs"));
    }
    match first {
        "dataframe" => Ok("dataframe"),
        "file" | "file_set" => Ok("file_set"),
        _ => Err(failure(
            "remote logical gather only supports DataFrame, File or FileSet inputs",
        )),
    }
}

/// Deployment placement policy. Operator-listed kinds execute locally. When
/// explicitly enabled, container adapters prepare requests on the coordinator
/// for the bound remote container backend. Other kinds use the typed worker.
pub struct PlacementTaskExecutor {
    local: Arc<dyn TaskExecutor>,
    remote: Option<Arc<dyn TaskExecutor>>,
    local_kinds: BTreeSet<String>,
    remote_container_dispatch: bool,
}

impl PlacementTaskExecutor {
    pub fn new(
        local: Arc<dyn TaskExecutor>,
        remote: Option<Arc<dyn TaskExecutor>>,
        local_kinds: impl IntoIterator<Item = String>,
    ) -> Result<Self, DagError> {
        let local_kinds = local_kinds.into_iter().collect::<BTreeSet<_>>();
        if local_kinds
            .iter()
            .any(|kind| kind.trim().is_empty() || kind == "process")
        {
            return Err(failure(
                "placement local kinds must be nonempty and cannot include process",
            ));
        }
        Ok(Self {
            local,
            remote,
            local_kinds,
            remote_container_dispatch: false,
        })
    }

    /// Enable only when the caller has bound an admitted remote container
    /// backend. Newly installed manifest nodes then require no kind whitelist
    /// or mutable worker registry: their compiled scripts/files travel through
    /// the container transport, retaining the loaded plugin identity.
    pub fn with_remote_container_dispatch(mut self) -> Self {
        self.remote_container_dispatch = true;
        self
    }
}

#[async_trait::async_trait]
impl TaskExecutor for PlacementTaskExecutor {
    fn requires_cancel_join(&self) -> bool {
        self.local.requires_cancel_join()
            || self
                .remote
                .as_ref()
                .is_some_and(|executor| executor.requires_cancel_join())
    }
    fn name(&self) -> &'static str {
        "placement"
    }

    async fn run(&self, execution: TaskExecution) -> JobResult {
        if execution.engine_ctx.runtime_output_projection.is_some() {
            return rejected(
                execution,
                failure("formal tasks cannot use the ordinary cluster placement executor"),
            );
        }
        if self.local_kinds.contains(&execution.submission.spec.kind)
            || (self.remote_container_dispatch && execution.node.delegates_to_container_runtime())
        {
            return self.local.run(execution).await;
        }
        match &self.remote {
            Some(remote) => remote.run(execution).await,
            None => rejected(
                execution,
                failure("task requires remote placement but no remote worker is configured"),
            ),
        }
    }
}

/// Deterministic plugin tree identity: sorted relative UTF-8 paths and the
/// exact bytes of regular files. Symlinks and special files are rejected.
pub fn typed_worker_tree_sha256(root: &Path) -> Result<String, DagError> {
    fn visit(root: &Path, dir: &Path, hash: &mut Sha256) -> Result<(), DagError> {
        let mut paths = std::fs::read_dir(dir)
            .map_err(|e| failure(e.to_string()))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| failure(e.to_string()))?;
        paths.sort();
        for path in paths {
            let metadata = std::fs::symlink_metadata(&path).map_err(|e| failure(e.to_string()))?;
            if metadata.file_type().is_symlink() {
                return Err(failure("plugin identity does not permit symlinks"));
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|e| failure(e.to_string()))?;
            let relative = relative
                .to_str()
                .ok_or_else(|| failure("plugin path must be UTF-8"))?;
            hash.update(if metadata.is_dir() { b"D" } else { b"F" });
            hash.update((relative.len() as u64).to_le_bytes());
            hash.update(relative.as_bytes());
            if metadata.is_dir() {
                visit(root, &path, hash)?;
            } else if metadata.is_file() {
                use std::io::Read;
                hash.update(metadata.len().to_le_bytes());
                let mut file = std::fs::File::open(&path).map_err(|e| failure(e.to_string()))?;
                let mut buffer = [0u8; 65536];
                loop {
                    let count = file.read(&mut buffer).map_err(|e| failure(e.to_string()))?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&buffer[..count]);
                }
            } else {
                return Err(failure(
                    "plugin tree must contain only regular files and directories",
                ));
            }
        }
        Ok(())
    }
    if std::fs::symlink_metadata(root)
        .map_err(|e| failure(e.to_string()))?
        .file_type()
        .is_symlink()
    {
        return Err(failure("plugin root cannot be a symlink"));
    }
    let mut hash = Sha256::new();
    visit(root, root, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

/// Decode and cross-check a process envelope before building any registry.
pub fn typed_worker_envelope(dispatch: &TaskDispatch) -> Result<TypedWorkerEnvelope, DagError> {
    if dispatch.spec.spec.get("private_native_context").is_some()
        || dispatch.spec.spec.get("private_typed_dispatch").is_some()
    {
        return Err(failure(
            "private delegation requires the compute-only worker entry",
        ));
    }
    decode_worker_envelope(dispatch, false)
}

pub(super) fn decode_worker_envelope(
    dispatch: &TaskDispatch,
    private: bool,
) -> Result<TypedWorkerEnvelope, DagError> {
    let envelope: TypedWorkerEnvelope = serde_json::from_value(
        dispatch
            .spec
            .spec
            .get(if private {
                "private_typed_dispatch"
            } else {
                "typed_dispatch"
            })
            .ok_or_else(|| failure("missing typed worker envelope"))?
            .clone(),
    )
    .map_err(|e| failure(format!("invalid typed worker envelope: {e}")))?;
    envelope.worker.validate()?;
    envelope.vfs_principal()?;
    if envelope.protocol != 1
        || dispatch.spec.kind != "process"
        || envelope.task.kind == "process"
        || envelope.task.id != dispatch.spec.id
        || envelope.worker.expected_source_revision != crate::source_revision()
    {
        return Err(failure(
            "typed worker protocol, task identity, or source revision mismatch",
        ));
    }
    // Verify the executable actually running, as well as the launch-time pin.
    use std::io::Read;
    let mut binary =
        std::fs::File::open(std::env::current_exe().map_err(|e| failure(e.to_string()))?)
            .map_err(|e| failure(e.to_string()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = binary
            .read(&mut buffer)
            .map_err(|e| failure(e.to_string()))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(&envelope.worker.executable_sha256) {
        return Err(failure("running typed worker executable SHA256 mismatch"));
    }
    if let (Some(root), Some(expected)) = (
        &envelope.worker.plugins_root,
        &envelope.worker.plugins_sha256,
    ) {
        if !typed_worker_tree_sha256(root)?.eq_ignore_ascii_case(expected) {
            return Err(failure("typed worker plugin tree SHA256 mismatch"));
        }
    }
    Ok(envelope)
}

/// Rebuild one typed node from a pinned registry and load only named staged
/// values. This function never reads a research contract or opens a database.
pub async fn execute_typed_worker(
    envelope: &TypedWorkerEnvelope,
    registry: &NodeRegistry,
    input_paths: BTreeMap<String, Vec<PathBuf>>,
    workspace: &Path,
) -> Result<(), DagError> {
    execute_typed_worker_core(envelope, registry, input_paths, workspace, false)
        .await
        .map(|_| ())
}

pub(super) async fn execute_typed_worker_core(
    envelope: &TypedWorkerEnvelope,
    registry: &NodeRegistry,
    input_paths: BTreeMap<String, Vec<PathBuf>>,
    workspace: &Path,
    private_projection: bool,
) -> Result<crate::dag::PortOutputs, DagError> {
    let mut rebound_task = envelope.task.clone();
    for (pointer, name) in &envelope.spec_files {
        let paths = input_paths
            .get(name)
            .filter(|paths| paths.len() == 1)
            .ok_or_else(|| failure("spec file requires exactly one verified transport snapshot"))?;
        let path = &paths[0];
        if !path.is_absolute()
            || !path.starts_with(workspace.join("typed-inputs"))
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
            || path
                .canonicalize()
                .map_err(|error| failure(format!("cannot resolve spec file snapshot: {error}")))?
                != *path
        {
            return Err(failure(
                "spec file snapshot must be inside this worker's typed-inputs directory",
            ));
        }
        let value = rebound_task
            .spec
            .pointer_mut(pointer)
            .filter(|value| value.is_string())
            .ok_or_else(|| failure("spec file pointer does not name an input path"))?;
        *value = serde_json::Value::String(format!(
            "file://{}",
            path.to_str()
                .ok_or_else(|| failure("spec file path must be UTF-8"))?
        ));
    }
    for (pointer, port) in &envelope.output_directories {
        let value = rebound_task
            .spec
            .pointer_mut(pointer)
            .filter(|value| value.as_str().is_some_and(vfs::uri::is_uri))
            .ok_or_else(|| failure("directory publication requires an original VFS target"))?;
        *value = serde_json::json!(format!(
            "file://{}",
            super::typed_publication::worker_root(workspace, *port).display()
        ));
    }
    let task = &rebound_task;
    let ctx = registry.ctx();
    let principal = envelope.vfs_principal()?;
    if envelope.worker.worker_vfs_manifest.is_some()
        && ctx.opendal.as_ref().map(|storage| storage.principal()) != principal.as_ref()
    {
        return Err(failure(
            "worker registry storage does not carry the original VFS principal",
        ));
    }
    if ctx.runtime_output_projection.is_some() && !private_projection {
        return Err(failure(
            "ordinary worker cannot execute formal runtime projections",
        ));
    }
    // Scheduler-native channel nodes do not have a registry factory. Rebuild
    // their actual declarative operator, just as manifest restoration does;
    // every plugin/bundle node still goes through the strict registry path.
    let mut node: Box<dyn crate::DagNode> = match task.kind.as_str() {
        "channel" | "core/channel" => Box::new(crate::ChannelNode::from_spec(&task.spec)?),
        "logical_gather" => Box::new(crate::dag::GatherNode::default()),
        _ => registry
            .build_node_strict(&task.kind, task.spec.clone())
            .map_err(|e| {
                failure(format!(
                    "remote worker cannot build kind `{}`: {e}",
                    task.kind
                ))
            })?,
    };
    if private_projection
        && (node.delegates_to_container_runtime() || node.plugin_identity().is_some())
    {
        return Err(failure(
            "delegated native computation cannot invoke a container or plugin",
        ));
    }
    if node.plugin_identity() != envelope.plugin_identity.as_ref() {
        return Err(failure(
            "remote node plugin implementation identity differs from coordinator",
        ));
    }
    if let Some(reason) = node.remote_execution_blocker() {
        return Err(failure(format!("remote_service_unavailable: {reason}")));
    }
    let declared = node.ports().clone();
    let expected = declared
        .output_ports()
        .iter()
        .map(|port| (port.index, port.data_type.to_string()))
        .collect::<BTreeMap<_, _>>();
    let requested = task
        .outputs
        .iter()
        .map(|port| (port.port, port.payload.clone()))
        .collect::<BTreeMap<_, _>>();
    // Gather's native Any output has one precise runtime contract, inferred
    // from homogeneous staged inputs. No other Any port is made portable.
    let expected = if task.kind == "logical_gather" {
        let payload =
            gather_output_payload(task.inputs.iter().map(|input| input.payload.as_str()))?;
        [(0, payload.to_string())].into_iter().collect()
    } else {
        if expected.values().any(|payload| payload == "any") {
            return Err(failure("remote typed output requires a concrete contract"));
        }
        expected
    };
    if expected != requested {
        return Err(failure(
            "remote node output port contract differs from coordinator",
        ));
    }
    let mut inputs = Vec::new();
    let mut seen = BTreeSet::new();
    for binding in &task.inputs {
        if !seen.insert(binding.port) {
            return Err(failure("duplicate typed worker input port"));
        }
        let paths = input_paths
            .get(&binding.name)
            .ok_or_else(|| failure(format!("missing staged input `{}`", binding.name)))?;
        let artifacts = paths
            .iter()
            .enumerate()
            .map(|(index, path)| {
                let mut artifact = FileRef::local(path, None)?;
                let source_index = if binding.payload == "structured_channel" {
                    index.checked_sub(1)
                } else {
                    Some(index)
                };
                if let Some(source) = source_index.and_then(|index| {
                    envelope
                        .input_files
                        .get(&binding.port)
                        .and_then(|files| files.get(index))
                }) {
                    artifact.format = source.format.clone();
                    artifact.fingerprint = source.fingerprint.clone();
                }
                Ok::<_, DagError>(artifact)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let data =
            super::remote::RemoteTaskExecutor::load_output(&artifacts, &binding.payload, ctx)
                .await?;
        inputs.push(NodeInput {
            port: binding.port,
            data,
        });
    }
    let declared_pointers = node
        .file_input_spec_pointers(&inputs)
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if declared_pointers != envelope.spec_files.keys().cloned().collect() {
        return Err(failure(
            "spec file transport fields differ from the loaded node's file input contract",
        ));
    }
    let declared_directories = node
        .file_output_directory_spec_pointers()
        .into_iter()
        .map(|(pointer, port)| (pointer.to_owned(), port))
        .collect::<BTreeMap<_, _>>();
    let mut publication_ports = BTreeSet::new();
    for (pointer, port) in &envelope.output_directories {
        if declared_directories.get(pointer) != Some(port)
            || !publication_ports.insert(*port)
            || !task
                .outputs
                .iter()
                .any(|out| out.port == *port && out.payload == "file_set")
        {
            return Err(failure(
                "directory publication differs from the loaded node's output contract",
            ));
        }
    }
    let mut expected_names = task
        .inputs
        .iter()
        .map(|binding| binding.name.as_str())
        .collect::<BTreeSet<_>>();
    for name in envelope.spec_files.values() {
        if !expected_names.insert(name.as_str()) {
            return Err(failure("duplicate typed worker transport input name"));
        }
    }
    if input_paths
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != expected_names
    {
        return Err(failure("unexpected staged worker input bindings"));
    }
    let reporter = NodeReporter::noop();
    let outputs = node.execute(ctx, &inputs, &reporter).await?;
    if outputs
        .iter()
        .any(|(port, _)| !requested.contains_key(port))
    {
        return Err(failure("typed worker returned an undeclared output port"));
    }
    let output_root = workspace.join("typed-outputs");
    // An attempt must never accidentally collect outputs left by another run.
    std::fs::create_dir(&output_root)
        .map_err(|e| failure(format!("cannot create fresh typed output directory: {e}")))?;
    for contract in &task.outputs {
        let Some(value) = outputs.get(&contract.port) else {
            if declared
                .output_port(contract.port)
                .is_some_and(|port| !port.required)
            {
                continue;
            }
            return Err(failure("typed worker omitted a required output port"));
        };
        if value.data_type().to_string() != contract.payload {
            return Err(failure("typed worker produced an unexpected output type"));
        }
        let root = output_root.join(format!("port-{}", contract.port));
        std::fs::create_dir(&root).map_err(|e| failure(e.to_string()))?;
        match value {
            NodeValue::DataFrame(frame) => {
                let file = std::fs::File::create(root.join("value.arrow"))
                    .map_err(|e| failure(e.to_string()))?;
                let mut writer = FileWriter::try_new(file, frame.schema().as_ref())
                    .map_err(|e| failure(e.to_string()))?;
                // This is transport data, never truncated evidence.
                for batch in frame
                    .clone()
                    .collect()
                    .await
                    .map_err(|e| failure(e.to_string()))?
                {
                    writer.write(&batch).map_err(|e| failure(e.to_string()))?;
                }
                writer.finish().map_err(|e| failure(e.to_string()))?;
            }
            NodeValue::Channel(channel) => {
                let bytes =
                    serde_json::to_vec(&channel.items).map_err(|e| failure(e.to_string()))?;
                std::fs::write(root.join("value.json"), bytes)
                    .map_err(|e| failure(e.to_string()))?;
            }
            NodeValue::File(file) => {
                copy_worker_file(file, &root.join(worker_output_name(file, 0)?), ctx).await?
            }
            NodeValue::FileSet(files) => {
                if files.is_empty() {
                    return Err(failure(
                        "ordinary process transport does not support empty file-set outputs",
                    ));
                }
                for (index, file) in files.iter().enumerate() {
                    copy_worker_file(file, &root.join(worker_output_name(file, index)?), ctx)
                        .await?;
                }
                if publication_ports.contains(&contract.port) {
                    super::typed_publication::write_manifest(
                        &root,
                        workspace,
                        contract.port,
                        files,
                    )?;
                }
            }
            NodeValue::Structured(value) => {
                value.validate(64, 100_000).map_err(failure)?;
                let manifest = serde_json::to_vec(value).map_err(|e| failure(e.to_string()))?;
                if manifest.len() > 16 * 1024 * 1024 {
                    return Err(failure("structured channel manifest exceeds byte limit"));
                }
                // Process transport sorts output paths. Reserve index zero for the
                // manifest so the coordinator can reconstruct artifact order.
                std::fs::write(root.join("00000000.json"), manifest)
                    .map_err(|e| failure(e.to_string()))?;
                for (index, file) in value.artifacts().into_iter().enumerate() {
                    copy_worker_file(file, &root.join(worker_output_name(file, index + 1)?), ctx)
                        .await?;
                }
            }
        }
    }
    Ok(outputs)
}

pub(super) fn worker_output_name(file: &FileRef, index: usize) -> Result<String, DagError> {
    let format = file.format.as_deref().or_else(|| {
        Path::new(&file.path)
            .extension()
            .and_then(|ext| ext.to_str())
    });
    match format {
        Some(format)
            if !format.is_empty()
                && format
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) =>
        {
            Ok(format!("{index:08}.{}", format.to_ascii_lowercase()))
        }
        Some(_) => Err(failure(
            "typed worker output format cannot be represented safely by the process artifact protocol",
        )),
        None => Ok(format!("{index:08}")),
    }
}

async fn copy_worker_file(
    file: &FileRef,
    destination: &Path,
    ctx: &crate::registry::NodeCtx,
) -> Result<(), DagError> {
    if let Some(path) = vfs::uri::virtual_path(&file.path) {
        let storage = ctx
            .opendal
            .as_ref()
            .ok_or_else(|| failure("worker output requires its configured object store"))?;
        let length = storage
            .content_length(path)
            .await
            .map_err(|e| failure(e.to_string()))?;
        let bytes = storage
            .read_range(path, 0..length)
            .await
            .map_err(|e| failure(e.to_string()))?;
        std::fs::write(destination, bytes.to_vec()).map_err(|e| failure(e.to_string()))?;
    } else {
        std::fs::copy(
            file.path.strip_prefix("file://").unwrap_or(&file.path),
            destination,
        )
        .map_err(|e| failure(format!("cannot publish typed worker output: {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "typed_worker_tests.rs"]
mod tests;
