//! Explicitly enrolled, real Slurm synthetic smoke. Never uses tempfile and
//! never removes local evidence or remote scratch, including on failure.
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dag_core::dag::DagError;
use dag_core::dag::node_event::{JobResult, NodeReporter};
use dag_core::{
    ChannelNode, ChannelOperator, FileRef, NodeCtx, NodeInput, NodeValue, RemoteTaskExecutor,
    SlurmSettings, SlurmTaskTransport, SshArtifactStore, SshTarget, TaskAttemptReceipt,
    TaskDispatch, TaskExecution, TaskExecutor, TaskInputBinding, TaskInputSource, TaskLease,
    TaskOutputBinding, TaskResources, TaskSpec, TaskSubmission, TaskTransport,
    TypedRemoteTaskExecutor, TypedWorkerSettings,
};
use datafusion::arrow::{
    array::{Array, Int64Array, StringArray},
    datatypes::{DataType, Field, Schema},
    ipc::reader::FileReader,
    record_batch::RecordBatch,
};
use datafusion::prelude::SessionContext;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

type SmokeResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[path = "slurm_typed_worker_smoke/warehouse.rs"]
mod warehouse;

#[path = "slurm_typed_worker_smoke/podman_allocation.rs"]
mod podman_allocation;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    /// Existing, canonical, operator-owned 0700 permanent evidence directory.
    /// This test deliberately never cleans it, even when no lease is obtained.
    local_root: PathBuf,
    /// An operator-selected, unique technical namespace outside science data.
    remote_scratch_root: String,
    ssh_host: String,
    ssh_program: String,
    account: String,
    partition: String,
    worker: TypedWorkerSettings,
    #[serde(default)]
    warehouse: Option<warehouse::WarehouseSettings>,
    #[serde(default)]
    podman_allocation: Option<podman_allocation::PodmanAllocationSettings>,
}

fn clean_absolute(path: &Path) -> bool {
    path.is_absolute()
        && !path.as_os_str().is_empty()
        && path.components().all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
        // components() normalizes internal '.', so inspect the raw spelling too.
        && !path.to_string_lossy().split('/').any(|part| part == "." || part == "..")
}

fn private_json(path: &Path, value: &impl serde::Serialize) -> SmokeResult<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn digest(path: &Path) -> SmokeResult<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn read_profile() -> SmokeResult<Profile> {
    read_profile_mode(false)
}

fn read_profile_mode(warehouse: bool) -> SmokeResult<Profile> {
    read_profile_for(warehouse, false)
}

fn read_profile_for(warehouse: bool, podman_allocation: bool) -> SmokeResult<Profile> {
    if std::env::var("AUTONOMICS_TYPED_SMOKE_ALLOW_SUBMIT").as_deref() != Ok("1") {
        return Err(
            "set AUTONOMICS_TYPED_SMOKE_ALLOW_SUBMIT=1 to authorize this synthetic job".into(),
        );
    }
    let config = PathBuf::from(std::env::var("AUTONOMICS_TYPED_SMOKE_CONFIG")?);
    let config_meta = std::fs::symlink_metadata(&config)?;
    let uid = std::fs::metadata("/proc/self")?.uid();
    if !clean_absolute(&config)
        || !config_meta.is_file()
        || config_meta.uid() != uid
        || config_meta.mode() & 0o077 != 0
        || config_meta.len() > 65536
    {
        return Err(
            "profile must be an absolute operator-owned private regular file <=64KiB".into(),
        );
    }
    let profile: Profile = serde_json::from_slice(&std::fs::read(config)?)?;
    if profile.warehouse.is_some() != warehouse {
        return Err("warehouse settings must select only the warehouse smoke".into());
    }
    if profile.podman_allocation.is_some() != podman_allocation {
        return Err(
            "Podman allocation settings must select only the Podman allocation smoke".into(),
        );
    }
    let root = &profile.local_root;
    let root_meta = std::fs::symlink_metadata(root)?;
    if !clean_absolute(root)
        || root.canonicalize()? != *root
        || !root_meta.is_dir()
        || root_meta.uid() != uid
        || root_meta.mode() & 0o777 != 0o700
    {
        return Err("local_root must already be canonical, operator-owned and mode 0700".into());
    }
    if !clean_absolute(Path::new(&profile.remote_scratch_root))
        || profile.remote_scratch_root == "/"
        || profile
            .remote_scratch_root
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err(
            "remote_scratch_root must be a clean, absolute, whitespace-free technical namespace"
                .into(),
        );
    }
    if profile.ssh_host.is_empty()
        || profile.ssh_host.starts_with('-')
        || profile
            .ssh_host
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        || !clean_absolute(Path::new(&profile.ssh_program))
    {
        return Err("explicit SSH alias and absolute ssh_program required".into());
    }
    for token in [&profile.account, &profile.partition] {
        if token.is_empty()
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err("explicit safe account/partition required".into());
        }
    }
    profile.worker.validate()?;
    if profile.worker.expected_source_revision != dag_core::source_revision() {
        return Err("test binary and worker source revision pins differ".into());
    }
    if profile.worker.plugins_root.is_some()
        || profile.worker.apptainer_catalog.is_some()
        || profile.worker.podman.is_some()
        || profile.worker.worker_vfs_manifest.is_some()
    {
        return Err("these synthetic smokes use no plugins, containers or data-home mounts".into());
    }
    Ok(profile)
}

#[tokio::test]
#[ignore = "submits three real synthetic Slurm tasks; requires a unique private warehouse profile"]
async fn real_cluster_typed_warehouse_roundtrip() -> SmokeResult<()> {
    warehouse::run().await
}

#[tokio::test]
#[ignore = "submits two real synthetic Slurm tasks with a pinned root-only OCI observer; requires explicit Podman allocation profile"]
async fn real_cluster_typed_podman_allocation_roundtrip() -> SmokeResult<()> {
    podman_allocation::run().await
}

/// Save the actual production transport dispatch/lease/receipt. In particular,
/// a successful submit is durably recorded before wait begins. On uncertain
/// submission the production .intents evidence stays in local_root as well.
struct RecordingTransport {
    inner: SlurmTaskTransport,
    root: PathBuf,
}

fn evidence_error(error: impl std::fmt::Display) -> DagError {
    DagError::Schedule(format!("cannot persist smoke evidence: {error}"))
}

#[async_trait::async_trait]
impl TaskTransport for RecordingTransport {
    async fn submit(
        &self,
        dispatch: TaskDispatch,
        artifacts: Vec<FileRef>,
    ) -> Result<TaskLease, DagError> {
        private_json(&self.root.join("DISPATCH.json"), &dispatch).map_err(evidence_error)?;
        let lease = self.inner.submit(dispatch, artifacts).await?;
        private_json(&self.root.join("LEASE.json"), &lease).map_err(evidence_error)?;
        Ok(lease)
    }
    async fn wait(&self, lease: &TaskLease) -> Result<TaskAttemptReceipt, DagError> {
        let receipt = self.inner.wait(lease).await?;
        private_json(&self.root.join("REMOTE_RECEIPT.json"), &receipt).map_err(evidence_error)?;
        Ok(receipt)
    }
    async fn cancel(&self, lease: &TaskLease) -> Result<(), DagError> {
        self.inner.cancel(lease).await
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

async fn capture(ssh: &SshTarget, command: &str, destination: &Path) -> SmokeResult<Value> {
    let output = ssh.run(command).await?;
    let evidence = json!({
        "queue_absent": dag_core::dag::execution::slurm_queue_absent(&output),
        "command": command, "exit_code": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    });
    private_json(destination, &evidence)?;
    Ok(evidence)
}

/// Independent read-only queue/accounting/control proof; no remote or local
/// deletion occurs even after this proof succeeds. Missing/ambiguous evidence
/// is a failed smoke, never an inferred successful exit.
async fn stopped_proof(ssh: &SshTarget, root: &Path, lease: &TaskLease) -> SmokeResult<()> {
    let job = &lease.lease_id;
    if job.is_empty() || !job.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid recorded Slurm job ID".into());
    }
    let queue = capture(
        ssh,
        &format!("LC_ALL=C squeue -h -j {job} -o %T"),
        &root.join("SQUEUE.json"),
    )
    .await?;
    let accounting = capture(
        ssh,
        &format!("sacct -n -P -j {job} --format=JobIDRaw,State,ExitCode,End"),
        &root.join("SACCT.json"),
    )
    .await?;
    let control = capture(
        ssh,
        &format!("scontrol show job {job}"),
        &root.join("SCONTROL.json"),
    )
    .await?;
    let mut valid = [&accounting, &control]
        .iter()
        .all(|entry| entry["exit_code"] == 0)
        && queue["queue_absent"] == true;
    let mut allocations = 0;
    let mut rows = 0;
    for line in accounting["stdout"]
        .as_str()
        .unwrap_or("")
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        let fields = line.trim().split('|').collect::<Vec<_>>();
        rows += 1;
        if fields.len() < 4 {
            valid = false;
            continue;
        }
        if fields[0] == job {
            allocations += 1;
        }
        valid &= (fields[0] == job || fields[0].starts_with(&format!("{job}.")))
            && fields[1] == "COMPLETED"
            && fields[2] == "0:0"
            && !fields[3].is_empty()
            && !["Unknown", "None", "N/A"].contains(&fields[3]);
    }
    valid &= allocations == 1 && rows >= 1;
    let fields = control["stdout"]
        .as_str()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>();
    valid &= fields.contains(&format!("JobId={job}").as_str())
        && fields.contains(&"JobState=COMPLETED")
        && fields.contains(&"ExitCode=0:0");
    private_json(
        &root.join("STOP_PROOF.json"),
        &json!({
            "verified": valid, "job_id": job, "remote_scratch_retained": true,
            "basis": ["SQUEUE.json", "SACCT.json", "SCONTROL.json"],
        }),
    )?;
    if !valid {
        return Err(
            "independent stopped-worker proof is incomplete; all scratch is retained".into(),
        );
    }
    Ok(())
}

async fn prepare_smoke(
    profile: &Profile,
    task_kind: &str,
) -> SmokeResult<(TypedRemoteTaskExecutor, SshTarget)> {
    let root = &profile.local_root;
    let executable = std::env::current_exe()?;
    private_json(
        &root.join("START.json"),
        &json!({
            "protocol": 1, "task_kind": task_kind, "source_revision": dag_core::source_revision(),
            "test_executable_sha256": digest(&executable)?,
            "worker": profile.worker, "remote_scratch_root": profile.remote_scratch_root,
            "account": profile.account, "partition": profile.partition,
            "resources": {"cpus":1,"memory_bytes":1073741824u64,"max_duration_ms":120000},
            "science": false, "cleanup": "retain local evidence and remote scratch",
        }),
    )?;
    for name in ["coordinator", "artifacts"] {
        let directory = root.join(name);
        std::fs::create_dir(&directory)?;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let ssh = SshTarget::new(&profile.ssh_program, &profile.ssh_host)
        .with_command_timeout(Duration::from_secs(60));
    let identity = capture(
        &ssh,
        &format!(
            "{} task-worker --identity",
            quote(&profile.worker.executable.to_string_lossy())
        ),
        &root.join("WORKER_IDENTITY.json"),
    )
    .await?;
    if identity["exit_code"] != 0 {
        return Err("worker identity command failed before submission".into());
    }
    let identity: Value = serde_json::from_str(
        identity["stdout"]
            .as_str()
            .ok_or("missing identity stdout")?,
    )?;
    if identity["protocol"] != 1
        || identity["source_revision"] != profile.worker.expected_source_revision
        || !identity["executable_sha256"]
            .as_str()
            .is_some_and(|hash| hash.eq_ignore_ascii_case(&profile.worker.executable_sha256))
    {
        return Err("deployed worker identity differs from explicit source/executable pins".into());
    }
    let transport = Arc::new(RecordingTransport {
        inner: SlurmTaskTransport::new(SlurmSettings {
            apptainer: None,
            ssh: ssh.clone(),
            scratch_dir: profile.remote_scratch_root.clone(),
            local_artifact_root: root.join("artifacts"),
            partition: profile.partition.clone(),
            account: Some(profile.account.clone()),
            gpus: None,
            qos: None,
            mem_per_cpu: "1G".into(),
            default_time: "00:02:00".into(),
            poll_interval: Duration::from_secs(2),
        }),
        root: root.clone(),
    });
    let store = Arc::new(SshArtifactStore::new(
        ssh.clone(),
        &profile.remote_scratch_root,
        root.join("artifacts"),
    ));
    let remote = Arc::new(RemoteTaskExecutor::new(
        transport,
        store,
        root.join("coordinator"),
    ));
    let executor = TypedRemoteTaskExecutor::new(remote, profile.worker.clone())?;
    Ok((executor, ssh))
}

#[tokio::test]
#[ignore = "submits one real synthetic Slurm job; requires explicit private profile and durable roots"]
async fn real_cluster_typed_channel_roundtrip() -> SmokeResult<()> {
    let profile = read_profile()?;
    let root = &profile.local_root;
    let (executor, ssh) = prepare_smoke(&profile, "core/channel").await?;
    let items = vec![json!({"synthetic":true,"ordinal":0}), json!(7), Value::Null];
    let operator = ChannelOperator::OfItems {
        items: items.clone(),
    };
    let execution = TaskExecution {
        node: Box::new(ChannelNode::new(operator.clone())),
        submission: TaskSubmission {
            spec: TaskSpec {
                id: "synthetic-typed-channel".into(),
                logical_node: None,
                kind: "core/channel".into(),
                spec: serde_json::to_value(&operator)?,
                axis: None,
                item_key: None,
                item: None,
                inputs: vec![],
                outputs: vec![TaskOutputBinding {
                    name: "synthetic_items".into(),
                    port: 0,
                    payload: "channel".into(),
                    artifact_paths: vec![],
                }],
            },
            inputs: vec![],
            resources: TaskResources {
                cpus: Some(1),
                memory_bytes: Some(1 << 30),
                max_duration_ms: Some(120_000),
            },
        },
        engine_ctx: Arc::new(NodeCtx::new(SessionContext::new().runtime_env(), None)),
        reporter: NodeReporter::noop(),
        cancellation: CancellationToken::new(),
    };
    let result = executor.run(execution).await;
    private_json(
        &root.join("RESULT.json"),
        &match &result {
            JobResult::Success { id, details, .. } => {
                json!({"status":"success", "task_id":id, "details":details})
            }
            JobResult::Failed {
                id, error, details, ..
            } => {
                json!({"status":"failed", "task_id":id, "error":error.to_string(), "details":details})
            }
        },
    )?;
    // Attempt the independent proof even if result transfer or node execution
    // failed; never delete scratch and never resubmit an uncertain attempt.
    let lease: TaskLease = serde_json::from_slice(&std::fs::read(root.join("LEASE.json"))?)?;
    let stop = stopped_proof(&ssh, root, &lease).await;
    let JobResult::Success {
        outputs, details, ..
    } = result
    else {
        return Err(
            "real typed worker task failed; inspect retained RESULT/intent evidence".into(),
        );
    };
    stop?;
    let Some(NodeValue::Channel(channel)) = outputs.get(&0) else {
        return Err("typed worker did not return a Channel on port zero".into());
    };
    if channel.items != items {
        return Err("Channel values changed across the real Slurm roundtrip".into());
    }
    let details = details.ok_or("successful execution lacks run details")?;
    if details.exit_code != Some(0) {
        return Err("successful execution lacks exit-code proof".into());
    }
    let manifest = details
        .task_manifest
        .ok_or("missing downloaded task manifest")?;
    let dispatch: TaskDispatch = serde_json::from_slice(&std::fs::read(&manifest.path)?)?;
    let envelope = &dispatch.spec.spec["typed_dispatch"];
    if dispatch.spec.kind != "process"
        || envelope["task"]["kind"] != "core/channel"
        || envelope["task"]["inputs"] != json!([])
        || dispatch.spec.spec["_autonomics_input_artifacts"] != json!({})
        || envelope["worker"] != serde_json::to_value(&profile.worker)?
    {
        return Err(
            "durable remote manifest differs from the zero-input typed/pinned contract".into(),
        );
    }
    let receipt: TaskAttemptReceipt =
        serde_json::from_slice(&std::fs::read(root.join("REMOTE_RECEIPT.json"))?)?;
    if receipt.status != "success"
        || receipt.executor != "slurm"
        || receipt.exit_code != 0
        || receipt.task_id != "synthetic-typed-channel"
        || receipt
            .output_artifacts_by_port
            .get(&0)
            .is_none_or(|files| files.len() != 1)
    {
        return Err("successful real Slurm receipt is incomplete".into());
    }
    let files = details
        .output_artifacts_by_port
        .get(&0)
        .ok_or("missing returned JSON artifact")?;
    if files.len() != 1
        || serde_json::from_slice::<Vec<Value>>(&std::fs::read(&files[0].path)?)? != items
    {
        return Err("returned output JSON differs from synthetic Channel payload".into());
    }
    private_json(
        &root.join("PASS.json"),
        &json!({
            "technical_smoke": "passed", "job_id": lease.lease_id,
            "source_revision": dag_core::source_revision(), "worker_executable_sha256": profile.worker.executable_sha256,
            "channel_items": items, "zero_input_artifacts": true, "independent_stop_proof": "STOP_PROOF.json",
            "remote_scratch_retained": true, "scientific_acceptance": false,
        }),
    )?;
    Ok(())
}

fn checked_rows(batches: &[RecordBatch], schema: &Schema) -> SmokeResult<Vec<(i64, String)>> {
    let mut rows = Vec::new();
    for batch in batches {
        if batch.schema().as_ref() != schema || batch.num_columns() != 2 {
            return Err("synthetic Arrow schema changed across transport".into());
        }
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or("synthetic id column is not Int64")?;
        let labels = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or("synthetic label column is not Utf8")?;
        if ids.null_count() != 0 || labels.null_count() != 0 {
            return Err("synthetic non-null values became null".into());
        }
        rows.extend(
            (0..batch.num_rows()).map(|row| (ids.value(row), labels.value(row).to_owned())),
        );
    }
    rows.sort();
    Ok(rows)
}

fn arrow_rows(path: &Path, schema: &Schema) -> SmokeResult<Vec<(i64, String)>> {
    let reader = FileReader::try_new(std::fs::File::open(path)?, None)?;
    if reader.schema().as_ref() != schema {
        return Err("synthetic Arrow footer schema changed".into());
    }
    let batches = reader.collect::<Result<Vec<_>, _>>()?;
    checked_rows(&batches, schema)
}

fn artifact_hash(artifact: &FileRef) -> SmokeResult<&str> {
    let fingerprint = artifact
        .fingerprint
        .as_ref()
        .ok_or("artifact fingerprint missing")?;
    let hash = fingerprint
        .content_hash
        .as_deref()
        .ok_or("artifact SHA256 missing")?;
    let hash = hash.strip_prefix("sha256:").unwrap_or(hash);
    if fingerprint.size == 0 || hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("artifact SHA256 or nonempty byte size invalid".into());
    }
    Ok(hash)
}

/// Inspect the actual retained private snapshots after independent job stop.
/// The metadata bridge deliberately supports the site's Python 3.6 login.
async fn snapshot_proof(
    ssh: &SshTarget,
    root: &Path,
    receipt: &TaskAttemptReceipt,
    artifacts: &BTreeMap<String, Vec<FileRef>>,
) -> SmokeResult<Value> {
    let script = r#"import hashlib,json,os,pathlib,stat,sys
workspace=pathlib.Path(sys.argv[1]);expected=json.loads(sys.argv[2])
assert workspace.is_absolute() and workspace.resolve()==workspace
root=workspace/'typed-inputs';owner=root.lstat().st_uid
assert stat.S_ISDIR(root.lstat().st_mode) and stat.S_IMODE(root.lstat().st_mode)==0o700
records=[]
for binding_index,(name,files) in enumerate(sorted(expected.items())):
 assert len(files)==1
 source=pathlib.Path(files[0]['path']);fingerprint=files[0]['fingerprint']
 parent=root/('input-%d-0'%binding_index)
 assert stat.S_ISDIR(parent.lstat().st_mode) and stat.S_IMODE(parent.lstat().st_mode)==0o700
 path=parent/source.name
 fd=os.open(str(path),os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
 try:
  before=os.fstat(fd)
  assert stat.S_ISREG(before.st_mode) and stat.S_IMODE(before.st_mode)==0o400 and before.st_uid==owner and before.st_nlink==1
  assert before.st_size==fingerprint['size'] and 0<before.st_size<65536
  digest=hashlib.sha256();count=0
  while True:
   chunk=os.read(fd,65536)
   if not chunk:break
   count+=len(chunk);assert count<=fingerprint['size'];digest.update(chunk)
  def identity(s):return (s.st_dev,s.st_ino,s.st_size,s.st_mtime_ns,s.st_ctime_ns)
  assert identity(before)==identity(os.fstat(fd))==identity(path.lstat())
  expected_hash=fingerprint['content_hash'];expected_hash=expected_hash[7:] if expected_hash.startswith('sha256:') else expected_hash
  assert digest.hexdigest().lower()==expected_hash.lower() and count==fingerprint['size']
  original=source.lstat();assert (original.st_dev,original.st_ino)!=(before.st_dev,before.st_ino)
  records.append({'name':name,'path':str(path),'source':str(source),'bytes':count,'sha256':digest.hexdigest(),'mode':stat.S_IMODE(before.st_mode)})
 finally:os.close(fd)
assert len(records)==2
print(json.dumps({'private_snapshots_verified':True,'files':records}))
"#;
    let observation = capture(
        ssh,
        &format!(
            "python3 -c {} {} {}",
            quote(script),
            quote(&receipt.workspace),
            quote(&serde_json::to_string(artifacts)?)
        ),
        &root.join("PRIVATE_INPUT_SNAPSHOTS.json"),
    )
    .await?;
    if observation["exit_code"] != 0 {
        return Err("private worker snapshot identity/hash verification failed".into());
    }
    let proof: Value = serde_json::from_str(
        observation["stdout"]
            .as_str()
            .ok_or("missing snapshot proof stdout")?,
    )?;
    if proof["private_snapshots_verified"] != true
        || proof["files"]
            .as_array()
            .is_none_or(|files| files.len() != 2)
    {
        return Err("private worker snapshot proof is incomplete".into());
    }
    Ok(proof)
}

#[tokio::test]
#[ignore = "submits one real synthetic DataFrame Slurm job; requires its own explicit private profile and durable roots"]
async fn real_cluster_typed_dataframe_gather_roundtrip() -> SmokeResult<()> {
    let profile = read_profile()?;
    let root = &profile.local_root;
    let (executor, ssh) = prepare_smoke(&profile, "logical_gather").await?;
    let session = SessionContext::new();
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("label", DataType::Utf8, false),
    ]));
    let expected = [
        vec![
            (1, "left-a".to_owned()),
            (2, "left-b".to_owned()),
            (3, "left-c".to_owned()),
        ],
        vec![
            (4, "right-a".to_owned()),
            (5, "right-b".to_owned()),
            (6, "right-c".to_owned()),
        ],
    ];
    let mut inputs = Vec::new();
    let mut bindings = Vec::new();
    for (port, name) in [(0u8, "left"), (1u8, "right")] {
        let values = &expected[usize::from(port)];
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(
                    values.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    values
                        .iter()
                        .map(|(_, label)| label.as_str())
                        .collect::<Vec<_>>(),
                )),
            ],
        )?;
        inputs.push(NodeInput {
            port,
            data: NodeValue::DataFrame(session.read_batch(batch)?),
        });
        bindings.push(TaskInputBinding {
            name: name.into(),
            port,
            source: TaskInputSource::UpstreamPort {
                from: format!("synthetic-{name}"),
                from_port: 0,
            },
            payload: "any".into(),
            path: None,
            fingerprint: None,
            staged_paths: vec![],
        });
    }
    let execution = TaskExecution {
        node: Box::new(dag_core::dag::GatherNode::default()),
        submission: TaskSubmission {
            spec: TaskSpec {
                id: "synthetic-typed-dataframe-gather".into(),
                logical_node: None,
                kind: "logical_gather".into(),
                spec: json!({}),
                axis: None,
                item_key: None,
                item: None,
                inputs: bindings,
                outputs: vec![TaskOutputBinding {
                    name: "gathered".into(),
                    port: 0,
                    payload: "any".into(),
                    artifact_paths: vec![],
                }],
            },
            inputs,
            resources: TaskResources {
                cpus: Some(1),
                memory_bytes: Some(1 << 30),
                max_duration_ms: Some(120_000),
            },
        },
        engine_ctx: Arc::new(NodeCtx::new(session.runtime_env(), None)),
        reporter: NodeReporter::noop(),
        cancellation: CancellationToken::new(),
    };
    let result = executor.run(execution).await;
    private_json(
        &root.join("RESULT.json"),
        &match &result {
            JobResult::Success { id, details, .. } => {
                json!({"status":"success","task_id":id,"details":details})
            }
            JobResult::Failed {
                id, error, details, ..
            } => {
                json!({"status":"failed","task_id":id,"error":error.to_string(),"details":details})
            }
        },
    )?;
    let lease: TaskLease = serde_json::from_slice(&std::fs::read(root.join("LEASE.json"))?)?;
    let stop = stopped_proof(&ssh, root, &lease).await;
    let JobResult::Success {
        outputs, details, ..
    } = result
    else {
        return Err("real typed DataFrame task failed; all intent/lease evidence retained".into());
    };
    stop?;
    let Some(NodeValue::DataFrame(frame)) = outputs.get(&0) else {
        return Err("typed gather did not return a DataFrame on port zero".into());
    };
    let all_rows = expected.iter().flatten().cloned().collect::<Vec<_>>();
    if checked_rows(&frame.clone().collect().await?, &schema)? != all_rows {
        return Err("returned DataFrame rows or values differ from all six synthetic rows".into());
    }
    let details = details.ok_or("successful DataFrame execution lacks run details")?;
    if details.exit_code != Some(0) {
        return Err("typed gather lacks exit-code proof".into());
    }
    let manifest = details
        .task_manifest
        .ok_or("missing downloaded typed gather manifest")?;
    let dispatch: TaskDispatch = serde_json::from_slice(&std::fs::read(&manifest.path)?)?;
    let envelope = &dispatch.spec.spec["typed_dispatch"];
    let typed_inputs = envelope["task"]["inputs"]
        .as_array()
        .ok_or("missing typed inputs")?;
    let artifacts: BTreeMap<String, Vec<FileRef>> =
        serde_json::from_value(dispatch.spec.spec["_autonomics_input_artifacts"].clone())?;
    if dispatch.spec.kind != "process"
        || envelope["task"]["kind"] != "logical_gather"
        || envelope["worker"] != serde_json::to_value(&profile.worker)?
        || typed_inputs.len() != 2
        || dispatch.spec.inputs.len() != 2
        || artifacts.len() != 2
        || envelope["task"]["outputs"][0]["payload"] != "dataframe"
    {
        return Err(
            "durable manifest differs from pinned two-input DataFrame gather contract".into(),
        );
    }
    for (port, name) in [(0u8, "left"), (1u8, "right")] {
        let binding = dispatch
            .spec
            .inputs
            .iter()
            .find(|binding| binding.port == port && binding.name == name)
            .ok_or("gather input binding missing")?;
        let typed = typed_inputs
            .iter()
            .find(|binding| binding["port"] == port && binding["name"] == name)
            .ok_or("typed gather input binding missing")?;
        let files = artifacts
            .get(name)
            .ok_or("transport artifact binding missing")?;
        if binding.payload != "dataframe"
            || typed["payload"] != "dataframe"
            || binding.staged_paths.len() != 1
            || files.len() != 1
        {
            return Err("each gather input must be exactly one concrete Arrow artifact".into());
        }
        let staged = Path::new(&binding.staged_paths[0]);
        if digest(staged)? != artifact_hash(&files[0])?
            || std::fs::metadata(staged)?.len() != files[0].fingerprint.as_ref().unwrap().size
            || arrow_rows(staged, &schema)? != expected[usize::from(port)]
        {
            return Err(
                "staged synthetic input bytes/schema/values differ from worker manifest identity"
                    .into(),
            );
        }
    }
    let receipt: TaskAttemptReceipt =
        serde_json::from_slice(&std::fs::read(root.join("REMOTE_RECEIPT.json"))?)?;
    if receipt.status != "success"
        || receipt.executor != "slurm"
        || receipt.exit_code != 0
        || receipt.task_id != "synthetic-typed-dataframe-gather"
    {
        return Err("successful real DataFrame Slurm receipt is incomplete".into());
    }
    if !clean_absolute(Path::new(&receipt.workspace))
        || !Path::new(&receipt.workspace).starts_with(&profile.remote_scratch_root)
    {
        return Err("recorded worker workspace is outside the enrolled technical namespace".into());
    }
    if digest(Path::new(&manifest.path))? != artifact_hash(&receipt.task_manifest)? {
        return Err("downloaded durable task manifest SHA256 differs from worker receipt".into());
    }
    let files = details
        .output_artifacts_by_port
        .get(&0)
        .ok_or("returned Arrow artifact missing")?;
    let remote_files = receipt
        .output_artifacts_by_port
        .get(&0)
        .ok_or("remote Arrow artifact missing")?;
    if files.len() != 1
        || remote_files.len() != 1
        || artifact_hash(&files[0])? != artifact_hash(&remote_files[0])?
        || digest(Path::new(&files[0].path))? != artifact_hash(&remote_files[0])?
        || arrow_rows(Path::new(&files[0].path), &schema)? != all_rows
    {
        return Err(
            "returned Arrow file hash/schema/rows/values differ from real worker output".into(),
        );
    }
    let snapshots = snapshot_proof(&ssh, root, &receipt, &artifacts).await?;
    private_json(
        &root.join("PASS.json"),
        &json!({
            "technical_smoke":"passed","job_id":lease.lease_id,"source_revision":dag_core::source_revision(),
            "worker_executable_sha256":profile.worker.executable_sha256,"input_rows_per_port":[3,3],"output_rows":6,
            "expected_rows":all_rows,"output_sha256":artifact_hash(&remote_files[0])?,
            "schema":{"id":"Int64","label":"Utf8","nullable":false},
            "typed_input_count":2,"private_snapshots":snapshots,"independent_stop_proof":"STOP_PROOF.json",
            "remote_scratch_retained":true,"scientific_acceptance":false,
        }),
    )?;
    Ok(())
}
