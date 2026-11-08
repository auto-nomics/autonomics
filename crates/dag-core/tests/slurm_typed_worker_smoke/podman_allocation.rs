//! Explicit synthetic production-plugin/typed-worker/Podman integration.
//! Two jobs: an observed container and a rejected unsupported PID request.
use super::*;
use dag_core::dag::execution::typed_worker::{
    TypedPodmanResourceControl, TypedPodmanSettings, TypedPodmanUserNamespace,
    typed_worker_tree_sha256,
};
use dag_core::fingerprint::PluginIdentity;
use dag_core::{DagNode, NodePorts, PortType};
use std::os::unix::fs::DirBuilderExt;

const POSITIVE: &str = "typed_podman_allocation_observe";
const DENIED: &str = "typed_podman_allocation_pids_denied";
const INPUT: &[u8] = b"autonomics synthetic container input v1\n";
const TEMPLATE: &str = include_str!("podman_manifest.toml");
// This entire integration target is Linux-only. EACCES may precede EROFS for
// a mode0444 input with dropped capabilities; independent ST_RDONLY evidence
// remains mandatory so permission denial alone never proves a readonly mount.
const LINUX_EACCES: i64 = 13;
const LINUX_EROFS: i64 = 30;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PodmanAllocationSettings {
    pub remote_python: String,
    pub image_reference: String,
    pub expected_host_uid: u32,
    pub podman: TypedPodmanSettings,
}

fn manifest(settings: &PodmanAllocationSettings) -> SmokeResult<String> {
    settings.podman.validate()?;
    let (name, digest) = settings
        .image_reference
        .rsplit_once("@sha256:")
        .ok_or("synthetic observer image must have an OCI SHA256 digest")?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"./_:-".contains(&b))
        || digest.len() != 64
        || !digest.bytes().all(|b| b.is_ascii_hexdigit())
        || !clean_absolute(Path::new(&settings.remote_python))
        || settings.expected_host_uid == 0
        || !matches!(
            settings.podman.resource_control,
            TypedPodmanResourceControl::SlurmAllocation { .. }
        )
        || settings.podman.user_namespace != TypedPodmanUserNamespace::SingleUidRoot
        || settings.podman.image_catalog.is_none()
    {
        return Err("synthetic Podman smoke requires explicit Slurm allocation, single-UID root and pinned image catalog".into());
    }
    Ok(TEMPLATE.replace("IMAGE_REFERENCE_PLACEHOLDER", &settings.image_reference))
}

#[derive(Clone)]
struct RemoteContainerDescriptor {
    kind: &'static str,
    ports: NodePorts,
    identity: PluginIdentity,
}

#[async_trait::async_trait]
impl DagNode for RemoteContainerDescriptor {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn ports(&self) -> &NodePorts {
        &self.ports
    }
    fn plugin_identity(&self) -> Option<&PluginIdentity> {
        Some(&self.identity)
    }
    fn task_resources(&self) -> Result<TaskResources, DagError> {
        Ok(resources())
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        _: &NodeCtx,
        _: &[NodeInput],
        _: &NodeReporter,
    ) -> Result<dag_core::dag::graph::PortOutputs, DagError> {
        Err(DagError::Schedule(
            "container smoke descriptor must never execute locally".into(),
        ))
    }
}

fn resources() -> TaskResources {
    TaskResources {
        cpus: Some(1),
        memory_bytes: Some(1 << 30),
        max_duration_ms: Some(120_000),
    }
}

fn private_dir(path: &Path) -> SmokeResult<()> {
    std::fs::DirBuilder::new().mode(0o700).create(path)?;
    Ok(())
}

fn write_bytes(path: &Path, bytes: &[u8]) -> SmokeResult<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

async fn python(
    ssh: &SshTarget,
    executable: &str,
    code: &str,
    evidence: &Path,
) -> SmokeResult<Value> {
    let result = capture(
        ssh,
        &format!(
            "{} -B - <<'AUTONOMICS_PODMAN_SMOKE_PY'\n{code}\nAUTONOMICS_PODMAN_SMOKE_PY",
            quote(executable)
        ),
        evidence,
    )
    .await?;
    if result["exit_code"] != 0 {
        return Err("synthetic Podman helper failed; preserve all evidence".into());
    }
    Ok(serde_json::from_str(
        result["stdout"].as_str().ok_or("missing helper stdout")?,
    )?)
}

async fn provision(
    profile: &Profile,
    settings: &PodmanAllocationSettings,
    fixture: &str,
    source: &str,
) -> SmokeResult<()> {
    let payload = serde_json::to_string(
        &json!({"root":fixture,"manifest":source,"uid":settings.expected_host_uid}),
    )?;
    let code = format!(
        r#"import os,stat,json,hashlib
p=json.loads({literal})
parts=p['root'].split('/')[1:]
assert len(parts)>2 and all(x not in ('','.','..') for x in parts)
assert os.geteuid()==p['uid'] and os.getegid()==p['uid']
fd=os.open('/',os.O_PATH|os.O_DIRECTORY)
for part in parts[:-1]:
 n=os.open(part,os.O_PATH|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=fd);os.close(fd);fd=n
os.mkdir(parts[-1],0o700,dir_fd=fd)
n=os.open(parts[-1],os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=fd);os.close(fd);fd=n
s=os.fstat(fd);assert s.st_uid==p['uid'] and stat.S_IMODE(s.st_mode)==0o700
for name in ('plugins','runs'):os.mkdir(name,0o700,dir_fd=fd)
plugins=os.open('plugins',os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=fd)
os.mkdir('hpc_probe',0o700,dir_fd=plugins)
plugin=os.open('hpc_probe',os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=plugins)
out=os.open('manifest.toml',os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600,dir_fd=plugin)
with os.fdopen(out,'wb') as stream:stream.write(p['manifest'].encode());stream.flush();os.fsync(stream.fileno())
for item in (plugin,plugins,fd):os.fsync(item);os.close(item)
print(json.dumps({{'root':p['root'],'uid':s.st_uid,'gid':s.st_gid,'inode':s.st_ino,'device':s.st_dev,'manifest_sha256':hashlib.sha256(p['manifest'].encode()).hexdigest()}}))
"#,
        literal = serde_json::to_string(&payload)?
    );
    let ssh = SshTarget::new(&profile.ssh_program, &profile.ssh_host)
        .with_command_timeout(Duration::from_secs(60));
    let value = python(
        &ssh,
        &settings.remote_python,
        &code,
        &profile.local_root.join("PROVISION.json"),
    )
    .await?;
    if value["root"] != fixture
        || value["uid"] != settings.expected_host_uid
        || value["manifest_sha256"] != format!("{:x}", Sha256::digest(source.as_bytes()))
    {
        return Err("synthetic plugin provisioning identity differs".into());
    }
    Ok(())
}

fn task(kind: &'static str, identity: &PluginIdentity, input: FileRef) -> TaskExecution {
    let session = SessionContext::new();
    TaskExecution {
        node: Box::new(RemoteContainerDescriptor {
            kind,
            identity: identity.clone(),
            ports: NodePorts::new()
                .add_input_port_of_type(None, PortType::File)
                .add_output_port_of_type(None, PortType::File),
        }),
        submission: TaskSubmission {
            spec: TaskSpec {
                id: kind.into(),
                kind: format!("hpc_probe/{kind}"),
                spec: json!({}),
                logical_node: None,
                axis: None,
                item_key: None,
                item: None,
                inputs: vec![TaskInputBinding {
                    name: "input".into(),
                    port: 0,
                    source: TaskInputSource::UpstreamPort {
                        from: "synthetic-fixture".into(),
                        from_port: 0,
                    },
                    payload: "file".into(),
                    path: None,
                    fingerprint: None,
                    staged_paths: vec![],
                }],
                outputs: vec![TaskOutputBinding {
                    name: "proof".into(),
                    port: 0,
                    payload: "file".into(),
                    artifact_paths: vec![],
                }],
            },
            inputs: vec![NodeInput::file(0, input)],
            resources: resources(),
        },
        engine_ctx: Arc::new(NodeCtx::new(session.runtime_env(), None)),
        reporter: NodeReporter::noop(),
        cancellation: CancellationToken::new(),
    }
}

fn validate_observation(value: &Value, expected_uid: u32) -> SmokeResult<()> {
    if value["schema"] != "AUTONOMICS_TECHNICAL_CONTAINER_OBSERVATION_V1"
        || value["mode"] != "observe"
        || value["technical_smoke_only"] != true
        || ["uid", "gid", "euid", "egid"]
            .iter()
            .any(|key| value[*key] != 0)
        || [
            "rootfs_readonly",
            "input_readonly",
            "network_loopback_only",
            "no_new_privileges",
        ]
        .iter()
        .any(|key| value[*key] != true)
        || value["rootfs_write_errno"] != LINUX_EROFS
        || !matches!(
            value["input_write_errno"].as_i64(),
            Some(LINUX_EACCES | LINUX_EROFS)
        )
        || value["network_interfaces"] != json!(["lo"])
        || value["affinity_cpus"]
            .as_array()
            .is_none_or(|cpus| cpus.is_empty() || cpus.iter().any(|cpu| cpu.as_u64().is_none()))
    {
        return Err(
            "real observer did not confirm the admitted UID, CPU, readonly and network contract"
                .into(),
        );
    }
    for key in ["uid_map", "gid_map"] {
        let map = value[key].as_str().ok_or("observer mapping missing")?;
        let rows = map
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        if rows.len() != 1
            || rows[0]
                .split_whitespace()
                .map(str::parse::<u32>)
                .collect::<Result<Vec<_>, _>>()?
                != vec![0, expected_uid, 1]
        {
            return Err(
                "observer did not confirm single-UID root maps to the admitted operator".into(),
            );
        }
    }
    Ok(())
}

fn membership_map(raw: &str) -> SmokeResult<BTreeMap<String, String>> {
    let mut members = BTreeMap::new();
    for line in raw.lines().filter(|line| !line.is_empty()) {
        let fields = line.splitn(3, ':').collect::<Vec<_>>();
        if fields.len() != 3
            || fields[0].parse::<u32>().is_err()
            || !clean_absolute(Path::new(fields[2]))
        {
            return Err("malformed observed cgroup membership".into());
        }
        if fields[1].is_empty() {
            continue;
        }
        for controller in fields[1].split(',') {
            if controller.is_empty()
                || members
                    .insert(controller.into(), fields[2].into())
                    .is_some()
            {
                return Err("duplicate observed cgroup controller".into());
            }
        }
    }
    Ok(members)
}

fn validate_container_membership(
    evidence: &Value,
    observation: &Value,
    job: &str,
    uid: u32,
) -> SmokeResult<()> {
    let step = evidence["step_id"].as_str().ok_or("worker step absent")?;
    let canonical_numeric = !step.is_empty()
        && step.bytes().all(|byte| byte.is_ascii_digit())
        && (step == "0" || !step.starts_with('0'));
    if step != "batch" && !canonical_numeric {
        return Err("worker step is neither batch nor canonical numeric".into());
    }
    let environment = &evidence["raw_environment"];
    if environment.as_object().is_none_or(|values| {
        !values.contains_key("SLURM_STEP_ID") || !values.contains_key("SLURM_STEPID")
    }) || environment["SLURM_JOB_ID"] != job
        || (!environment["SLURM_JOBID"].is_null() && environment["SLURM_JOBID"] != job)
    {
        return Err(
            "worker Slurm environment evidence is incomplete or belongs to another job".into(),
        );
    }
    match evidence["step_identity_source"].as_str() {
        Some("kernel_batch_membership")
            if step == "batch"
                && environment["SLURM_STEP_ID"].is_null()
                && environment["SLURM_STEPID"].is_null() => {}
        Some("slurm_environment")
            if ["SLURM_STEP_ID", "SLURM_STEPID"]
                .iter()
                .any(|key| environment[*key].as_str() == Some(step))
                && ["SLURM_STEP_ID", "SLURM_STEPID"].iter().all(|key| {
                    environment[*key].is_null() || environment[*key].as_str() == Some(step)
                }) => {}
        _ => return Err("worker step is not bound to its actual environment/kernel source".into()),
    }
    let worker = membership_map(
        evidence["raw_cgroup"]
            .as_str()
            .ok_or("worker cgroup absent")?,
    )?;
    let container = membership_map(
        observation["cgroup"]
            .as_str()
            .ok_or("container cgroup absent")?,
    )?;
    let anchor = format!("/uid_{uid}/job_{job}/step_{step}");
    for controller in ["cpu", "memory", "cpuset"] {
        let path = worker
            .get(controller)
            .ok_or("worker resource controller absent")?;
        let suffix = path
            .split_once(&anchor)
            .map(|(_, suffix)| suffix)
            .ok_or("worker hierarchy lacks exact job/UID/step anchor")?;
        if !(suffix.is_empty() || suffix.starts_with('/')) {
            return Err("worker resource anchor is a partial match".into());
        }
    }
    // Explicit --cgroupns=host exposes the same hierarchy identities. Every
    // container controller must remain at/below its actual worker membership;
    // a shared PID membership is only a custody match, never a per-job cap.
    for (controller, path) in &worker {
        let actual = container
            .get(controller)
            .ok_or("container lost a worker controller")?;
        if !Path::new(actual).starts_with(Path::new(path)) {
            return Err("container escaped the kernel-verified worker cgroup".into());
        }
    }
    Ok(())
}

fn validate_backend_records(
    proof: &Value,
    podman: &TypedPodmanSettings,
    workspace: &str,
    uid: u32,
) -> SmokeResult<()> {
    let backend = &proof["backend"]["json"];
    let oci = &proof["oci"]["json"];
    let runtime = backend["runtime"]
        .as_str()
        .ok_or("backend runtime absent")?;
    let runtime_path = Path::new(runtime);
    let leaf = runtime_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !clean_absolute(runtime_path)
        || runtime_path.parent() != Some(Path::new("/tmp"))
        || leaf.len() != 10
        || !leaf.starts_with("apm-")
        || !leaf[4..].bytes().all(|byte| byte.is_ascii_alphanumeric())
        || backend["schema"] != "ORDINARY_PODMAN_SLURM_LOCAL_BACKEND_V1"
        || backend["workspace"]
            != json!(Path::new(workspace).join("typed-state/containers/workspace"))
        || backend["cleanup"] != "RETAINED_UNTIL_INDEPENDENT_HELPER_STOP_AND_IDENTITY_PROOF"
    {
        return Err("worker did not retain its private node-local Podman backend identity".into());
    }
    for (key, leaf) in [("storage", "storage"), ("runroot", "run")] {
        let identity = &backend[key];
        if identity["path"] != json!(runtime_path.join(leaf))
            || identity["uid"] != uid
            || identity["gid"].as_u64().is_none()
            || identity["dev"].as_u64().is_none()
            || identity["ino"].as_u64().is_none_or(|ino| ino == 0)
            || identity["mode"]
                .as_u64()
                .is_none_or(|mode| mode & 0o177777 != 0o40700)
        {
            return Err(
                "worker Podman storage/runroot ownership or sibling identity differs".into(),
            );
        }
    }
    if backend["storage"]["dev"] != backend["runroot"]["dev"]
        || backend["storage"]["ino"] == backend["runroot"]["ino"]
    {
        return Err("worker Podman storage and runroot are not distinct local directories".into());
    }
    let runtime_pin = podman
        .oci_runtime
        .as_ref()
        .ok_or("OCI runtime path unpinned")?;
    let runtime_hash = podman
        .oci_runtime_sha256
        .as_ref()
        .ok_or("OCI runtime SHA unpinned")?;
    let quoted = serde_json::to_string(runtime_pin.to_str().ok_or("OCI runtime is not UTF-8")?)?;
    let configuration = format!(
        "[engine]\nruntime=\"autonomics-crun\"\nruntime_supports_nocgroup=[\"autonomics-crun\"]\nruntime_supports_json=[\"autonomics-crun\"]\n[engine.runtimes]\nautonomics-crun=[{quoted}]\n"
    );
    if oci["schema"] != "ORDINARY_PODMAN_OCI_RUNTIME_V1"
        || oci["path"] != json!(runtime_pin)
        || oci["sha256"].as_str() != Some(runtime_hash.as_str())
        || oci["engine_name"] != "autonomics-crun"
        || oci["config_path"] != json!(runtime_path.join("config/containers/containers.conf"))
        || oci["config_sha256"] != format!("{:x}", Sha256::digest(configuration.as_bytes()))
        || oci["container_execution_proven"] != false
        || oci["dev"].as_u64().is_none()
        || oci["ino"].as_u64().is_none_or(|ino| ino == 0)
        || oci["uid"]
            .as_u64()
            .is_none_or(|owner| owner != 0 && owner != u64::from(uid))
        || oci["mode"].as_u64().is_none_or(|mode| {
            mode & 0o170000 != 0o100000 || mode & 0o111 == 0 || mode & 0o022 != 0
        })
    {
        return Err("worker OCI runtime/config identity differs from the operator pin".into());
    }
    Ok(())
}

async fn resource_proof(
    ssh: &SshTarget,
    settings: &PodmanAllocationSettings,
    root: &Path,
    receipt: &TaskAttemptReceipt,
    lease: &TaskLease,
    observation: &Value,
) -> SmokeResult<Value> {
    let code = format!(
        r#"import os,stat,json,hashlib
root={workspace};fd=os.open('/',os.O_PATH|os.O_DIRECTORY)
for part in root.split('/')[1:]:
 n=os.open(part,os.O_PATH|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=fd);os.close(fd);fd=n
def descend(parent,path):
 current=os.dup(parent)
 for part in path.split('/'):
  n=os.open(part,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=current);os.close(current);current=n
 return current
def read(parent,name):
 f=os.open(name,os.O_RDONLY|os.O_NONBLOCK|os.O_NOFOLLOW,dir_fd=parent)
 s=os.fstat(f);assert stat.S_ISREG(s.st_mode) and s.st_uid=={uid} and stat.S_IMODE(s.st_mode)==0o600 and s.st_nlink==1 and s.st_size<=1048576
 with os.fdopen(f,'rb') as stream:
  raw=stream.read(1048577);after=os.fstat(stream.fileno())
 named=os.stat(name,dir_fd=parent,follow_symlinks=False)
 stamp=lambda x:(x.st_dev,x.st_ino,x.st_size,x.st_mode,x.st_uid,x.st_gid,x.st_nlink,x.st_mtime_ns,x.st_ctime_ns)
 assert stamp(s)==stamp(after)==stamp(named)
 return {{'json':json.loads(raw),'sha256':hashlib.sha256(raw).hexdigest(),'bytes':len(raw),'inode':s.st_ino}}
state=descend(fd,'typed-state');work=descend(state,'containers/workspace')
names=[name for name in os.listdir(work) if name.endswith('.slurm-allocation.json')]
assert len(names)==1
result={{'run':read(work,names[0]),'preparation':read(state,'slurm-allocation-preparation.json'),'run_filename':names[0],'backend':read(state,'podman-local-backend.json'),'oci':read(state,'podman-oci-runtime.json')}}
for item in (work,state,fd):os.close(item)
print(json.dumps(result))
"#,
        workspace = json!(receipt.workspace),
        uid = settings.expected_host_uid
    );
    let proof = python(
        ssh,
        &settings.remote_python,
        &code,
        &root.join("RESOURCE_ADMISSION.json"),
    )
    .await?;
    validate_backend_records(
        &proof,
        &settings.podman,
        &receipt.workspace,
        settings.expected_host_uid,
    )?;
    let run = &proof["run"]["json"];
    let preparation = &proof["preparation"]["json"];
    let evidence = &run["evidence"];
    let requested = &evidence["validated_request"];
    if run["schema"] != "SLURM_ALLOCATION_RUN_ADMISSION_V1"
        || run["resource_control"] != "slurm_allocation"
        || run["user_namespace"] != "single_uid_root"
        || run["container_execution_proven"] != false
        || run["request"]["cpus"].as_f64() != Some(1.0)
        || run["request"]["memory"] != "1GiB"
        || run["request"]["pids_limit"] != Value::Null
        || run["request"]["timeout_secs"] != 120
        || evidence["schema"] != "SLURM_ALLOCATION_V1"
        || evidence["cgroup_version"] != 1
        || evidence["job_id"] != lease.lease_id
        || evidence["uid"] != settings.expected_host_uid
        || evidence["cpu_cap_numerator"]
            .as_u64()
            .is_none_or(|cap| cap == 0)
        || evidence["cpu_cap_denominator"]
            .as_u64()
            .is_none_or(|cap| cap == 0)
        || evidence["cpu_cap_numerator"].as_u64() > evidence["cpu_cap_denominator"].as_u64()
        || evidence["memory_cap_bytes"]
            .as_u64()
            .is_none_or(|cap| cap == 0 || cap > (1 << 30))
        || requested["cpus"].as_f64() != Some(1.0)
        || requested["memory_bytes"] != (1u64 << 30)
        || requested["pids_limit"] != Value::Null
        || preparation["schema"] != "SLURM_ALLOCATION_PREPARATION_V1"
        || preparation["request_admitted"] != false
        || preparation["container_execution_proven"] != false
        || preparation["evidence"]["job_id"] != lease.lease_id
        || preparation["evidence"]["step_id"] != evidence["step_id"]
        || preparation["evidence"]["uid"] != settings.expected_host_uid
    {
        return Err(
            "actual worker resource admission did not confirm the exact Slurm/node budget".into(),
        );
    }
    validate_container_membership(
        evidence,
        observation,
        &lease.lease_id,
        settings.expected_host_uid,
    )?;
    let expected_swap = match settings.podman.resource_control {
        TypedPodmanResourceControl::SlurmAllocation { swap_policy } => {
            serde_json::to_value(swap_policy)?
        }
        _ => return Err("non-Slurm smoke policy".into()),
    };
    if run["swap_policy"] != expected_swap || requested["swap_policy"] != expected_swap {
        return Err("actual worker swap policy differs from explicit operator policy".into());
    }
    if requested["memsw_not_greater_than_ram"] != (expected_swap == "disabled") {
        return Err("swap inheritance was confused with an admitted disabled-swap policy".into());
    }
    let mut allowed = std::collections::BTreeSet::new();
    for segment in evidence["effective_cpuset"]
        .as_str()
        .ok_or("worker cpuset absent")?
        .split(',')
    {
        let (first, last) = match segment.split_once('-') {
            Some((first, last)) => (first.parse::<u32>()?, last.parse::<u32>()?),
            None => {
                let cpu = segment.parse::<u32>()?;
                (cpu, cpu)
            }
        };
        if last < first || u64::from(last) - u64::from(first) > 65536 {
            return Err("unbounded worker cpuset".into());
        }
        allowed.extend(first..=last);
    }
    if observation["affinity_cpus"]
        .as_array()
        .ok_or("observer affinity absent")?
        .iter()
        .any(|cpu| {
            cpu.as_u64()
                .is_none_or(|cpu| u32::try_from(cpu).map_or(true, |cpu| !allowed.contains(&cpu)))
        })
    {
        return Err("actual container affinity escaped the verified job cpuset".into());
    }
    Ok(proof)
}

async fn plugin_readback(
    profile: &Profile,
    root: &Path,
    label: &str,
    expected: &str,
) -> SmokeResult<()> {
    let ssh = SshTarget::new(&profile.ssh_program, &profile.ssh_host)
        .with_command_timeout(Duration::from_secs(60));
    let evidence = capture(
        &ssh,
        &format!(
            "{} task-worker --identity --plugins-root {}",
            quote(&profile.worker.executable.to_string_lossy()),
            quote(
                &profile
                    .worker
                    .plugins_root
                    .as_ref()
                    .ok_or("plugin root absent")?
                    .to_string_lossy()
            )
        ),
        &root.join(label),
    )
    .await?;
    if evidence["exit_code"] != 0 {
        return Err("actual synthetic plugin identity query failed".into());
    }
    let identity: Value = serde_json::from_str(
        evidence["stdout"]
            .as_str()
            .ok_or("plugin identity stdout absent")?,
    )?;
    if identity["plugins_sha256"] != expected
        || identity["source_revision"] != profile.worker.expected_source_revision
        || identity["executable_sha256"] != profile.worker.executable_sha256
    {
        return Err("actual worker/plugin identity differs from source and byte pins".into());
    }
    Ok(())
}

async fn execute(
    profile: &Profile,
    kind: &'static str,
    identity: &PluginIdentity,
    input: FileRef,
    denied: bool,
) -> SmokeResult<(TaskAttemptReceipt, Option<Value>)> {
    let root = &profile.local_root;
    let (executor, ssh) = prepare_smoke(profile, &format!("hpc_probe/{kind}")).await?;
    let result = executor.run(task(kind, identity, input)).await;
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
    let receipt: TaskAttemptReceipt =
        serde_json::from_slice(&std::fs::read(root.join("REMOTE_RECEIPT.json"))?)?;
    // Always obtain independent job termination before assessing semantics.
    if denied {
        warehouse::denied_stop(&ssh, root, &lease, &receipt).await?;
    } else {
        stopped_proof(&ssh, root, &lease).await?;
    }
    if receipt.task_id != kind
        || receipt.executor != "slurm"
        || (!denied && (receipt.status != "success" || receipt.exit_code != 0))
        || !Path::new(&receipt.workspace).starts_with(&profile.remote_scratch_root)
    {
        return Err("container smoke receipt escaped its enrolled task/workspace".into());
    }
    let settings = profile
        .podman_allocation
        .as_ref()
        .ok_or("missing Podman settings")?;
    if denied {
        if !matches!(result, JobResult::Failed { .. }) {
            return Err("explicit PID request unexpectedly succeeded".into());
        }
        let stderr = Path::new(&receipt.workspace).join(format!("slurm-{}.err", lease.lease_id));
        let code = format!(
            "import os,stat,json\np={}\nf=os.open(p,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK);s=os.fstat(f);assert stat.S_ISREG(s.st_mode) and s.st_size<=1048576\nb=os.read(f,1048577);os.close(f);print(json.dumps({{'stderr':b.decode('utf8','replace')}}))",
            json!(stderr)
        );
        let error = python(
            &ssh,
            &settings.remote_python,
            &code,
            &root.join("WORKER_ERROR.json"),
        )
        .await?;
        let error = error["stderr"].as_str().unwrap_or("").to_ascii_lowercase();
        if !(error.contains("pid") && (error.contains("slurm") || error.contains("inherited"))) {
            return Err("negative task did not fail for explicit inherited PID admission".into());
        }
        return Ok((receipt, None));
    }
    let JobResult::Success {
        outputs, details, ..
    } = result
    else {
        return Err("real Podman worker did not succeed".into());
    };
    let details = details.ok_or("container execution details absent")?;
    let manifest = details
        .task_manifest
        .ok_or("typed container manifest absent")?;
    if digest(Path::new(&manifest.path))? != artifact_hash(&receipt.task_manifest)? {
        return Err("container task manifest hash changed".into());
    }
    let dispatch: TaskDispatch = serde_json::from_slice(&std::fs::read(&manifest.path)?)?;
    let envelope = &dispatch.spec.spec["typed_dispatch"];
    if envelope["worker"] != serde_json::to_value(&profile.worker)?
        || envelope["plugin_identity"] != serde_json::to_value(identity)?
        || envelope["task"]["kind"] != format!("hpc_probe/{kind}")
        || dispatch.spec.spec["_autonomics_input_artifacts"]["input"]
            .as_array()
            .is_none_or(|files| files.len() != 1)
    {
        return Err(
            "production worker did not retain the exact synthetic plugin/input contract".into(),
        );
    }
    let input_hash = dispatch.spec.spec["_autonomics_input_artifacts"]["input"][0]
        ["fingerprint"]["content_hash"].as_str().ok_or("input transport SHA256 absent")?;
    if input_hash.strip_prefix("sha256:").unwrap_or(input_hash)
        != format!("{:x}", Sha256::digest(INPUT))
    {
        return Err("worker input was not the frozen synthetic byte sequence".into());
    }
    let Some(NodeValue::File(file)) = outputs.get(&0) else {
        return Err("container observer did not return its JSON file".into());
    };
    if std::fs::metadata(&file.path)?.len() > 65536 {
        return Err("observer proof exceeds bounded technical payload".into());
    }
    let remote = receipt
        .output_artifacts_by_port
        .get(&0)
        .and_then(|files| {
            if files.len() == 1 {
                files.first()
            } else {
                None
            }
        })
        .ok_or("unique remote observer proof absent")?;
    if digest(Path::new(&file.path))? != artifact_hash(remote)? {
        return Err("observer output hash changed".into());
    }
    let observation: Value = serde_json::from_slice(&std::fs::read(&file.path)?)?;
    validate_observation(&observation, settings.expected_host_uid)?;
    resource_proof(&ssh, settings, root, &receipt, &lease, &observation).await?;
    private_json(&root.join("CONTAINER_OBSERVATION.json"), &observation)?;
    Ok((receipt, Some(observation)))
}

pub(super) async fn run() -> SmokeResult<()> {
    let mut profile = read_profile_for(false, true)?;
    let settings = profile
        .podman_allocation
        .clone()
        .ok_or("explicit Podman allocation settings required")?;
    let source = manifest(&settings)?;
    let fixture = Path::new(&profile.remote_scratch_root)
        .parent()
        .and_then(Path::to_str)
        .ok_or("Podman fixture parent required")?
        .to_owned();
    if Path::new(&profile.remote_scratch_root)
        .file_name()
        .and_then(|name| name.to_str())
        != Some("runs")
        || fixture == "/"
    {
        return Err("Podman scratch must select a unique fixture_root/runs".into());
    }
    let root = profile.local_root.clone();
    let local_plugins = root.join("plugins");
    private_dir(&local_plugins)?;
    private_dir(&local_plugins.join("hpc_probe"))?;
    write_bytes(
        &local_plugins.join("hpc_probe/manifest.toml"),
        source.as_bytes(),
    )?;
    let plugin_hash = typed_worker_tree_sha256(&local_plugins)?;
    let base_worker = profile.worker.clone();
    profile.worker.plugins_root = Some(Path::new(&fixture).join("plugins"));
    profile.worker.plugins_sha256 = Some(plugin_hash.clone());
    profile.worker.podman = Some(settings.podman.clone());
    let identity = PluginIdentity {
        manifest_sha256: format!("sha256:{:x}", Sha256::digest(source.as_bytes())),
        script_sha256: None,
        image_reference: settings.image_reference.clone(),
        panels: vec![],
    };
    private_json(
        &root.join("PODMAN_START.json"),
        &json!({
            "source_revision":dag_core::source_revision(),"test_executable_sha256":digest(&std::env::current_exe()?)?,
            "base_worker":base_worker,"effective_worker":profile.worker,"plugin_identity":identity,
            "fixture_root":fixture,"remote_scratch_root":profile.remote_scratch_root,
            "expected_host_uid":settings.expected_host_uid,"resources":resources(),
            "worker_factory":"production manifest loader and ContainerCommandNode",
            "coordinator_factory_tested":false,"scientific_acceptance":false,
        }),
    )?;
    provision(&profile, &settings, &fixture, &source).await?;
    plugin_readback(&profile, &root, "PLUGIN_IDENTITY_BEFORE.json", &plugin_hash).await?;
    let input_path = root.join("synthetic-input.txt");
    write_bytes(&input_path, INPUT)?;
    let input = FileRef::local(&input_path, Some("txt".into()))?;
    let mut jobs = BTreeMap::new();
    for (label, kind, denied) in [("observed", POSITIVE, false), ("pid-denied", DENIED, true)] {
        let child = root.join(label);
        private_dir(&child)?;
        profile.local_root = child;
        let (receipt, _) = execute(&profile, kind, &identity, input.clone(), denied).await?;
        let lease: TaskLease =
            serde_json::from_slice(&std::fs::read(profile.local_root.join("LEASE.json"))?)?;
        jobs.insert(label, lease.lease_id.clone());
        private_json(
            &profile.local_root.join("SEMANTIC_PASS.json"),
            &json!({"job_id":lease.lease_id,"expected_task_failure":denied,"exit_code":receipt.exit_code,"plugin_identity":identity,"independent_stop":true}),
        )?;
    }
    if jobs
        .values()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != 2
    {
        return Err("synthetic tasks did not have distinct actual leases".into());
    }
    if typed_worker_tree_sha256(&local_plugins)? != plugin_hash {
        return Err("local plugin source changed during acceptance".into());
    }
    plugin_readback(&profile, &root, "PLUGIN_IDENTITY_AFTER.json", &plugin_hash).await?;
    private_json(
        &root.join("PODMAN_PASS.json"),
        &json!({
            "technical_smoke":"passed","source_revision":dag_core::source_revision(),
            "worker_executable_sha256":profile.worker.executable_sha256,"jobs":jobs,
            "plugin_identity":identity,"image_reference":settings.image_reference,
            "resource_control":settings.podman.resource_control,"user_namespace":settings.podman.user_namespace,
            "worker_factory":"production manifest loader and ContainerCommandNode",
            "remote_scratch_retained":true,"scientific_acceptance":false,
        }),
    )?;
    Ok(())
}

#[test]
fn synthetic_backend_records_require_local_siblings_and_exact_pinned_oci_configuration() {
    let uid = 12345;
    let podman: TypedPodmanSettings = serde_json::from_value(json!({
        "executable":"/technical/podman","executable_sha256":format!("{:x}",Sha256::digest(b"toy Podman")),
        "oci_runtime":"/technical/crun","oci_runtime_sha256":format!("{:x}",Sha256::digest(b"toy crun")),
        "resource_control":{"mode":"slurm_allocation","swap_policy":"inherit"},"user_namespace":"single_uid_root"
    })).unwrap();
    let configuration = "[engine]\nruntime=\"autonomics-crun\"\nruntime_supports_nocgroup=[\"autonomics-crun\"]\nruntime_supports_json=[\"autonomics-crun\"]\n[engine.runtimes]\nautonomics-crun=[\"/technical/crun\"]\n";
    let valid = json!({"backend":{"json":{
        "schema":"ORDINARY_PODMAN_SLURM_LOCAL_BACKEND_V1","runtime":"/tmp/apm-toy123",
        "workspace":"/technical/attempt/typed-state/containers/workspace",
        "cleanup":"RETAINED_UNTIL_INDEPENDENT_HELPER_STOP_AND_IDENTITY_PROOF",
        "storage":{"path":"/tmp/apm-toy123/storage","dev":7,"ino":10,"uid":uid,"gid":uid,"mode":0o40700},
        "runroot":{"path":"/tmp/apm-toy123/run","dev":7,"ino":11,"uid":uid,"gid":uid,"mode":0o40700}
    }},"oci":{"json":{
        "schema":"ORDINARY_PODMAN_OCI_RUNTIME_V1","path":"/technical/crun","sha256":podman.oci_runtime_sha256,
        "dev":8,"ino":12,"uid":uid,"mode":0o100500,"engine_name":"autonomics-crun",
        "config_path":"/tmp/apm-toy123/config/containers/containers.conf",
        "config_sha256":format!("{:x}",Sha256::digest(configuration.as_bytes())),"container_execution_proven":false
    }}});
    validate_backend_records(&valid, &podman, "/technical/attempt", uid).unwrap();
    for (pointer, changed) in [
        ("/backend/json/runtime", json!("/shared/apm-toy123")),
        ("/backend/json/storage/mode", json!(0o40755)),
        ("/backend/json/runroot/ino", json!(10)),
        ("/backend/json/storage/uid", json!(999)),
        (
            "/backend/json/workspace",
            json!("/technical/another/typed-state/containers/workspace"),
        ),
        ("/oci/json/path", json!("/untrusted/crun")),
        (
            "/oci/json/sha256",
            json!(format!("{:x}", Sha256::digest(b"another crun"))),
        ),
        (
            "/oci/json/config_sha256",
            json!(format!("{:x}", Sha256::digest(b"changed config"))),
        ),
        (
            "/oci/json/config_path",
            json!("/tmp/apm-toy123/config/other.conf"),
        ),
        ("/oci/json/mode", json!(0o100522)),
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(pointer).unwrap() = changed;
        assert!(
            validate_backend_records(&invalid, &podman, "/technical/attempt", uid).is_err(),
            "accepted {pointer}"
        );
    }
}

#[test]
fn synthetic_observer_requires_real_readonly_namespaces_and_single_host_mapping() {
    let uid = 12345;
    let valid = json!({"schema":"AUTONOMICS_TECHNICAL_CONTAINER_OBSERVATION_V1","mode":"observe","technical_smoke_only":true,
        "uid":0,"gid":0,"euid":0,"egid":0,"uid_map":"0 12345 1\n","gid_map":"0 12345 1\n",
        "rootfs_readonly":true,"input_readonly":true,"network_loopback_only":true,"no_new_privileges":true,
        "rootfs_write_errno":LINUX_EROFS,"input_write_errno":LINUX_EROFS,"network_interfaces":["lo"],"affinity_cpus":[7]});
    validate_observation(&valid, uid).unwrap();
    let mut permission_denied = valid.clone();
    permission_denied["input_write_errno"] = json!(LINUX_EACCES);
    validate_observation(&permission_denied, uid).unwrap();
    permission_denied["input_readonly"] = json!(false);
    assert!(
        validate_observation(&permission_denied, uid).is_err(),
        "EACCES on a writable mount must not prove readonly input"
    );
    for (key, value) in [
        ("input_readonly", json!(false)),
        ("input_write_errno", json!(0)),
        ("input_write_errno", json!(1)),
        ("rootfs_write_errno", json!(LINUX_EACCES)),
        ("uid_map", json!("0 12345 65536\n")),
        ("gid_map", json!("0 99999 1\n")),
        ("network_interfaces", json!(["lo", "eth0"])),
        ("affinity_cpus", json!([])),
        ("uid", json!(12345)),
    ] {
        let mut invalid = valid.clone();
        invalid[key] = value;
        assert!(
            validate_observation(&invalid, uid).is_err(),
            "accepted changed {key}"
        );
    }
}

#[test]
fn synthetic_container_membership_requires_the_same_kernel_job_and_real_step_source() {
    let path = "/slurm/uid_12345/job_6789/step_batch/task_0";
    let cgroup = format!(
        "6:cpu,cpuacct:{path}\n10:memory:{path}\n3:cpuset:{path}\n5:pids:/system.slice/slurmd.service\n"
    );
    let mut evidence = json!({"step_id":"batch","step_identity_source":"kernel_batch_membership",
        "raw_environment":{"SLURM_JOB_ID":"6789","SLURM_JOBID":"6789","SLURM_STEP_ID":null,"SLURM_STEPID":null},"raw_cgroup":cgroup});
    let observed = json!({"cgroup":cgroup});
    validate_container_membership(&evidence, &observed, "6789", 12345).unwrap();
    for wrong in [
        cgroup.replace("job_6789", "job_67890"),
        cgroup.replace("step_batch", "step_0"),
        cgroup.replace("/system.slice/slurmd.service", "/other.service"),
        "6:cpu,cpuacct:/\n10:memory:/\n3:cpuset:/\n5:pids:/system.slice/slurmd.service\n"
            .to_string(),
        cgroup
            .lines()
            .filter(|line| !line.starts_with("10:"))
            .collect::<Vec<_>>()
            .join("\n"),
    ] {
        assert!(
            validate_container_membership(&evidence, &json!({"cgroup":wrong}), "6789", 12345)
                .is_err()
        );
    }
    evidence["raw_environment"]["SLURM_STEP_ID"] = json!("0");
    assert!(validate_container_membership(&evidence, &observed, "6789", 12345).is_err());
    evidence["step_id"] = json!("0");
    evidence["step_identity_source"] = json!("slurm_environment");
    let numeric = cgroup.replace("step_batch", "step_0");
    evidence["raw_cgroup"] = json!(numeric);
    validate_container_membership(&evidence, &json!({"cgroup":numeric}), "6789", 12345).unwrap();
    evidence["raw_environment"]["SLURM_STEPID"] = json!("1");
    assert!(
        validate_container_membership(&evidence, &json!({"cgroup":numeric}), "6789", 12345)
            .is_err()
    );
}
