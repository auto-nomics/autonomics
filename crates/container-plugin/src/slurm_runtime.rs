//! Slurm/Apptainer implementation of the existing container-node connection.
//! Nodes retain their /work contract and VFS publication on the coordinator.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use container_runtime::types::PrivateEnvironment;
use container_runtime::{
    ContainerNetwork, ContainerRunRequest, ContainerRunResult, ContainerRuntimeError, GpuRequest,
    PodmanConnection, PullPolicy,
};
use dag_core::{
    ApptainerSettings, FileRef, ProcessTaskOutput, ProcessTaskSpec, SifImage, SlurmSettings,
    SlurmTaskTransport, SshArtifactStore, SshTarget, TaskArtifactStore, TaskDispatch, TaskLease,
    TaskOutputBinding, TaskResources, TaskSpec, TaskTransport,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const WORKER: &str = include_str!("slurm_container_worker.py");
type Result<T> = std::result::Result<T, ContainerRuntimeError>;

/// Operator-selected native probe, measured again on the allocated node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceProbeProgram {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    pub engine_revision: String,
}

fn invalid(message: impl Into<String>) -> ContainerRuntimeError {
    ContainerRuntimeError::Invalid(message.into())
}
fn mapped(error: impl std::fmt::Display) -> ContainerRuntimeError {
    invalid(error.to_string())
}

fn record_submission_failure(
    journal: &Path,
    state: &mut Value,
    error: &dag_core::dag::DagError,
) -> Result<()> {
    // Classify only the transport's typed protocol evidence. Diagnostic text
    // (including historical authentication errors) cannot prove NOT_SENT.
    if let dag_core::dag::DagError::SubmissionNotSent {
        local_ssh_stopped, ..
    } = error
    {
        state["state"] = json!("SUBMISSION_NOT_SENT");
        state["local_ssh_stopped"] = json!(local_ssh_stopped);
    } else {
        state["state"] = json!("SUBMISSION_UNKNOWN");
    }
    state["automatic_replay_authorized"] = json!(false);
    record(journal, state)
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn bytes_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn gpu_allocation(request: &GpuRequest, partition: Option<&str>) -> Result<Option<u32>> {
    let count = match request {
        GpuRequest::None => return Ok(None),
        // Legacy "all" means every GPU admitted to this job, with a bounded
        // default allocation of one GPU rather than every GPU on a shared node.
        GpuRequest::All => 1,
        GpuRequest::Count(count) if *count > 0 => *count,
        GpuRequest::Count(_) => return Err(invalid("GPU count must be positive")),
        GpuRequest::Devices(_) => {
            return Err(invalid(
                "host GPU device IDs cannot select scheduler-owned allocations; request a GPU count",
            ));
        }
    };
    if partition.is_none() {
        return Err(invalid("GPU jobs require an operator GPU Slurm partition"));
    }
    Ok(Some(count))
}

fn admitted_cpus(requested: Option<f64>, gpus: Option<u32>, per_gpu: u32) -> Result<Option<u32>> {
    let requested = requested.map(|n| n.ceil().min(u32::MAX as f64) as u32);
    match gpus {
        None => Ok(requested),
        Some(count) => {
            let floor = count
                .checked_mul(per_gpu)
                .filter(|n| *n > 0)
                .ok_or_else(|| invalid("GPU CPU admission overflow"))?;
            Ok(Some(requested.unwrap_or(1).max(floor)))
        }
    }
}

pub fn file_hash(path: &Path) -> Result<String> {
    file_hash_observed(
        path,
        &dag_core::dag::node_event::NodeReporter::noop(),
        "hashing_workspace",
    )
}

fn file_hash_observed(
    path: &Path,
    reporter: &dag_core::dag::node_event::NodeReporter,
    phase: &str,
) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    let total = file.metadata()?.len();
    let mut current = 0u64;
    let mut last = std::time::Instant::now();
    reporter.phase_progress(phase, 0, Some(total), "bytes");
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        current += count as u64;
        if last.elapsed() >= Duration::from_secs(1) {
            reporter.phase_progress(phase, current, Some(total), "bytes");
            last = std::time::Instant::now();
        }
    }
    reporter.phase_progress(phase, current, Some(total), "bytes");
    Ok(format!("{:x}", hash.finalize()))
}

fn relative(path: &str) -> Result<&Path> {
    let value = Path::new(path);
    if path.is_empty()
        || value.is_absolute()
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || value
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid("noncanonical container transfer path"));
    }
    Ok(value)
}

fn absolute(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| invalid("UTF-8 path required"))?;
    if !path.is_absolute()
        || path == Path::new("/")
        || value.contains([',', ':', '\\'])
        || value.chars().any(char::is_control)
        || value
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(invalid(
            "absolute normalized path without bind delimiters required",
        ));
    }
    Ok(value.into())
}

fn private_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let info = path.symlink_metadata()?;
    if !info.is_dir()
        || info.file_type().is_symlink()
        || info.uid() != unsafe { libc::getuid() }
        || info.mode() & 0o077 != 0
    {
        return Err(invalid("private operator-owned state directory required"));
    }
    Ok(())
}

fn record(path: &Path, value: &Value) -> Result<()> {
    private_dir(
        path.parent()
            .ok_or_else(|| invalid("record parent missing"))?,
    )?;
    if path.is_symlink() {
        return Err(invalid("journal symlink refused"));
    }
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(mapped)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(mapped)?;
    std::fs::File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}

fn stopped_worker_projection(proof: &Value) -> Result<Value> {
    if proof["full_subtree_terminal"] != true {
        return Err(invalid("cannot publish an unconfirmed worker stop"));
    }
    let rows = proof["accounting"]["stdout"]
        .as_str()
        .ok_or_else(|| invalid("stop accounting evidence missing"))?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields = line.split('|').collect::<Vec<_>>();
            if fields.len() != 4 {
                return Err(invalid("invalid stop accounting record"));
            }
            Ok(json!({"job_id":fields[0],"state":fields[1],"exit_code":fields[2],"end":fields[3]}))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut control = serde_json::Map::new();
    for token in proof["control"]["stdout"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
    {
        if let Some((key, value)) = token.split_once('=') {
            if matches!(key, "JobId" | "JobState" | "ExitCode" | "EndTime") {
                control.insert(key.into(), value.into());
            }
        }
    }
    Ok(
        json!({"schema":"SLURM_WORKER_STOP_V1","job_id":proof["job_id"],
        "full_subtree_terminal":true,"queue_absent":true,"accounting":rows,"control":control,
        "raw_proof_sha256":bytes_hash(&serde_json::to_vec_pretty(proof).map_err(mapped)?)}),
    )
}

fn report_stopped_worker(
    reporter: &dag_core::dag::node_event::NodeReporter,
    root: &Path,
    proof: &Value,
) -> Result<()> {
    let projection = stopped_worker_projection(proof)?;
    record(&root.join("STOP_PROOF.json"), proof)?;
    reporter.execution(dag_core::dag::live::ExecutionObservation {
        phase: "checking_worker_stop".into(),
        worker: Some(
            json!({"evidence":"verified_stop","backend":"slurm-apptainer",
            "id":proof["job_id"],"proof":projection}),
        ),
        ..Default::default()
    });
    Ok(())
}

fn workspace_files(request: &ContainerRunRequest) -> Result<(Vec<String>, Vec<(Value, PathBuf)>)> {
    workspace_files_observed(request, &dag_core::dag::node_event::NodeReporter::noop())
}

fn workspace_files_observed(
    request: &ContainerRunRequest,
    reporter: &dag_core::dag::node_event::NodeReporter,
) -> Result<(Vec<String>, Vec<(Value, PathBuf)>)> {
    let root = request.workspace.host_path.canonicalize()?;
    let allowed = request
        .input_mounts
        .iter()
        .map(|mount| mount.host_dir.canonicalize())
        .collect::<std::io::Result<Vec<_>>>()?;
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut todo = vec![root.clone()];
    while let Some(dir) = todo.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let rel = path
                .strip_prefix(&root)
                .map_err(mapped)?
                .to_str()
                .ok_or_else(|| invalid("UTF-8 workspace path required"))?
                .to_string();
            relative(&rel)?;
            if rel == container_runtime::gc::SCRATCH_LOCK_FILE || rel == ".autonomics/receipts" {
                continue;
            }
            let info = path.symlink_metadata()?;
            if info.is_dir() {
                dirs.push(rel);
                todo.push(path);
                continue;
            }
            let readonly = info.file_type().is_symlink();
            let source = if readonly {
                let resolved = path.canonicalize()?;
                if !resolved.starts_with(&root) && !allowed.iter().any(|p| resolved.starts_with(p))
                {
                    return Err(invalid(
                        "staged symlink escaped its authorized input mounts",
                    ));
                }
                resolved
            } else {
                path
            };
            let source_info = source.symlink_metadata()?;
            if !source_info.is_file() || source_info.file_type().is_symlink() {
                return Err(invalid(
                    "workspace may contain only regular files and admitted input links",
                ));
            }
            let sha = file_hash_observed(&source, reporter, &format!("hashing_workspace:{rel}"))?;
            files.push((
                json!({"path": rel, "size": source_info.len(), "sha256": sha,
                "mode": source_info.mode() & 0o777, "readonly": readonly}),
                source,
            ));
        }
    }
    dirs.sort();
    files.sort_by(|a, b| a.0["path"].as_str().cmp(&b.0["path"].as_str()));
    Ok((dirs, files))
}

/// Every ordinary container node uses this connection when Slurm and the SIF
/// manifest are enabled. There is no fallback to local Podman after submission.
pub struct SlurmContainerRuntime {
    settings: SlurmSettings,
    data_ssh: Option<SshTarget>,
    images: ApptainerSettings,
    workspace: PathBuf,
    journal: PathBuf,
    remote_panels: String,
    python: String,
    gpu_partition: Option<String>,
    gpu_cpus_per_gpu: u32,
    gpu_qos: Option<String>,
    resource_probe: Option<(ResourceProbeProgram, Value)>,
    submission_recorder: Option<Arc<dyn dag_core::SlurmSubmissionRecorder>>,
}

impl SlurmContainerRuntime {
    pub fn new(
        settings: SlurmSettings,
        workspace: PathBuf,
        journal: PathBuf,
        remote_panels: String,
        python: String,
    ) -> Result<Self> {
        let images = settings
            .apptainer
            .clone()
            .ok_or_else(|| invalid("operator SIF manifest is required"))?;
        images.validate().map_err(mapped)?;
        private_dir(&journal)?;
        if python.is_empty() || python.chars().any(char::is_control) {
            return Err(invalid("explicit remote Python executable required"));
        }
        if !remote_panels.starts_with('/') && !remote_panels.starts_with("~/") {
            return Err(invalid(
                "remote panel cache must be absolute or relative to remote home",
            ));
        }
        let sources = journal.join("sources");
        private_dir(&sources)?;
        let capsule = sources.join(format!("{}.py", bytes_hash(WORKER.as_bytes())));
        if capsule.exists() {
            if file_hash(&capsule)? != bytes_hash(WORKER.as_bytes()) {
                return Err(invalid("worker capsule identity differs"));
            }
        } else {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(capsule)?;
            file.write_all(WORKER.as_bytes())?;
            file.sync_all()?;
        }
        Ok(Self {
            settings,
            data_ssh: None,
            images,
            workspace,
            journal,
            remote_panels,
            python,
            gpu_partition: None,
            gpu_cpus_per_gpu: 16,
            gpu_qos: None,
            resource_probe: None,
            submission_recorder: None,
        })
    }

    /// Operator-approved data endpoint for bulk payloads. Submission, polling,
    /// cancellation and stop proofs remain on the control endpoint.
    pub fn with_data_ssh(mut self, target: SshTarget) -> Self {
        self.data_ssh = Some(target);
        self
    }

    fn artifact_store(&self, local_root: PathBuf) -> SshArtifactStore {
        SshArtifactStore::new(
            self.transfer_ssh().clone(),
            self.settings.scratch_dir.clone(),
            local_root,
        )
    }

    fn transfer_ssh(&self) -> &SshTarget {
        self.data_ssh.as_ref().unwrap_or(&self.settings.ssh)
    }

    /// Installed only by the trusted formal runtime constructor. The recorder
    /// and each pending submission retain the original kernel lease.
    pub fn with_submission_recorder(
        mut self,
        recorder: Arc<dyn dag_core::SlurmSubmissionRecorder>,
    ) -> Self {
        self.submission_recorder = Some(recorder);
        self
    }

    /// Configure only from a frozen operator-approved policy. This measures
    /// actual node limits and readiness; it grants no scientific authority.
    pub fn with_resource_probe(
        mut self,
        program: ResourceProbeProgram,
        budget: Value,
    ) -> Result<Self> {
        absolute(&program.path)?;
        let hex = |value: &str, n| {
            value.len() == n
                && value
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        };
        let fields = [
            "max_concurrency",
            "cpus",
            "memory_bytes",
            "timeout_ms",
            "memory_available_reserve_bytes",
            "disk_reserve_fraction",
            "peak_disk_bytes",
        ];
        let positive = ["cpus", "memory_bytes", "timeout_ms", "peak_disk_bytes"];
        if !hex(&program.sha256, 64)
            || !hex(&program.engine_revision, 40)
            || program.bytes == 0
            || program.bytes > 2 * 1024 * 1024 * 1024
            || budget.as_object().is_none_or(|o| {
                o.len() != fields.len() || fields.iter().any(|k| !o.contains_key(*k))
            })
            || budget["max_concurrency"].as_u64() != Some(1)
            || positive
                .iter()
                .any(|k| !budget[*k].as_u64().is_some_and(|n| n > 0))
            || budget["memory_bytes"]
                .as_u64()
                .zip(budget["memory_available_reserve_bytes"].as_u64())
                .is_none_or(|(memory, reserve)| memory.checked_add(reserve).is_none())
            || !budget["disk_reserve_fraction"]
                .as_f64()
                .is_some_and(|n| n.is_finite() && (0.0..1.0).contains(&n))
        {
            return Err(invalid(
                "complete native resource probe and frozen task budget required",
            ));
        }
        self.resource_probe = Some((program, budget));
        Ok(self)
    }

    fn resource_probe_request(
        &self,
        request: &ContainerRunRequest,
        cpus: Option<u32>,
    ) -> Result<Option<Value>> {
        let Some((program, template)) = &self.resource_probe else {
            return Ok(None);
        };
        let cpu =
            cpus.ok_or_else(|| invalid("remote resource admission requires explicit CPUs"))?;
        let memory = request
            .memory
            .as_deref()
            .map(container_runtime::memory_quantity_bytes)
            .transpose()?
            .ok_or_else(|| invalid("remote resource admission requires explicit memory"))?;
        let timeout = request
            .timeout_secs
            .checked_mul(1000)
            .ok_or_else(|| invalid("timeout overflow"))?;
        if cpu == 0
            || memory == 0
            || timeout == 0
            || u64::from(cpu) > template["cpus"].as_u64().unwrap()
            || memory > template["memory_bytes"].as_u64().unwrap()
            || timeout > template["timeout_ms"].as_u64().unwrap()
        {
            return Err(invalid("remote request exceeds its signed task budget"));
        }
        let mut budget = template.clone();
        budget["cpus"] = json!(cpu);
        budget["memory_bytes"] = json!(memory);
        budget["timeout_ms"] = json!(timeout);
        Ok(Some(json!({"program":program,"budget":budget})))
    }

    /// Admit GPU requests only through an explicit operator partition.
    pub fn with_gpu_partition(mut self, partition: String) -> Result<Self> {
        if partition.is_empty()
            || !partition
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(invalid("invalid Slurm GPU partition"));
        }
        if partition == "debuga100" && self.gpu_qos.is_none() {
            self.gpu_qos = Some("debug".into());
        }
        self.gpu_partition = Some(partition);
        Ok(self)
    }

    /// Site CPU admission floor for each allocated GPU; positive and operator owned.
    pub fn with_gpu_cpus_per_gpu(mut self, cpus: u32) -> Result<Self> {
        if cpus == 0 {
            return Err(invalid("GPU CPU admission floor must be positive"));
        }
        self.gpu_cpus_per_gpu = cpus;
        Ok(self)
    }

    /// Override the GPU QOS when the operator profile requires one.
    pub fn with_gpu_qos(mut self, qos: String) -> Result<Self> {
        if qos.is_empty()
            || !qos
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(invalid("invalid GPU Slurm QOS"));
        }
        self.gpu_qos = Some(qos);
        Ok(self)
    }

    fn image(&self, reference: &str) -> Result<&SifImage> {
        Ok(self.image_binding(reference)?.1)
    }

    fn image_binding<'a>(&'a self, reference: &str) -> Result<(&'a str, &'a SifImage)> {
        if let Some((key, image)) = self.images.images.get_key_value(reference) {
            return Ok((key.as_str(), image));
        }
        // OCI digests are content addresses independent of repository aliases.
        // Only a full 64-character digest can match; filenames are never used.
        let digest = reference
            .rsplit_once("@sha256:")
            .map(|(_, d)| d)
            .filter(|d| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| invalid("container image must be pinned by full OCI digest"))?;
        let mut candidates = self
            .images
            .images
            .iter()
            .filter(|(key, _)| key.ends_with(&format!("@sha256:{digest}")));
        let (key, image) = candidates.next().ok_or_else(|| {
            invalid(format!(
                "image {reference} is absent from the site SIF inventory"
            ))
        })?;
        if candidates.any(|(_, other)| other != image) {
            return Err(invalid("ambiguous SIF conversion for this OCI digest"));
        }
        Ok((key.as_str(), image))
    }

    async fn remote_prefix(&self, value: &str) -> Result<String> {
        let value = if let Some(rest) = value.strip_prefix("~/") {
            let output = self.settings.ssh.run("cd && pwd").await.map_err(mapped)?;
            if !output.status.success() {
                return Err(invalid("remote home resolution failed"));
            }
            let home = String::from_utf8(output.stdout).map_err(mapped)?;
            format!("{}/{}", home.trim(), rest)
        } else {
            value.into()
        };
        absolute(Path::new(&value))
    }

    async fn remote_ok(&self, script: &str) -> Result<()> {
        let output = self
            .settings
            .ssh
            .run_script_once(script)
            .await
            .map_err(mapped)?;
        if !output.status.success() {
            return Err(invalid(format!(
                "remote staging failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    async fn upload_private(&self, local: &Path, destination: &str) -> Result<()> {
        let parent = Path::new(destination).parent().unwrap();
        let script = format!(
            "umask 077\nmkdir -p -- {}\nset -C\ncat > {}",
            quote(&absolute(parent)?),
            quote(destination)
        );
        let output = self
            .transfer_ssh()
            .run_with_file_stdin(&script, local)
            .await
            .map_err(mapped)?;
        if !output.status.success() {
            return Err(invalid("private file upload refused"));
        }
        Ok(())
    }

    async fn panel(&self, panel: &container_runtime::CachedPanel, task: &str) -> Result<Value> {
        let raw = panel.manifest_bytes.as_deref().ok_or_else(|| {
            invalid("remote panel transfer requires the original verified manifest bytes")
        })?;
        let manifest: container_runtime::PanelManifest =
            serde_json::from_slice(raw).map_err(mapped)?;
        manifest.validate().map_err(mapped)?;
        if manifest.id != panel.id || manifest.digest != panel.digest {
            return Err(invalid("panel cache binding differs"));
        }
        let remote_root = self.remote_prefix(&self.remote_panels).await?;
        let root = format!(
            "{}/{}",
            remote_root,
            bytes_hash(format!("{}\0{}", panel.id, panel.digest).as_bytes())
        );
        let manifest_hash = bytes_hash(raw);
        let mut manifest_file = tempfile::NamedTempFile::new()?;
        manifest_file.write_all(raw)?;
        manifest_file.as_file().sync_all()?;
        self.remote_ok(&format!("umask 077\nmkdir -p -- {}", quote(&root)))
            .await?;
        let mut entries = Vec::new();
        entries.push((
            "manifest.json".to_string(),
            manifest_file.path().to_path_buf(),
            raw.len() as u64,
            manifest_hash.clone(),
        ));
        for file in &manifest.files {
            relative(&file.path)?;
            let source = panel.host_path.join(&file.path);
            if source.is_symlink() || !source.is_file() {
                return Err(invalid("panel file is absent or symlinked"));
            }
            let expected = file.sha256.strip_prefix("sha256:").unwrap_or(&file.sha256);
            if source.metadata()?.len() != file.size || file_hash(&source)? != expected {
                return Err(invalid("local panel content changed before transfer"));
            }
            entries.push((file.path.clone(), source, file.size, expected.into()));
        }
        for (relative, source, size, hash) in entries {
            let destination = format!("{root}/{relative}");
            let check = self.settings.ssh.run(&format!("test -f {path} && test ! -L {path} && test \"$(stat -c %s -- {path})\" = {size}", path=quote(&destination))).await.map_err(mapped)?;
            if check.status.success() {
                continue;
            } // compute worker verifies full bytes before mounting
            let pending = format!("{destination}.pending-{task}");
            self.upload_private(&source, &pending).await?;
            // Publish only measured bytes; leave mismatching/foreign paths intact.
            self.remote_ok(&format!("set -eu\nprintf '%s  %s\\n' {hash} {pending} | /usr/bin/sha256sum -c --status\nif ! ln -- {pending} {destination}; then\n test ! -L {destination}\n printf '%s  %s\\n' {hash} {destination} | /usr/bin/sha256sum -c --status\nfi\nrm -- {pending}",
                hash=quote(&hash), pending=quote(&pending), destination=quote(&destination))).await?;
        }
        Ok(json!({"id": panel.id, "digest": panel.digest, "root": root,
            "manifest_sha256": manifest_hash, "mount_path": panel.mount_path}))
    }

    async fn run_inner(
        &self,
        request: ContainerRunRequest,
        private: Option<PrivateEnvironment>,
        observer: container_runtime::observation::RunObserver,
    ) -> Result<ContainerRunResult> {
        if let Some(recorder) = &self.submission_recorder {
            recorder.ensure_ready().map_err(mapped)?;
        }
        let reporter = dag_core::dag::node_event::NodeReporter::noop().with_observer(move |o| {
            observer.emit(container_runtime::observation::RuntimeObservation {
                phase: o.phase,
                current: o.current,
                total: o.total,
                unit: o.unit,
                message: o.message,
                worker: o.worker,
                observed_at_ms: o.observed_at_ms,
            });
        });
        reporter.phase(
            "preparing_remote_workspace",
            "preparing the remote container workspace",
        );
        container_runtime::connection::validate_run_request(&request)?;
        if request.cpus.is_some_and(|value| !value.is_finite()) {
            return Err(invalid("finite CPU count required"));
        }
        if request.pull_policy == PullPolicy::Always {
            return Err(invalid(
                "runtime image pulls are unavailable; admit a prepared SIF first",
            ));
        }
        // The worker creates a private tmpfs namespace with this exact byte cap.
        // Keep the original spelling too for operator provenance.
        let shm_size_bytes = request
            .shm_size
            .as_deref()
            .map(container_runtime::memory_quantity_bytes)
            .transpose()?;
        let gpu_count = gpu_allocation(&request.gpus, self.gpu_partition.as_deref())?;
        let effective_cpus = admitted_cpus(request.cpus, gpu_count, self.gpu_cpus_per_gpu)?;
        let resource_probe = self.resource_probe_request(&request, effective_cpus)?;
        if request.env.iter().any(|(name, _)| name == "OPENGWAS_JWT") {
            return Err(invalid(
                "credentials require the private environment channel",
            ));
        }
        let image = self.image(&request.image)?.clone();
        absolute(Path::new(&request.workspace.container_workdir))?;
        for panel in &request.panels {
            absolute(Path::new(&panel.mount_path))?;
            let destination = Path::new(&panel.mount_path);
            let workdir = Path::new(&request.workspace.container_workdir);
            if destination.starts_with(workdir) || workdir.starts_with(destination) {
                return Err(invalid("panel mount overlaps the writable workspace"));
            }
        }
        let task = container_runtime::unique_container_name();
        let root = self.journal.join(&task);
        private_dir(&root)?;
        let journal = root.join("STATE.json");
        let remote = format!(
            "{}/{}",
            self.remote_prefix(&self.settings.scratch_dir).await?,
            task
        );
        let mut state = json!({"schema": "SLURM_CONTAINER_ATTEMPT_V1", "task_id": task,
            // A retry has a fresh transport attempt. Bind the caller without
            // treating timestamps or equal job counts as provenance.
            "request_name": request.name,
            "request_workspace": {"host_path": request.workspace.host_path,
                "container_workdir": request.workspace.container_workdir},
            "remote_workspace": remote, "image": request.image, "sif": image,
            "worker_sha256": bytes_hash(WORKER.as_bytes()), "state": "PREPARING", "job_id": null,
            "remote_scratch_cleaned": false, "private_environment": private.is_some(),
            "gpu_allocation": {"gpus": gpu_count,
                "partition": if gpu_count.is_some() { self.gpu_partition.as_deref() } else { Some(self.settings.partition.as_str()) },
                "account": self.settings.account, "cpus_per_gpu": self.gpu_cpus_per_gpu,
                "effective_cpus": effective_cpus,
                "qos": if gpu_count.is_some() { self.gpu_qos.as_ref() } else { self.settings.qos.as_ref() }}});
        record(&journal, &state)?;
        self.remote_ok(&format!(
            "umask 077\nmkdir -p -- {}\nmkdir -m 700 -- {}",
            quote(remote.rsplit_once('/').unwrap().0),
            quote(&remote)
        ))
        .await?;
        if let Some(data) = &self.data_ssh {
            // Prove the two endpoints see this exact fresh attempt directory,
            // rather than assuming that equal absolute paths share storage.
            let witness = format!("{remote}/.data-endpoint-witness");
            self.remote_ok(&format!(
                "umask 077\nprintf %s {} > {}",
                quote(&task),
                quote(&witness)
            ))
            .await?;
            let seen = data
                .run(&format!(
                    "test -f {path} && test ! -L {path} && test \"$(cat -- {path})\" = {value}",
                    path = quote(&witness),
                    value = quote(&task)
                ))
                .await
                .map_err(mapped)?;
            if !seen.status.success() {
                return Err(invalid(
                    "bulk endpoint cannot see the control attempt witness",
                ));
            }
        }
        let store = self.artifact_store(root.join("downloads"));
        let (directories, files) = workspace_files_observed(&request, &reporter)?;
        for (item, source) in &files {
            store
                .upload_observed(
                    FileRef::local(source, None).map_err(mapped)?,
                    &task,
                    &format!("payload/{}", item["path"].as_str().unwrap()),
                    &reporter,
                )
                .await
                .map_err(mapped)?;
        }
        let mut panels = Vec::new();
        for panel in &request.panels {
            reporter.phase(
                "preparing_remote_panels",
                "verifying and transferring reference panels",
            );
            panels.push(self.panel(panel, &task).await?);
        }
        let worker_path = self
            .journal
            .join("sources")
            .join(format!("{}.py", bytes_hash(WORKER.as_bytes())));
        store
            .upload(
                FileRef::local(&worker_path, None).map_err(mapped)?,
                &task,
                "container-worker.py",
            )
            .await
            .map_err(mapped)?;
        let mut payload_request = json!({"schema": "APPTAINER_CONTAINER_REQUEST_V1", "program": self.images.program,
            "sif": image, "command": request.command, "env": request.env,
            "workdir": request.workspace.container_workdir, "files": files.iter().map(|(v,_)| v).collect::<Vec<_>>(),
            "directories": directories, "panels": panels,
            "network": match request.network { ContainerNetwork::Isolated => "isolated", ContainerNetwork::Egress => "egress" },
            "read_only_rootfs": request.read_only_rootfs, "user": request.user,
            "pids_limit": request.pids_limit, "shm_size": request.shm_size,
            "shm_size_bytes": shm_size_bytes,
            "gpus": gpu_count.is_some(), "gpu_count": gpu_count,
            "private_environment": private.is_some()});
        if let Some(probe) = &resource_probe {
            payload_request["resource_probe"] = probe.clone();
        }
        let request_bytes = serde_json::to_vec(&payload_request).map_err(mapped)?;
        record(&root.join("REQUEST.json"), &payload_request)?;
        let mut request_file = tempfile::NamedTempFile::new()?;
        request_file.write_all(&request_bytes)?;
        store
            .upload(
                FileRef::local(request_file.path(), None).map_err(mapped)?,
                &task,
                "container-request.json",
            )
            .await
            .map_err(mapped)?;
        if let Some(private) = private {
            let mut file = tempfile::NamedTempFile::new()?;
            let values: BTreeMap<_, _> = private.values().iter().cloned().collect();
            file.write_all(
                &serde_json::to_vec(&values)
                    .map_err(|_| invalid("private environment encoding failed"))?,
            )?;
            self.upload_private(file.path(), &format!("{remote}/private-environment.json"))
                .await?;
        }
        let script = format!(
            "set -eu\ncd \"$AUTONOMICS_WORKSPACE\"\nprintf '%s  %s\\n' {} 'container-worker.py' | /usr/bin/sha256sum -c --status\nprintf '%s  %s\\n' {} 'container-request.json' | /usr/bin/sha256sum -c --status\nexec {} -B container-worker.py\n",
            quote(&bytes_hash(WORKER.as_bytes())),
            quote(&bytes_hash(&request_bytes)),
            quote(&self.python)
        );
        let process = ProcessTaskSpec {
            container: None,
            script: Some(script),
            command: vec![],
            env: BTreeMap::new(),
            outputs: vec![ProcessTaskOutput {
                port: 0,
                pattern: "container-result.json".into(),
                optional: false,
            }],
        };
        let dispatch = TaskDispatch {
            spec: TaskSpec {
                id: task.clone(),
                logical_node: None,
                kind: "process".into(),
                spec: serde_json::to_value(process).map_err(mapped)?,
                axis: None,
                item_key: None,
                item: None,
                inputs: vec![],
                outputs: vec![TaskOutputBinding {
                    name: "result".into(),
                    port: 0,
                    payload: "file".into(),
                    artifact_paths: vec![],
                }],
            },
            resources: TaskResources {
                cpus: effective_cpus,
                memory_bytes: request
                    .memory
                    .as_deref()
                    .map(container_runtime::memory_quantity_bytes)
                    .transpose()?,
                max_duration_ms: Some(
                    request
                        .timeout_secs
                        .checked_mul(1000)
                        .ok_or_else(|| invalid("timeout overflow"))?,
                ),
            },
        };
        state["dispatch"] = serde_json::to_value(&dispatch).map_err(mapped)?;
        state["state"] = json!("SUBMITTING");
        record(&journal, &state)?;
        let mut settings = self.settings.clone();
        // A node receives only its allocation; `all` never requests the whole host.
        settings.gpus = gpu_count;
        if gpu_count.is_some() {
            settings.partition = self.gpu_partition.clone().unwrap();
            settings.qos = self.gpu_qos.clone();
        }
        settings.local_artifact_root = root.clone();
        let mut transport = SlurmTaskTransport::new(settings);
        if let Some(recorder) = &self.submission_recorder {
            transport = transport.with_submission_recorder(Arc::clone(recorder));
        }
        let transport = Arc::new(transport);
        reporter.phase("submitting", "submitting the admitted Slurm job");
        let lease = match transport.submit(dispatch, vec![]).await {
            Ok(lease) => lease,
            Err(error) => {
                record_submission_failure(&journal, &mut state, &error)?;
                return Err(mapped(error));
            }
        };
        state["job_id"] = json!(lease.lease_id);
        state["state"] = json!("SUBMITTED");
        record(&journal, &state)?;
        let mut guard = RemoteCancelGuard {
            transport: Arc::clone(&transport),
            ssh: self.settings.ssh.clone(),
            lease: Some(lease.clone()),
            journal: journal.clone(),
        };
        let receipt = match tokio::time::timeout(
            Duration::from_secs(request.timeout_secs.saturating_add(60)),
            transport.wait_observed(&lease, &reporter),
        )
        .await
        {
            Ok(Ok(receipt)) => receipt,
            _ => {
                let _ = transport.cancel(&lease).await;
                reporter.phase(
                    "checking_worker_stop",
                    "verifying the remote worker subtree has stopped",
                );
                let proof = stop_proof(&self.settings.ssh, &lease.lease_id).await?;
                state["stop_proof"] = proof.clone();
                state["state"] = json!(if proof["full_subtree_terminal"] == true {
                    "STOPPED_FAILED"
                } else {
                    "STOP_UNKNOWN"
                });
                record(&journal, &state)?;
                if proof["full_subtree_terminal"] == true {
                    guard.lease = None;
                    report_stopped_worker(&reporter, &root, &proof)?;
                }
                return Err(ContainerRuntimeError::Timeout {
                    timeout_secs: request.timeout_secs,
                });
            }
        };
        let proof = stop_proof(&self.settings.ssh, &lease.lease_id).await?;
        state["receipt"] = serde_json::to_value(&receipt).map_err(mapped)?;
        state["stop_proof"] = proof.clone();
        if proof["full_subtree_terminal"] != true {
            state["state"] = json!("STOP_UNKNOWN");
            record(&journal, &state)?;
            return Err(invalid(
                "remote stop unconfirmed; workspace and attempt retained",
            ));
        }
        guard.lease = None;
        state["state"] = json!("STOPPED");
        record(&journal, &state)?;
        report_stopped_worker(&reporter, &root, &proof)?;
        if receipt.exit_code != 0 {
            return Err(invalid(
                "Slurm worker failed; attempt and remote diagnostics retained",
            ));
        }
        let artifact = receipt
            .output_artifacts_by_port
            .get(&0)
            .and_then(|files| files.first())
            .ok_or_else(|| invalid("container result missing"))?;
        let result_file = store
            .download(artifact.clone(), &task, "result.json")
            .await
            .map_err(mapped)?;
        let result: Value =
            serde_json::from_slice(&std::fs::read(&result_file.path)?).map_err(mapped)?;
        if result["schema"] != "APPTAINER_CONTAINER_RESULT_V1" {
            return Err(invalid("unrecognized container result"));
        }
        if let Some(value) = result.get("worker_software") {
            let manifest_sha256 = receipt
                .task_manifest
                .fingerprint
                .as_ref()
                .and_then(|fingerprint| fingerprint.content_hash.as_deref())
                .ok_or_else(|| {
                    invalid("worker software evidence requires the submitted manifest SHA256")
                })?;
            let software = crate::worker_software::verify(
                value,
                crate::worker_software::ExpectedWorkerSoftware {
                    task_id: &task,
                    job_id: &lease.lease_id,
                    manifest_sha256: manifest_sha256
                        .strip_prefix("sha256:")
                        .unwrap_or(manifest_sha256),
                    request_sha256: &bytes_hash(&request_bytes),
                    worker_sha256: &bytes_hash(WORKER.as_bytes()),
                    sif_path: image
                        .path
                        .to_str()
                        .ok_or_else(|| invalid("UTF-8 SIF path required"))?,
                    sif_sha256: &image.sha256,
                    succeeded: result["exit_code"] == 0,
                },
            )
            .map_err(invalid)?;
            record(&root.join("WORKER_SOFTWARE.json"), &software)?;
            state["worker_software"] = software.clone();
            record(&journal, &state)?;
            reporter.execution(dag_core::dag::live::ExecutionObservation {
                phase: "collecting_outputs".into(),
                worker: Some(
                    json!({"evidence":"verified_software", "backend":"slurm-apptainer",
                    "id":lease.lease_id,"software":software}),
                ),
                ..Default::default()
            });
        } else if result["exit_code"] == 0 {
            return Err(invalid(
                "worker software evidence missing on a successful result",
            ));
        }
        if let Some(workspace) = result.get("node_workspace") {
            // This is the worker's local scratch disposition, distinct from
            // the independently obtained whole-allocation stop proof above.
            state["node_workspace"] = workspace.clone();
            record(&journal, &state)?;
        }
        if let Some(failure) = result.get("failure") {
            // Worker diagnostics contain only fixed public codes/phases, never
            // exception text or payload values. Retain them with the attempt.
            state["worker_failure"] = failure.clone();
            record(&journal, &state)?;
        }
        let entries = result["files"]
            .as_array()
            .ok_or_else(|| invalid("invalid output transfer manifest"))?;
        let exit_code: i32 = result["exit_code"]
            .as_i64()
            .and_then(|n| n.try_into().ok())
            .ok_or_else(|| invalid("invalid tool exit code"))?;
        if let Some(expected) = &resource_probe {
            let admitted = &result["resource_admission"];
            if (exit_code == 0 || !entries.is_empty())
                && (admitted["schema"] != "FORMAL_REMOTE_RESOURCE_PROBE_V1"
                    || admitted["job_id"] != lease.lease_id
                    || admitted["budget"] != expected["budget"]
                    || admitted["engine_revision"] != expected["program"]["engine_revision"]
                    || admitted["kernel_limits_verified"] != true
                    || admitted["readiness"]["ready"] != true
                    || admitted["formal_authorized"] != false
                    || admitted["scientific_gate_granted"] != false)
            {
                return Err(invalid(
                    "remote resource proof differs from the actual allocation request",
                ));
            }
            state["resource_admission"] = admitted.clone();
        }
        let temporary = tempfile::tempdir()?;
        let mut restored = Vec::new();
        let mut seen = BTreeSet::new();
        for (index, item) in entries.iter().enumerate() {
            let name = item["path"]
                .as_str()
                .ok_or_else(|| invalid("missing output path"))?;
            relative(name)?;
            if !seen.insert(name) || item["index"].as_u64() != Some(index as u64) {
                return Err(invalid("duplicate or unordered output transfer"));
            }
            let size = item["size"]
                .as_u64()
                .ok_or_else(|| invalid("missing output size"))?;
            let hash = item["sha256"]
                .as_str()
                .ok_or_else(|| invalid("missing output hash"))?;
            let file = FileRef::remote(
                format!("{}/export/{index}", receipt.workspace),
                None,
                size,
                Some(hash.into()),
            );
            let downloaded = store
                .download_observed(file, &task, &format!("export-{index}"), &reporter)
                .await
                .map_err(mapped)?;
            let source = PathBuf::from(downloaded.path);
            if source.metadata()?.len() != size || file_hash(&source)? != hash {
                return Err(invalid("restored output identity differs"));
            }
            let staged = temporary.path().join(index.to_string());
            std::fs::copy(&source, &staged)?;
            restored.push((
                name.to_string(),
                staged,
                item["mode"].as_u64().unwrap_or(0o600) as u32 & 0o777,
            ));
        }
        // Validate every destination before changing any caller-visible output.
        for (name, _, _) in &restored {
            let mut destination = request.workspace.host_path.clone();
            for component in relative(name)?.components() {
                destination.push(component);
                if destination.is_symlink() {
                    return Err(invalid("output destination contains a symlink"));
                }
            }
        }
        for (name, source, mode) in restored {
            let destination = request.workspace.host_path.join(name);
            std::fs::create_dir_all(destination.parent().unwrap())?;
            let mut staged = tempfile::NamedTempFile::new_in(destination.parent().unwrap())?;
            std::io::copy(&mut std::fs::File::open(source)?, &mut staged)?;
            staged
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(mode))?;
            staged.as_file().sync_all()?;
            staged.persist(destination).map_err(mapped)?;
        }
        state["state"] = json!(if exit_code == 0 {
            "COMPLETE"
        } else {
            "TOOL_FAILED"
        });
        state["exit_code"] = json!(exit_code);
        record(&journal, &state)?;
        let stdout = result["stdout"].as_str().unwrap_or_default().to_string();
        let stderr = result["stderr"].as_str().unwrap_or_default().to_string();
        if exit_code != 0 {
            return Err(ContainerRuntimeError::ExitStatus {
                exit_code,
                stdout,
                stderr,
            });
        }
        Ok(ContainerRunResult {
            exit_code,
            stdout,
            stderr,
        })
    }
}

#[async_trait]
impl PodmanConnection for SlurmContainerRuntime {
    fn image_resolution(
        &self,
        reference: &str,
    ) -> Result<container_runtime::ContainerImageResolution> {
        let (key, image) = self.image_binding(reference)?;
        Ok(container_runtime::ContainerImageResolution {
            requested_reference: reference.into(),
            matched_reference: key.into(),
            sif_path: image.path.clone(),
            sif_sha256: image.sha256.clone(),
        })
    }
    fn name(&self) -> &'static str {
        "slurm-apptainer"
    }
    fn workspace_root(&self) -> &Path {
        &self.workspace
    }
    async fn run(&self, request: ContainerRunRequest) -> Result<ContainerRunResult> {
        self.run_inner(request, None, Default::default()).await
    }
    async fn run_with_private_env(
        &self,
        request: ContainerRunRequest,
        private: PrivateEnvironment,
    ) -> Result<ContainerRunResult> {
        self.run_observed(request, Some(private), Default::default())
            .await
    }

    async fn run_observed(
        &self,
        request: ContainerRunRequest,
        private: Option<PrivateEnvironment>,
        observer: container_runtime::observation::RunObserver,
    ) -> Result<ContainerRunResult> {
        let suppress = private.is_some();
        let result = self.run_inner(request, private, observer).await;
        if !suppress {
            return result;
        }
        result.map_err(|error| match error {
            ContainerRuntimeError::Timeout { timeout_secs } => {
                ContainerRuntimeError::Timeout { timeout_secs }
            }
            ContainerRuntimeError::ExitStatus { exit_code, .. } => {
                ContainerRuntimeError::ExitStatus {
                    exit_code,
                    stdout: String::new(),
                    stderr: "private container failed; captured output suppressed".into(),
                }
            }
            _ => invalid("private container execution failed; diagnostics suppressed"),
        })
    }
}

struct RemoteCancelGuard {
    transport: Arc<SlurmTaskTransport>,
    ssh: SshTarget,
    lease: Option<TaskLease>,
    journal: PathBuf,
}
impl Drop for RemoteCancelGuard {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        let transport = Arc::clone(&self.transport);
        let ssh = self.ssh.clone();
        let journal = self.journal.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = transport.cancel(&lease).await;
                let proof = stop_proof(&ssh, &lease.lease_id).await;
                if let Ok(raw) = std::fs::read(&journal) {
                    if let Ok(mut state) = serde_json::from_slice::<Value>(&raw) {
                        if state["job_id"] != lease.lease_id {
                            return;
                        }
                        state["state"] = json!("STOP_UNKNOWN");
                        if let Ok(proof) = proof {
                            if proof["full_subtree_terminal"] == true {
                                state["state"] = json!("CANCELLED_STOPPED");
                            }
                            state["stop_proof"] = proof;
                        }
                        let _ = record(&journal, &state);
                    }
                }
            });
        }
        // The durable RUNNING record remains pending if the runtime is gone.
        // Neither branch removes remote scratch or infers a successful exit.
    }
}

/// Independent whole-allocation proof, including batch and extern steps.
pub async fn stop_proof(ssh: &SshTarget, job: &str) -> Result<Value> {
    if job.is_empty() || !job.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("invalid Slurm job ID"));
    }
    let mut last = Value::Null;
    for _ in 0..15 {
        let queue = ssh
            .run(&format!("squeue -h -j {job} -o %T"))
            .await
            .map_err(mapped)?;
        let accounting = ssh
            .run(&format!(
                "sacct -n -P -j {job} --format=JobID,State%40,ExitCode,End"
            ))
            .await
            .map_err(mapped)?;
        let control = ssh
            .run(&format!("scontrol show job -o {job}"))
            .await
            .map_err(mapped)?;
        let raw = String::from_utf8(accounting.stdout.clone()).map_err(mapped)?;
        let rows: Vec<_> = raw
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.split('|').collect::<Vec<_>>())
            .collect();
        let complete = rows.len() >= 3
            && [
                job.to_string(),
                format!("{job}.batch"),
                format!("{job}.extern"),
            ]
            .iter()
            .all(|id| {
                rows.iter()
                    .filter(|row| row.first() == Some(&id.as_str()))
                    .count()
                    == 1
            })
            && rows.iter().all(|row| {
                row.len() == 4
                    && (row[0] == job || row[0].starts_with(&format!("{job}.")))
                    && matches!(
                        row[1],
                        "COMPLETED"
                            | "FAILED"
                            | "CANCELLED"
                            | "TIMEOUT"
                            | "OUT_OF_MEMORY"
                            | "NODE_FAIL"
                            | "PREEMPTED"
                            | "BOOT_FAIL"
                            | "DEADLINE"
                            | "REVOKED"
                    )
                    && row[2].split_once(':').is_some_and(|(a, b)| {
                        !a.is_empty()
                            && !b.is_empty()
                            && a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit())
                    })
                    && !matches!(row[3], "" | "Unknown" | "None")
            });
        let control_text = String::from_utf8_lossy(&control.stdout);
        let consistent = rows
            .iter()
            .find(|row| row.first() == Some(&job))
            .is_some_and(|row| {
                row.len() == 4
                    && [
                        format!("JobId={job}"),
                        format!("JobState={}", row[1]),
                        format!("ExitCode={}", row[2]),
                    ]
                    .iter()
                    .all(|field| control_text.split_whitespace().any(|value| value == field))
            });
        let proven = queue.status.success()
            && queue.stdout.iter().all(u8::is_ascii_whitespace)
            && accounting.status.success()
            && control.status.success()
            && complete
            && consistent;
        last = json!({"job_id": job, "full_subtree_terminal": proven,
            "queue": {"exit": queue.status.code(), "stdout": String::from_utf8_lossy(&queue.stdout), "stderr": String::from_utf8_lossy(&queue.stderr)},
            "accounting": {"exit": accounting.status.code(), "stdout": raw, "stderr": String::from_utf8_lossy(&accounting.stderr)},
            "control": {"exit": control.status.code(), "stdout": control_text, "stderr": String::from_utf8_lossy(&control.stderr)}});
        if proven {
            return Ok(last);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Ok(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use container_runtime::{InputMount, WorkspaceRef};

    #[test]
    fn submission_journal_uses_protocol_evidence_not_error_text() {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::set_permissions(fixture.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let journal = fixture.path().join("STATE.json");
        for stopped in [true, false] {
            let mut state = json!({"state":"SUBMITTING", "dispatch":{"id":"original"}});
            record_submission_failure(
                &journal,
                &mut state,
                &dag_core::dag::DagError::SubmissionNotSent {
                    message: "synthetic authentication denial".into(),
                    local_ssh_stopped: stopped,
                },
            )
            .unwrap();
            let saved: Value = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
            assert_eq!(saved["state"], "SUBMISSION_NOT_SENT");
            assert_eq!(saved["local_ssh_stopped"], stopped);
            assert_eq!(saved["automatic_replay_authorized"], false);
            assert_eq!(saved["dispatch"]["id"], "original");
        }
        let mut state = json!({"state":"SUBMITTING"});
        record_submission_failure(
            &journal,
            &mut state,
            &dag_core::dag::DagError::Schedule(
                "Permission denied; payload NOT_SENT; initial SSH exit 255".into(),
            ),
        )
        .unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
        assert_eq!(saved["state"], "SUBMISSION_UNKNOWN");
        assert_eq!(saved["automatic_replay_authorized"], false);
    }

    #[test]
    fn stop_projection_retains_accounting_and_refuses_unknown_without_exporting_commands() {
        let proof = json!({"job_id":"123","full_subtree_terminal":true,
            "accounting":{"stdout":"123|COMPLETED|0:0|2026-10-29T01:00:00\n123.batch|COMPLETED|0:0|2026-10-29T01:00:00\n123.extern|COMPLETED|0:0|2026-10-29T01:00:00\n"},
            "control":{"stdout":"JobId=123 JobState=COMPLETED ExitCode=0:0 EndTime=2026-10-29T01:00:00 Command=private-payload WorkDir=/private"}});
        let projected = stopped_worker_projection(&proof).unwrap();
        assert_eq!(projected["accounting"].as_array().unwrap().len(), 3);
        assert_eq!(projected["control"]["JobId"], "123");
        assert_eq!(
            projected["raw_proof_sha256"],
            bytes_hash(&serde_json::to_vec_pretty(&proof).unwrap())
        );
        assert!(!projected.to_string().contains("private"));
        let mut unknown = proof;
        unknown["full_subtree_terminal"] = false.into();
        assert!(stopped_worker_projection(&unknown).is_err());
    }

    #[tokio::test]
    async fn bulk_payloads_and_private_panel_uploads_use_only_data_endpoint() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        let log = fixture.path().join("data-calls");
        let fake = fixture.path().join("data-ssh");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf 'data\\n' >> {}\ncat > /dev/null\n",
                quote(log.to_str().unwrap())
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        let settings = SlurmSettings {
            ssh: SshTarget::new("/usr/bin/false", "control.invalid"),
            scratch_dir: "/remote/attempts".into(),
            local_artifact_root: fixture.path().join("downloads"),
            partition: "test".into(),
            account: None,
            gpus: None,
            qos: None,
            mem_per_cpu: "1G".into(),
            default_time: "00:01:00".into(),
            poll_interval: Duration::from_millis(1),
            apptainer: Some(ApptainerSettings {
                program: "/usr/bin/apptainer".into(),
                images: BTreeMap::from([(
                    format!("example/toy@sha256:{}", "a".repeat(64)),
                    SifImage {
                        path: "/admitted/toy.sif".into(),
                        sha256: "b".repeat(64),
                    },
                )]),
            }),
        };
        let runtime = SlurmContainerRuntime::new(
            settings,
            fixture.path().join("work"),
            fixture.path().join("journal"),
            "/remote/panels".into(),
            "/usr/bin/python3".into(),
        )
        .unwrap()
        .with_data_ssh(SshTarget::new(fake.to_str().unwrap(), "data.invalid"));
        let input = fixture.path().join("input");
        std::fs::write(&input, b"synthetic input").unwrap();
        runtime
            .artifact_store(fixture.path().join("downloads"))
            .upload(FileRef::local(&input, None).unwrap(), "attempt", "payload")
            .await
            .unwrap();
        runtime
            .upload_private(&input, "/remote/panels/panel.pending")
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(log).unwrap(), "data\ndata\ndata\n");
        // A data endpoint must never replace the scheduler/stop-proof channel.
        assert!(runtime.remote_ok("true").await.is_err());
    }

    #[tokio::test]
    async fn failed_prepare_retains_exact_caller_and_distinct_attempt_identity() {
        let fixture = tempfile::tempdir().unwrap();
        let work = fixture.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let journal = fixture.path().join("attempts");
        let image = format!("example/toy@sha256:{}", "a".repeat(64));
        let settings = SlurmSettings {
            apptainer: Some(ApptainerSettings {
                program: "/usr/bin/apptainer".into(),
                images: BTreeMap::from([(
                    image.clone(),
                    SifImage {
                        path: "/admitted/toy.sif".into(),
                        sha256: "b".repeat(64),
                    },
                )]),
            }),
            // Fail before submission; this test cannot contact a cluster.
            ssh: SshTarget::new("/usr/bin/false", "unused.invalid"),
            scratch_dir: "/remote/attempts".into(),
            local_artifact_root: fixture.path().join("downloads"),
            partition: "test".into(),
            account: None,
            gpus: None,
            qos: None,
            mem_per_cpu: "1G".into(),
            default_time: "00:01:00".into(),
            poll_interval: Duration::from_millis(1),
        };
        let runtime = SlurmContainerRuntime::new(
            settings.clone(),
            fixture.path().join("runtime"),
            journal.clone(),
            "/remote/panels".into(),
            "/usr/bin/python3".into(),
        )
        .unwrap();
        let resolved = runtime.image_resolution(&image).unwrap();
        assert_eq!(resolved.matched_reference, image);
        assert_eq!(resolved.sif_sha256, "b".repeat(64));
        let alias = format!("alias/toy@sha256:{}", "a".repeat(64));
        let aliased = runtime.image_resolution(&alias).unwrap();
        assert_eq!(aliased.requested_reference, alias);
        assert_eq!(aliased.matched_reference, image);
        assert_eq!(aliased.sif_path, resolved.sif_path);
        assert!(runtime.image_resolution("toy:latest").is_err());
        let request = ContainerRunRequest {
            image,
            command: vec!["true".into()],
            workspace: WorkspaceRef {
                host_path: work.clone(),
                container_workdir: "/work".into(),
            },
            env: vec![],
            panels: vec![],
            input_mounts: vec![],
            network: ContainerNetwork::Isolated,
            read_only_rootfs: true,
            pull_policy: PullPolicy::Never,
            cpus: Some(1.),
            memory: Some("1GiB".into()),
            pids_limit: Some(64),
            shm_size: None,
            gpus: GpuRequest::None,
            user: None,
            timeout_secs: 60,
            name: "original-node-request".into(),
        };
        assert!(
            runtime
                .resource_probe_request(&request, Some(1))
                .unwrap()
                .is_none()
        );
        let program = ResourceProbeProgram {
            path: "/synthetic/research-worker".into(),
            sha256: "c".repeat(64),
            bytes: 100,
            engine_revision: "d".repeat(40),
        };
        let budget = json!({"max_concurrency":1,"cpus":8,"memory_bytes":32_u64*1024*1024*1024,
            "timeout_ms":180000,"memory_available_reserve_bytes":123,
            "disk_reserve_fraction":0.01,"peak_disk_bytes":456});
        // Invalid/incomplete declarations fail before any SSH or submission.
        let mut incomplete = budget.clone();
        incomplete["allow_unbounded"] = json!(true);
        assert!(
            runtime
                .with_resource_probe(program.clone(), incomplete)
                .is_err()
        );
        let runtime = SlurmContainerRuntime::new(
            settings,
            fixture.path().join("runtime"),
            journal.clone(),
            "/remote/panels".into(),
            "/usr/bin/python3".into(),
        )
        .unwrap()
        .with_resource_probe(program, budget.clone())
        .unwrap();
        let projected = runtime
            .resource_probe_request(&request, Some(1))
            .unwrap()
            .unwrap();
        let mut expected = budget;
        expected["cpus"] = json!(1);
        expected["memory_bytes"] = json!(1024_u64 * 1024 * 1024);
        expected["timeout_ms"] = json!(60000);
        assert_eq!(projected["budget"], expected);
        assert!(runtime.resource_probe_request(&request, None).is_err());
        assert!(runtime.resource_probe_request(&request, Some(9)).is_err());
        let mut bad = request.clone();
        bad.memory = Some("64GiB".into());
        assert!(runtime.resource_probe_request(&bad, Some(1)).is_err());
        bad = request.clone();
        bad.timeout_secs = 181;
        assert!(runtime.resource_probe_request(&bad, Some(1)).is_err());
        assert!(runtime.run(request).await.is_err());
        let attempts: Vec<_> = std::fs::read_dir(journal)
            .unwrap()
            .map(|entry| entry.unwrap().path().join("STATE.json"))
            .filter(|path| path.is_file())
            .collect();
        assert_eq!(attempts.len(), 1);
        let state: Value = serde_json::from_slice(&std::fs::read(&attempts[0]).unwrap()).unwrap();
        assert_eq!(state["request_name"], "original-node-request");
        assert_eq!(state["request_workspace"]["host_path"], json!(work));
        assert_eq!(state["request_workspace"]["container_workdir"], "/work");
        assert_ne!(state["task_id"], state["request_name"]);
        assert_eq!(state["state"], "PREPARING");
        assert!(state["job_id"].is_null());
    }

    #[test]
    fn gpu_requests_are_bounded_and_require_scheduler_admission() {
        assert_eq!(gpu_allocation(&GpuRequest::None, None).unwrap(), None);
        assert!(gpu_allocation(&GpuRequest::All, None).is_err());
        assert_eq!(
            gpu_allocation(&GpuRequest::All, Some("a100")).unwrap(),
            Some(1)
        );
        assert_eq!(
            gpu_allocation(&GpuRequest::Count(3), Some("a100")).unwrap(),
            Some(3)
        );
        assert!(gpu_allocation(&GpuRequest::Count(0), Some("a100")).is_err());
        assert!(gpu_allocation(&GpuRequest::Devices("0".into()), Some("a100")).is_err());
    }

    #[test]
    fn gpu_cpu_floor_is_admitted_without_changing_cpu_only_requests() {
        assert_eq!(admitted_cpus(Some(1.5), None, 16).unwrap(), Some(2));
        assert_eq!(admitted_cpus(None, None, 16).unwrap(), None);
        assert_eq!(admitted_cpus(Some(4.0), Some(1), 16).unwrap(), Some(16));
        assert_eq!(admitted_cpus(Some(40.0), Some(2), 16).unwrap(), Some(40));
        assert_eq!(admitted_cpus(None, Some(2), 16).unwrap(), Some(32));
        assert!(admitted_cpus(None, Some(u32::MAX), 16).is_err());
    }

    #[test]
    fn transfer_copies_only_admitted_staged_input_links() {
        let fixture = tempfile::tempdir().unwrap();
        let work = fixture.path().join("work");
        let inputs = fixture.path().join("input");
        std::fs::create_dir(&work).unwrap();
        std::fs::create_dir(&inputs).unwrap();
        std::fs::write(inputs.join("selected"), b"selected").unwrap();
        std::fs::write(inputs.join("unrelated"), b"must not upload").unwrap();
        std::fs::create_dir_all(work.join(".autonomics/receipts/attempt")).unwrap();
        std::fs::write(
            work.join(".autonomics/receipts/attempt/task.json"),
            b"coordinator receipt",
        )
        .unwrap();
        std::os::unix::fs::symlink(inputs.join("selected"), work.join("staged")).unwrap();
        let mut request = ContainerRunRequest {
            image: "unused".into(),
            command: vec!["true".into()],
            workspace: WorkspaceRef {
                host_path: work.clone(),
                container_workdir: "/work".into(),
            },
            env: vec![],
            panels: vec![],
            input_mounts: vec![InputMount {
                host_dir: inputs.clone(),
            }],
            network: ContainerNetwork::Isolated,
            read_only_rootfs: true,
            pull_policy: PullPolicy::Never,
            cpus: None,
            memory: None,
            pids_limit: None,
            shm_size: None,
            gpus: GpuRequest::None,
            user: None,
            timeout_secs: 60,
            name: "toy".into(),
        };
        let (_, files) = workspace_files(&request).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0["path"], "staged");
        assert_eq!(files[0].0["readonly"], true);
        assert_eq!(files[0].1, inputs.join("selected").canonicalize().unwrap());
        request.input_mounts.clear();
        assert!(workspace_files(&request).is_err());
        request.input_mounts.push(InputMount { host_dir: inputs });
        std::os::unix::fs::symlink(fixture.path(), work.join("directory-link")).unwrap();
        assert!(workspace_files(&request).is_err());
    }

    #[test]
    fn transfer_paths_reject_aliases_and_bind_delimiters() {
        for value in [
            "", "/root", "../file", "x/../y", "x//y", "x/./y", "x\\y", "x\ny",
        ] {
            assert!(relative(value).is_err(), "{value:?}");
        }
        assert!(relative(".autonomics/inputs/input.json").is_ok());
        for value in ["/", "relative", "/x/../y", "/x//y", "/x:/host", "/x,ro"] {
            assert!(absolute(Path::new(value)).is_err(), "{value:?}");
        }
        assert_eq!(
            absolute(Path::new("/work/output.json")).unwrap(),
            "/work/output.json"
        );
    }
}
