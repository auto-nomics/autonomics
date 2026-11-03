//! Compute-only delegation from an admitted private producer to its pinned
//! Slurm worker. These records grant no Store, signing, G0 or reveal authority.
//! Ordinary typed-worker entry points continue to reject this protocol.
use super::typed_worker::{
    TypedWorkerEnvelope, TypedWorkerSettings, decode_worker_envelope, execute_typed_worker_core,
    lower_worker_task, worker_output_name,
};
use super::{TaskDispatch, TaskExecution, TaskResources};
use crate::dag::DagError;
use crate::dag::graph::PortOutputs;
use crate::dag::runtime::NodeRunDetails;
use crate::registry::NodeRegistry;
use crate::runtime_output::RuntimeOutputProjection;
use crate::value::{FileRef, NodeValue};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

const SCOPE: &str = "PRIVATE_NATIVE_COMPUTE_V3";
const MAX_MANIFEST: u64 = 4 * 1024 * 1024;
const MAX_FILES: usize = 8192;
fn refused(message: impl std::fmt::Display) -> DagError {
    DagError::Schedule(format!("private native delegation: {message}"))
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn absolute(path: &Path) -> bool {
    path.is_absolute() && clean(path) && path.components().count() > 1
}
fn clean(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
        && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
        && path
            .to_str()
            .is_some_and(|s| !s.chars().any(char::is_control))
}
fn relative(path: &Path) -> bool {
    !path.is_absolute() && clean(path)
}
fn spec_hash(spec: &serde_json::Value) -> Result<String, DagError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(spec).map_err(refused)?)
    ))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeComputeBudget {
    pub resources: TaskResources,
    pub memory_reserve_bytes: u64,
    pub disk_reserve_fraction: f64,
    pub peak_disk_bytes: u64,
}
impl NativeComputeBudget {
    pub fn validate(&self) -> Result<(), DagError> {
        self.resources.validate()?;
        if self.resources.cpus.is_none()
            || self.resources.memory_bytes.is_none()
            || self.resources.max_duration_ms.is_none()
            || self
                .resources
                .memory_bytes
                .unwrap()
                .checked_add(self.memory_reserve_bytes)
                .is_none()
            || self.peak_disk_bytes == 0
            || !self.disk_reserve_fraction.is_finite()
            || !(0.0..1.0).contains(&self.disk_reserve_fraction)
        {
            return Err(refused("complete finite compute budget required"));
        }
        Ok(())
    }
}

/// Constructed by the private coordinator from the actual running contract,
/// executable pin and initialized output projection. It is a compute request,
/// never an independently transferable scientific authorization certificate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateNativeBinding {
    pub scope: String,
    pub contract_sha256: String,
    pub module_id: String,
    /// Formal Store BEGIN identity, independent of the DAG history UUID.
    pub run_id: String,
    /// Actual recorded DAG invocation; owns the runtime output projection.
    pub engine_run_id: String,
    pub task_id: String,
    pub kind: String,
    pub spec_sha256: String,
    pub remote_user: String,
    pub engine_sha256: String,
    pub compute_scratch_root: PathBuf,
    pub projection_identity_blake3: Option<String>,
    pub projected_directories: BTreeMap<PathBuf, PathBuf>,
    pub sink: Option<PathBuf>,
    pub metadata_port: u8,
    pub budget: NativeComputeBudget,
}
impl PrivateNativeBinding {
    /// Append publication metadata after the scientific ports. The process
    /// transport requires a dense, ordered output list; an unused high port
    /// is not a valid substitute and must never renumber scientific outputs.
    pub fn metadata_port_for_task(task: &super::TaskSpec) -> Result<u8, DagError> {
        let next =
            u8::try_from(task.outputs.len()).map_err(|_| refused("no metadata output port"))?;
        if task
            .outputs
            .iter()
            .enumerate()
            .any(|(index, output)| usize::from(output.port) != index)
        {
            return Err(refused(
                "scientific outputs must use ordered contiguous ports",
            ));
        }
        Ok(next)
    }

    pub fn scope() -> &'static str {
        SCOPE
    }
    pub fn specification_sha256(spec: &serde_json::Value) -> Result<String, DagError> {
        spec_hash(spec)
    }
    pub fn validate(&self) -> Result<(), DagError> {
        self.budget.validate()?;
        let uuid = |value: &str| {
            value.len() == 36
                && value.bytes().enumerate().all(|(i, b)| {
                    if [8, 13, 18, 23].contains(&i) {
                        b == b'-'
                    } else {
                        b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                    }
                })
        };
        if self.scope != SCOPE
            || !absolute(&self.compute_scratch_root)
            || !uuid(&self.run_id)
            || !uuid(&self.engine_run_id)
            || !digest(&self.contract_sha256)
            || !digest(&self.engine_sha256)
            || !digest(&self.spec_sha256)
            || [&self.module_id, &self.task_id, &self.kind]
                .iter()
                .any(|s| s.is_empty() || s.len() > 1024 || s.chars().any(char::is_control))
            || self.kind == "process"
            || self.remote_user.is_empty()
            || self.remote_user.len() > 128
            || !self
                .remote_user
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || self.projected_directories.len() > 4096
            || self
                .projected_directories
                .iter()
                .any(|(a, b)| !absolute(a) || !absolute(b))
            || self.sink.as_deref().is_some_and(|p| !absolute(p))
            || self
                .projection_identity_blake3
                .as_deref()
                .is_some_and(|v| !digest(v))
            || (!self.projected_directories.is_empty() && self.projection_identity_blake3.is_none())
        {
            return Err(refused("invalid compute binding"));
        }
        let logical: Vec<_> = self.projected_directories.keys().collect();
        let physical: Vec<_> = self.projected_directories.values().collect();
        for list in [logical, physical] {
            for (i, p) in list.iter().enumerate() {
                if list
                    .iter()
                    .skip(i + 1)
                    .any(|q| p.starts_with(q) || q.starts_with(p))
                {
                    return Err(refused("overlapping publisher declarations"));
                }
            }
        }
        if let Some(sink) = &self.sink {
            if self
                .projected_directories
                .values()
                .any(|p| sink.starts_with(p) || p.starts_with(sink))
            {
                return Err(refused("scalar sink aliases directory publisher"));
            }
        }
        Ok(())
    }
    fn check_task(&self, task: &super::TaskSpec) -> Result<(), DagError> {
        self.validate()?;
        if self.task_id != task.id
            || self.kind != task.kind
            || self.spec_sha256 != spec_hash(&task.spec)?
            || self.metadata_port != Self::metadata_port_for_task(task)?
        {
            return Err(refused("task/specification/publication port differs"));
        }
        if let Some(sink) = &self.sink {
            if !matches!(
                self.kind.as_str(),
                "dataframe_to_file" | "core/dataframe_to_file"
            ) || task
                .spec
                .get("path")
                .and_then(|v| v.as_str())
                .map(Path::new)
                != Some(sink.as_path())
            {
                return Err(refused(
                    "scalar sink requires the exact compiled dataframe_to_file path",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn validate_for_task(
        &self,
        e: &TaskExecution,
        settings: &TypedWorkerSettings,
    ) -> Result<(), DagError> {
        self.check_task(&e.submission.spec)?;
        self.check_settings(settings)?;
        if e.submission.resources != self.budget.resources
            || e.node.delegates_to_container_runtime()
            || e.node.plugin_identity().is_some()
            || e.node
                .runtime_output_directories()
                .into_iter()
                .collect::<BTreeSet<_>>()
                != self.projected_directories.keys().cloned().collect()
            || e.node.sink_path().map(PathBuf::from) != self.sink
        {
            return Err(refused(
                "loaded node, publishers or resources differ from binding",
            ));
        }
        self.verify_projection(e.engine_ctx.runtime_output_projection.as_deref())
    }
    fn check_settings(&self, settings: &TypedWorkerSettings) -> Result<(), DagError> {
        if settings.executable_sha256 != self.engine_sha256
            || settings.plugins_root.is_some()
            || settings.plugins_sha256.is_some()
            || settings.apptainer_catalog.is_some()
            || settings.apptainer_executable.is_some()
            || settings.podman.is_some()
            || settings.native_environment.is_some()
            || settings.worker_vfs_manifest.is_some()
            || settings.worker_vfs_sha256.is_some()
        {
            return Err(refused(
                "native computation requires the pinned engine without ambient plugins/runtime/VFS",
            ));
        }
        Ok(())
    }
    pub fn verify_projection(
        &self,
        projection: Option<&RuntimeOutputProjection>,
    ) -> Result<(), DagError> {
        self.validate()?;
        match (projection, self.projection_identity_blake3.as_deref()) {
            (Some(p), Some(identity)) => {
                p.verify_runtime_directory()?;
                if p.run_id() != self.engine_run_id || p.identity_blake3()? != identity {
                    return Err(refused("producer projection/run identity changed"));
                }
                for (logical, actual) in &self.projected_directories {
                    if p.resolve(logical)? != *actual {
                        return Err(refused("producer destination changed"));
                    }
                }
                Ok(())
            }
            (None, None) if self.projected_directories.is_empty() => Ok(()),
            _ => Err(refused("producer projection presence changed")),
        }
    }
}

pub fn lower_private_native_task(
    e: &mut TaskExecution,
    settings: &TypedWorkerSettings,
    binding: &PrivateNativeBinding,
) -> Result<(), DagError> {
    lower_worker_task(e, settings, Some(binding))
}

pub fn private_native_worker_envelope(
    dispatch: &TaskDispatch,
) -> Result<(TypedWorkerEnvelope, PrivateNativeBinding), DagError> {
    if dispatch.spec.spec.get("typed_dispatch").is_some() {
        return Err(refused("mixed ordinary/private envelope"));
    }
    let envelope = decode_worker_envelope(dispatch, true)?;
    let binding: PrivateNativeBinding = serde_json::from_value(
        dispatch
            .spec
            .spec
            .get("private_native_context")
            .ok_or_else(|| refused("missing producer binding"))?
            .clone(),
    )
    .map_err(refused)?;
    binding.check_task(&envelope.task)?;
    binding.check_settings(&envelope.worker)?;
    if dispatch.resources != binding.budget.resources
        || !envelope.output_directories.is_empty()
        || envelope.plugin_identity.is_some()
        || dispatch.spec.outputs.len() != envelope.task.outputs.len() + 1
        || !dispatch.spec.outputs.iter().any(|p| {
            p.port == binding.metadata_port
                && p.payload == "file"
                && p.name == "private-native-publication"
        })
    {
        return Err(refused(
            "private dispatch resource or output contract differs",
        ));
    }
    for expected in &envelope.task.outputs {
        let actual: Vec<_> = dispatch
            .spec
            .outputs
            .iter()
            .filter(|p| p.port == expected.port)
            .collect();
        if actual.len() != 1
            || actual[0].name != expected.name
            || actual[0].payload != expected.payload
        {
            return Err(refused("private dispatch replaced a scientific output"));
        }
    }
    Ok((envelope, binding))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishedFile {
    port: u8,
    index: usize,
    logical_directory: Option<PathBuf>,
    relative_path: PathBuf,
    bytes: u64,
    sha256: String,
    format: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationManifest {
    binding: PrivateNativeBinding,
    worker_uid: u32,
    worker_projection: Option<RuntimeOutputProjection>,
    scratch_directory: PathBuf,
    scratch_removed: bool,
    files: Vec<PublishedFile>,
}
fn real_file(path: &Path) -> Result<(), DagError> {
    if !absolute(path) || path.canonicalize().map_err(refused)? != path {
        return Err(refused("file path is not canonical"));
    }
    let s = fs::symlink_metadata(path).map_err(refused)?;
    if !s.is_file() || s.file_type().is_symlink() {
        return Err(refused("regular non-aliased file required"));
    }
    for parent in path.ancestors().skip(1) {
        if !fs::symlink_metadata(parent).map_err(refused)?.is_dir()
            || fs::symlink_metadata(parent)
                .map_err(refused)?
                .file_type()
                .is_symlink()
        {
            return Err(refused("file ancestry contains an alias"));
        }
    }
    Ok(())
}
fn hash_file(path: &Path) -> Result<(u64, String), DagError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    real_file(path)?;
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(refused)?;
    let before = f.metadata().map_err(refused)?;
    let stamp = |s: &fs::Metadata| {
        (
            s.dev(),
            s.ino(),
            s.mode(),
            s.nlink(),
            s.size(),
            s.mtime(),
            s.mtime_nsec(),
            s.ctime(),
            s.ctime_nsec(),
        )
    };
    let mut h = Sha256::new();
    let mut bytes = 0;
    let mut buf = [0; 65536];
    loop {
        let n = f.read(&mut buf).map_err(refused)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        bytes += n as u64;
    }
    if bytes != before.len()
        || stamp(&before) != stamp(&f.metadata().map_err(refused)?)
        || stamp(&before) != stamp(&fs::symlink_metadata(path).map_err(refused)?)
    {
        return Err(refused("file changed while hashing"));
    }
    Ok((bytes, format!("{:x}", h.finalize())))
}
fn local_file(file: &FileRef) -> Result<PathBuf, DagError> {
    let p = PathBuf::from(file.path.strip_prefix("file://").unwrap_or(&file.path));
    real_file(&p)?;
    Ok(p)
}
fn private_dir(path: &Path) -> Result<(), DagError> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(refused)
}
fn files_by_port(outputs: &PortOutputs) -> Result<BTreeMap<(u8, usize), FileRef>, DagError> {
    let mut files = BTreeMap::new();
    for (port, value) in outputs.iter() {
        match value {
            NodeValue::File(f) => {
                files.insert((*port, 0), f.clone());
            }
            NodeValue::FileSet(list) => {
                for (i, f) in list.iter().enumerate() {
                    files.insert((*port, i), f.clone());
                }
            }
            _ => (),
        }
    }
    if files.len() > MAX_FILES {
        return Err(refused("publication file count exceeded"));
    }
    Ok(files)
}

/// Probe only fresh, self-owned empty directories. Never downgrade to a
/// racy existence check plus rename when the filesystem lacks NOREPLACE.
fn prove_atomic_directory_publication(root: &Path) -> Result<(), DagError> {
    use std::os::unix::ffi::OsStrExt;
    let rename = |source: &Path, target: &Path| -> std::io::Result<()> {
        let a =
            std::ffi::CString::new(source.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
        let b =
            std::ffi::CString::new(target.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                a.as_ptr(),
                libc::AT_FDCWD,
                b.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    };
    let source = root.join(".atomic-source");
    let target = root.join(".atomic-target");
    private_dir(&source)?;
    rename(&source, &target).map_err(|e| {
        refused(format!(
            "compute scratch lacks atomic no-clobber directory publication: {e}"
        ))
    })?;
    private_dir(&source)?;
    match rename(&source, &target) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        _ => {
            return Err(refused(
                "compute scratch did not preserve an existing publication target",
            ));
        }
    }
    fs::remove_dir(source).map_err(refused)?;
    fs::remove_dir(target).map_err(refused)?;
    Ok(())
}

/// Run only the compiled native computation in a fresh remote staging area.
/// The data-engine entry must first verify actual Slurm identity and cgroup.
pub async fn execute_private_native_worker(
    envelope: &TypedWorkerEnvelope,
    binding: &PrivateNativeBinding,
    registry: &NodeRegistry,
    inputs: BTreeMap<String, Vec<PathBuf>>,
    workspace: &Path,
) -> Result<(), DagError> {
    binding.check_task(&envelope.task)?;
    binding.check_settings(&envelope.worker)?;
    if !absolute(workspace) || workspace.canonicalize().map_err(refused)? != workspace {
        return Err(refused("canonical workspace required"));
    }
    let compiled = registry
        .build_node_strict(&envelope.task.kind, envelope.task.spec.clone())
        .map_err(refused)?;
    if compiled.delegates_to_container_runtime()
        || compiled.plugin_identity().is_some()
        || compiled
            .runtime_output_directories()
            .into_iter()
            .collect::<BTreeSet<_>>()
            != binding.projected_directories.keys().cloned().collect()
        || compiled.sink_path().map(PathBuf::from) != binding.sink
    {
        return Err(refused(
            "worker publisher declarations differ from producer",
        ));
    }
    let mut ctx = registry.ctx().clone();
    if ctx.runtime_output_projection.is_some() {
        return Err(refused("worker must start without an ambient projection"));
    }
    // Shared Lustre may reject RENAME_NOREPLACE even when the same node's
    // local XFS supports it. Scientific publishers retain their original
    // atomicity; only their physical staging is relocated to admitted scratch.
    if binding
        .compute_scratch_root
        .canonicalize()
        .map_err(refused)?
        != binding.compute_scratch_root
        || !fs::symlink_metadata(&binding.compute_scratch_root)
            .map_err(refused)?
            .is_dir()
    {
        return Err(refused("canonical existing compute scratch root required"));
    }
    let scratch = tempfile::Builder::new()
        .prefix(&format!("autonomics-native-{}-", binding.engine_run_id))
        .tempdir_in(&binding.compute_scratch_root)
        .map_err(refused)?;
    let scratch_directory = scratch.path().to_owned();
    prove_atomic_directory_publication(&scratch_directory)?;
    let projection = if binding.projected_directories.is_empty() {
        None
    } else {
        let root = scratch_directory.join("projection");
        private_dir(&root)?;
        let destinations = binding
            .projected_directories
            .keys()
            .enumerate()
            .map(|(i, logical)| {
                (
                    logical.clone(),
                    root.join(&binding.engine_run_id)
                        .join(format!("publisher-{i}")),
                )
            })
            .collect();
        let p = RuntimeOutputProjection::new(binding.engine_run_id.clone(), root, destinations)?;
        p.validate_for_run(
            &binding.engine_run_id,
            &binding.projected_directories.keys().cloned().collect(),
            &inputs.values().flatten().cloned().collect::<Vec<_>>(),
        )?;
        Some(p.initialize_run_directory()?)
    };
    ctx.runtime_output_projection = projection.clone().map(Arc::new);
    let registry = registry.with_node_ctx(ctx);
    let mut physical = envelope.clone();
    let remote_sink = if let Some(sink) = &binding.sink {
        let root = scratch_directory.join("sink");
        private_dir(&root)?;
        let target = root.join(
            sink.file_name()
                .ok_or_else(|| refused("sink filename missing"))?,
        );
        physical.task.spec["path"] = serde_json::json!(target);
        Some(target)
    } else {
        None
    };
    let outputs = execute_typed_worker_core(&physical, &registry, inputs, workspace, true).await?;
    let mut files = Vec::new();
    for ((port, index), file) in files_by_port(&outputs)? {
        let source = local_file(&file)?;
        let mut destination = None;
        if let Some(p) = &projection {
            p.verify_runtime_directory()?;
            for (logical, root) in p.destinations() {
                if let Ok(rel) = source.strip_prefix(root) {
                    if !relative(rel) || destination.is_some() {
                        return Err(refused("ambiguous publisher file"));
                    }
                    destination = Some((Some(logical.clone()), rel.to_owned()));
                }
            }
        }
        if remote_sink.as_ref() == Some(&source) {
            destination = Some((None, source.file_name().unwrap().into()));
        }
        let (logical_directory, relative_path) =
            destination.ok_or_else(|| refused("native file outside its declared publisher"))?;
        let (bytes, sha256) = hash_file(&source)?;
        let transported = workspace
            .join("typed-outputs")
            .join(format!("port-{port}"))
            .join(worker_output_name(&file, index)?);
        if hash_file(&transported)? != (bytes, sha256.clone()) {
            return Err(refused("serialized native file differs from producer"));
        }
        files.push(PublishedFile {
            port,
            index,
            logical_directory,
            relative_path,
            bytes,
            sha256,
            format: file.format,
        });
    }
    // All file bytes and their serialized copies were hashed before cleanup.
    // A failure to close our own scratch is not successful evidence.
    scratch
        .close()
        .map_err(|e| refused(format!("compute scratch cleanup failed: {e}")))?;
    match fs::symlink_metadata(&scratch_directory) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        _ => return Err(refused("compute scratch still exists after cleanup")),
    }
    let manifest = PublicationManifest {
        binding: binding.clone(),
        worker_uid: unsafe { libc::geteuid() },
        worker_projection: projection,
        scratch_directory,
        scratch_removed: true,
        files,
    };
    let bytes = serde_json::to_vec(&manifest).map_err(refused)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err(refused("publication manifest too large"));
    }
    let root = workspace
        .join("typed-outputs")
        .join(format!("port-{}", binding.metadata_port));
    private_dir(&root)?;
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("publication.json"))
        .map_err(refused)?;
    f.write_all(&bytes).map_err(refused)?;
    f.sync_all().map_err(refused)?;
    Ok(())
}

/// Validate every returned file before restoring names under the original,
/// still-initialized private projection. No overwrite or implicit retry.
pub fn restore_private_native_outputs(
    binding: &PrivateNativeBinding,
    projection: Option<&RuntimeOutputProjection>,
    outputs: PortOutputs,
    details: &mut NodeRunDetails,
) -> Result<PortOutputs, DagError> {
    binding.verify_projection(projection)?;
    let meta = outputs
        .get(&binding.metadata_port)
        .ok_or_else(|| refused("missing publication manifest"))?
        .as_file()?
        .clone();
    let path = local_file(&meta)?;
    let (size, meta_hash) = hash_file(&path)?;
    if size > MAX_MANIFEST {
        return Err(refused("publication manifest exceeds bound"));
    }
    let mut raw = Vec::new();
    fs::File::open(&path)
        .map_err(refused)?
        .take(MAX_MANIFEST + 1)
        .read_to_end(&mut raw)
        .map_err(refused)?;
    if format!("{:x}", Sha256::digest(&raw)) != meta_hash || raw.len() as u64 != size {
        return Err(refused("publication manifest changed"));
    }
    let manifest: PublicationManifest = serde_json::from_slice(&raw).map_err(refused)?;
    if manifest.binding != *binding
        || manifest.files.len() > MAX_FILES
        || !manifest.scratch_removed
        || manifest.scratch_directory.parent() != Some(binding.compute_scratch_root.as_path())
        || !absolute(&manifest.scratch_directory)
        || !manifest
            .scratch_directory
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| {
                n.starts_with(&format!("autonomics-native-{}-", binding.engine_run_id))
            })
    {
        return Err(refused("publication binding changed"));
    }
    match &manifest.worker_projection {
        Some(p) if !binding.projected_directories.is_empty() => {
            p.validate_shape()?;
            if p.staging_root() != manifest.scratch_directory.join("projection") {
                return Err(refused(
                    "remote projection escaped its admitted compute scratch",
                ));
            }
            if p.run_id() != binding.engine_run_id
                || p.destinations().keys().collect::<Vec<_>>()
                    != binding.projected_directories.keys().collect::<Vec<_>>()
            {
                return Err(refused("remote projection differs"));
            }
        }
        None if binding.projected_directories.is_empty() => (),
        _ => return Err(refused("remote publisher declarations changed")),
    }
    let mut values = PortOutputs::new();
    for (port, value) in outputs.iter() {
        if *port != binding.metadata_port {
            values.insert(*port, value.clone());
        }
    }
    let actual = files_by_port(&values)?;
    let mut planned = BTreeMap::new();
    let mut destinations = BTreeMap::<PathBuf, (u64, String)>::new();
    for file in &manifest.files {
        let source = actual
            .get(&(file.port, file.index))
            .ok_or_else(|| refused("manifest names a missing file"))?;
        let transported_name = worker_output_name(
            &FileRef::new(file.relative_path.to_string_lossy(), file.format.clone()),
            file.index,
        )?;
        let transported_format = Path::new(&transported_name)
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_owned);
        if !relative(&file.relative_path)
            || !digest(&file.sha256)
            || source.format != transported_format
            || planned.contains_key(&(file.port, file.index))
        {
            return Err(refused("invalid/repeated publication member"));
        }
        let source = local_file(source)?;
        if hash_file(&source)? != (file.bytes, file.sha256.clone()) {
            return Err(refused("returned file failed content verification"));
        }
        let destination = match &file.logical_directory {
            Some(logical) => binding
                .projected_directories
                .get(logical)
                .ok_or_else(|| refused("undeclared logical publisher"))?
                .join(&file.relative_path),
            None => {
                let sink = binding
                    .sink
                    .as_ref()
                    .ok_or_else(|| refused("undeclared scalar sink"))?;
                if file.relative_path
                    != Path::new(
                        sink.file_name()
                            .ok_or_else(|| refused("sink filename missing"))?,
                    )
                {
                    return Err(refused("scalar sink filename differs"));
                }
                sink.clone()
            }
        };
        if let Some(old) =
            destinations.insert(destination.clone(), (file.bytes, file.sha256.clone()))
        {
            if old != (file.bytes, file.sha256.clone()) {
                return Err(refused("different outputs alias one destination"));
            }
        }
        planned.insert((file.port, file.index), (source, destination, file));
    }
    if planned.len() != actual.len() {
        return Err(refused("unlisted returned file"));
    }
    // Preflight all destinations before the first publication. Partial failure
    // remains failed evidence; the formal producer cannot checkpoint it.
    for destination in destinations.keys() {
        if destination.try_exists().map_err(refused)? || fs::symlink_metadata(destination).is_ok() {
            return Err(refused("publication target already exists"));
        }
    }
    let mut restored = BTreeMap::new();
    for (key, (source, destination, file)) in &planned {
        if !restored
            .values()
            .any(|f: &FileRef| f.path == destination.to_string_lossy())
        {
            let parent = destination
                .parent()
                .ok_or_else(|| refused("missing publication parent"))?;
            let mut missing = Vec::new();
            let mut cursor = parent;
            while !cursor.try_exists().map_err(refused)? {
                missing.push(cursor.to_owned());
                cursor = cursor
                    .parent()
                    .ok_or_else(|| refused("publisher ancestor missing"))?;
            }
            if cursor.canonicalize().map_err(refused)? != cursor {
                return Err(refused("publication parent alias"));
            }
            for dir in missing.iter().rev() {
                private_dir(dir)?;
            }
            let pending = parent.join(format!(
                ".native-{}-{}-{}-{}.part",
                binding.run_id,
                &binding.spec_sha256[..16],
                key.0,
                key.1
            ));
            use std::os::unix::fs::OpenOptionsExt;
            let mut input = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(source)
                .map_err(refused)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&pending)
                .map_err(refused)?;
            std::io::copy(&mut input, &mut output).map_err(refused)?;
            output.sync_all().map_err(refused)?;
            if hash_file(&pending)? != (file.bytes, file.sha256.clone())
                || hash_file(source)? != (file.bytes, file.sha256.clone())
            {
                return Err(refused("publication copy failed identity verification"));
            }
            fs::hard_link(&pending, destination).map_err(refused)?;
            fs::remove_file(&pending).map_err(refused)?;
            fs::File::open(parent)
                .map_err(refused)?
                .sync_all()
                .map_err(refused)?;
        }
        let mut file_ref = FileRef::local(destination, file.format.clone())?;
        file_ref.fingerprint.as_mut().unwrap().content_hash = Some(file.sha256.clone());
        restored.insert(*key, file_ref);
    }
    let mut result = PortOutputs::new();
    for (port, value) in values.iter() {
        match value {
            NodeValue::File(_) => {
                result.insert_file(*port, restored[&(*port, 0)].clone());
            }
            NodeValue::FileSet(files) => {
                result.insert(
                    *port,
                    NodeValue::FileSet(
                        (0..files.len())
                            .map(|i| restored[&(*port, i)].clone())
                            .collect(),
                    ),
                );
            }
            _ => {
                result.insert(*port, value.clone());
            }
        }
    }
    for (port, files) in &mut details.output_artifacts_by_port {
        if *port != binding.metadata_port {
            for (index, file) in files.iter_mut().enumerate() {
                if let Some(new) = restored.get(&(*port, index)) {
                    *file = new.clone();
                }
            }
        }
    }
    details
        .output_artifacts_by_port
        .remove(&binding.metadata_port);
    details.output_artifacts = details
        .output_artifacts_by_port
        .values()
        .flatten()
        .cloned()
        .chain(std::iter::once(meta))
        .collect();
    binding.verify_projection(projection)?;
    Ok(result)
}

#[cfg(test)]
#[path = "private_native_tests.rs"]
mod tests;
