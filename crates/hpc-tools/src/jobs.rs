//! Explicit, account-scoped Slurm tools. Submission intents persist separately
//! from sessions so a daemon restart cannot blindly repeat an uncertain sbatch.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::HpcClient;
use crate::monitor::{MonitorHub, Owner};
use agentik_core::storage::AgentStorage;
use agentik_core::tools::ToolContext;

pub const REMOTE_JOBS_HELPER: &str = include_str!("remote_jobs.py");

fn execution_error(message: impl Into<String>) -> ToolError {
    let message: String = message.into();
    ToolError::ExecutionFailed {
        source: message.into(),
    }
}

#[tool(
    name = "hpc_cluster",
    description = "Inspect the configured HPC account: identity, partitions, resources, accounts, quota, or own queue. Read-only queries run on the login node. A visible partition may require another submission endpoint: on SJTU, cpu requires cluster=pi, while Siyuan jobs use cluster=siyuan. Query identity/accounts at the intended endpoint. Filesystem capacity alone does not establish a user quota. GPFS reports verified user/group/fileset limits and conservative remaining bytes/files when all scope queries succeed; unknown provider results remain unverified."
)]
#[serde(deny_unknown_fields)]
pub struct ClusterInput {
    #[desc = "Operator-configured cluster: default or siyuan (default), or pi"]
    pub cluster: Option<String>,
    #[desc = "identity, partitions, resources, accounts, quota, or queue"]
    pub action: String,
    #[desc = "Optional partition filter for resources"]
    pub partition: Option<String>,
    #[desc = "Optional absolute remote filesystem path for quota queries"]
    pub path: Option<String>,
}

struct ClusterTool {
    client: Arc<HpcClient>,
}

#[async_trait]
impl ToolFunction for ClusterTool {
    type Input = ClusterInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let client = self
            .client
            .for_cluster(input.cluster.as_deref())
            .map_err(execution_error)?;
        let mut request =
            serde_json::to_value(input).map_err(|e| execution_error(e.to_string()))?;
        request
            .as_object_mut()
            .unwrap()
            .retain(|_, value| !value.is_null());
        request["tool"] = json!("cluster");
        request.as_object_mut().unwrap().remove("cluster");
        run_remote(&client, &request).await
    }
}

#[tool(
    name = "hpc_jobs",
    description = "Submit and manage Slurm jobs in the configured Unix account. Actions: submit, status, list, cancel, logs, recover, inspect, hold, release, requeue, update, watch, monitors, stop_monitor. inspect reads integrity-checked local frozen script bytes (bounded base64), request/resource hashes and metadata without SSH. recover accepts only cluster and submission_key; never supply a reconstructed script or resources. Missing remote intent remains UNKNOWN; use inspect for original local evidence. Omit unused optional fields or use JSON null, never the string \"null\". Gateway submissions automatically register a durable monitor and terminal notification to the same agent/context. watch attaches an existing submission_key without resubmitting; monitors lists only this context's records; stop_monitor stops notification/observation without cancelling the remote job. An idle conversation may still have running remote jobs. Terminal notices require logs/output/hash/QC verification and grant no scientific processing permission. On SJTU, cpu requires cluster=pi; partition visibility alone does not prove the selected endpoint can submit. New submissions first run sbatch --test-only: REJECTED means no submission was attempted; inspect current_validation.diagnostic for a required endpoint. Submit requires a stable submission_key: reuse it with identical script/resources after a timeout or daemon restart; uncertain submissions are recovered by exact durable nonce and never blindly retried. UNKNOWN recovery may include current_validation, which diagnoses today's request only and does not prove the original submission failed. Never change cluster/key to bypass UNKNOWN. OPERATOR_FOLLOWUP_COMPLETED means a separately reviewed, isolated environment diagnostic was verified against its frozen intent, script and successful stopped-job accounting. Use followup for environment evidence and stop repeating the old key; original_submission remains historical UNKNOWN, not a global workflow lock. OPERATOR_FOLLOWUP_UNVERIFIED does not establish completion. Batch script contents execute on allocated compute nodes, never the login node. cancel reports STOP_PENDING until owned-job accounting and queue evidence confirm termination. These tools do not grant scientific data or method approval."
)]
#[serde(deny_unknown_fields)]
pub struct JobsInput {
    #[desc = "Operator-configured cluster: default or siyuan (default), or pi. Reuse the same cluster with submission_key during recovery"]
    pub cluster: Option<String>,
    #[desc = "submit, status, list, cancel, logs, recover, inspect, hold, release, requeue, update, watch, monitors, or stop_monitor. inspect reads only the local frozen request; it does not contact the cluster"]
    pub action: String,
    #[desc = "Numeric Slurm job ID, or one array task ID such as 123_4"]
    pub job_id: Option<String>,
    #[desc = "Required for submit/recover/inspect. Stable caller-chosen key, 1-128 ASCII letters/digits/underscore/dash/dot; reuse for uncertain submission recovery"]
    pub submission_key: Option<String>,
    #[desc = "Complete sbatch script, including shebang; required for submit, at most 1 MiB"]
    pub script: Option<String>,
    #[desc = "Absolute existing remote working directory; required for submit"]
    pub working_directory: Option<String>,
    #[desc = "Partition; defaults to configured partition for submit"]
    pub partition: Option<String>,
    #[desc = "Project/account; defaults to configured account for submit"]
    pub account: Option<String>,
    #[desc = "Optional Slurm quality of service"]
    pub qos: Option<String>,
    #[desc = "Optional single Slurm node feature constraint"]
    pub constraint: Option<String>,
    #[desc = "Optional Slurm reservation"]
    pub reservation: Option<String>,
    #[desc = "Number of compute nodes"]
    pub nodes: Option<u32>,
    #[desc = "Number of Slurm tasks"]
    pub tasks: Option<u32>,
    #[desc = "CPU cores per task"]
    pub cpus_per_task: Option<u32>,
    #[desc = "Memory MiB per node"]
    pub memory_mb: Option<u64>,
    #[desc = "Total GPU count"]
    pub gpus: Option<u32>,
    #[desc = "Slurm time limit, e.g. 00:10:00 or 1-00:00:00; update changes only this field"]
    pub time_limit: Option<String>,
    #[desc = "Optional Slurm job array, e.g. 0-9%2"]
    pub array: Option<String>,
    #[desc = "Optional Slurm dependency, e.g. afterok:123:124"]
    pub dependency: Option<String>,
    #[desc = "logs only: stdout or stderr (default stdout)"]
    pub stream: Option<String>,
    #[desc = "logs only: explicit absolute path, useful after Slurm stops retaining a job's log paths"]
    pub path: Option<String>,
    #[desc = "logs/inspect: byte offset (default 0); inspect returns exact frozen script bytes as base64"]
    pub offset: Option<u64>,
    #[desc = "logs/inspect: bounded byte count (default 65536, maximum 1048576)"]
    pub max_bytes: Option<u32>,
    #[desc = "list only: include completed/accounted jobs, default false"]
    pub include_completed: Option<bool>,
    #[desc = "list only: UTC accounting start time YYYY-MM-DD or YYYY-MM-DDTHH:MM:SS, default seven days ago"]
    pub start_time: Option<String>,
    #[desc = "list only: maximum returned accounting allocations, default 100 and maximum 1000"]
    pub limit: Option<u32>,
}

#[derive(Clone)]
pub struct Monitoring {
    pub hub: Arc<MonitorHub>,
    pub agent_path: String,
    pub storage: Arc<dyn AgentStorage>,
}

struct JobsTool {
    client: Arc<HpcClient>,
    monitoring: Option<Monitoring>,
}

impl JobsTool {
    async fn owner(&self, ctx: &ToolContext) -> Result<Owner, ToolError> {
        let binding = self
            .monitoring
            .as_ref()
            .ok_or_else(|| execution_error("durable monitoring is not configured"))?;
        let session = ctx.session.as_ref().ok_or_else(|| {
            execution_error("durable monitoring requires the owning conversation")
        })?;
        let agent = binding
            .storage
            .get_agent_by_name(&binding.agent_path)
            .await
            .map_err(|_| execution_error("cannot resolve monitoring owner"))?
            .ok_or_else(|| execution_error("monitor owner is not registered"))?;
        if binding
            .storage
            .agent_context_id(agent.id)
            .await
            .map_err(|_| execution_error("cannot resolve monitoring context"))?
            != Some(session.session_id)
        {
            return Err(execution_error("monitor context identity mismatch"));
        }
        Ok(Owner {
            agent_id: agent.id.to_string(),
            agent_path: binding.agent_path.clone(),
            session_id: session.session_id.to_string(),
        })
    }

    async fn invoke(
        &self,
        input: JobsInput,
        ctx: Option<&ToolContext>,
    ) -> Result<ToolResult, ToolError> {
        crate::reject_placeholder_strings(
            &serde_json::to_value(&input)?,
            &[
                "cluster",
                "job_id",
                "submission_key",
                "working_directory",
                "partition",
                "account",
                "qos",
                "constraint",
                "reservation",
                "time_limit",
                "array",
                "dependency",
                "stream",
                "path",
                "start_time",
            ],
        )?;
        if matches!(input.action.as_str(), "recover" | "inspect") {
            // Recovery always uses the original local request. Silently ignoring
            // caller-supplied replacements made reconstructed scripts look frozen.
            let supplied = serde_json::to_value(&input)?;
            for (field, value) in supplied.as_object().unwrap() {
                if !(value.is_null()
                    || matches!(field.as_str(), "action" | "cluster" | "submission_key")
                    || (input.action == "inspect"
                        && matches!(field.as_str(), "offset" | "max_bytes")))
                {
                    return Err(ToolError::ValidationFailed {
                        message: format!(
                            "{} does not accept {field}; use only the original cluster and submission_key. Use inspect to read frozen evidence. No remote operation was attempted.",
                            input.action
                        ),
                    });
                }
            }
        }
        let client = self
            .client
            .for_cluster(input.cluster.as_deref())
            .map_err(execution_error)?;
        let action = input.action.clone();
        let cluster = input.cluster.clone();
        let key = input.submission_key.clone();
        if action == "inspect" {
            let key = key
                .as_deref()
                .ok_or_else(|| execution_error("inspect requires submission_key"))?;
            let record = intent(&client, key, None).map_err(execution_error)?;
            return inspect_intent(&record, input.offset, input.max_bytes);
        }
        let mut request =
            serde_json::to_value(input).map_err(|e| execution_error(e.to_string()))?;
        // Omitted options are truly absent for the remote helper, including its
        // documented defaults. JSON null must not shadow a default.
        request
            .as_object_mut()
            .unwrap()
            .retain(|_, value| !value.is_null());
        request["tool"] = json!("jobs");
        request.as_object_mut().unwrap().remove("cluster");
        if action == "submit" {
            if request.get("partition").is_none() {
                request["partition"] = json!(client.config().default_partition);
            }
            if request.get("account").is_none() {
                if let Some(account) = &client.config().default_account {
                    request["account"] = json!(account);
                }
            }
            // The same pure validator used by submit runs before binding a
            // local key or creating a monitor. It neither writes remote intent
            // files nor invokes the scheduler. Transport failure here is safe:
            // no submission has been attempted and no key has been consumed.
            let mut validation_request = request.clone();
            validation_request["action"] = json!("validate_submission_request");
            let validation = run_remote_value(&client, &validation_request).await?;
            if validation["state"] != "VALIDATED" {
                return Ok(ToolResult::error(validation.to_string()));
            }
        }
        if matches!(action.as_str(), "monitors" | "watch" | "stop_monitor") {
            let context = ctx.ok_or_else(|| {
                execution_error("monitor operations require the owning conversation")
            })?;
            let owner = self.owner(context).await?;
            let monitoring = self.monitoring.as_ref().unwrap();
            if action == "monitors" {
                let records = monitoring.hub.records().map_err(execution_error)?;
                return Ok(ToolResult::success_json(
                    json!({"monitors":records.into_iter().filter(|r|r.owner==owner).map(|r|r.view()).collect::<Vec<_>>(),"scientific_admission_granted":false}),
                ));
            }
            let key = key
                .as_deref()
                .ok_or_else(|| execution_error("watch/stop_monitor requires submission_key"))?;
            let local = intent(&client, key, None).map_err(execution_error)?;
            let mut record = monitoring
                .hub
                .prepare(
                    &client,
                    cluster.as_deref(),
                    &local,
                    owner.clone(),
                    json!(&*context.session.as_ref().unwrap().plan.load_full()),
                )
                .map_err(execution_error)?;
            if action == "stop_monitor" {
                record = monitoring
                    .hub
                    .stop(&record.id, &owner)
                    .map_err(execution_error)?;
                return Ok(ToolResult::success_json(
                    json!({"monitor":record.view(),"remote_job_cancelled":false}),
                ));
            }
            record = monitoring
                .hub
                .resume(&record.id, &owner)
                .map_err(execution_error)?;
            let recovered = run_remote_value(
                &client,
                &json!({"tool":"jobs","action":"recover","nonce":local["nonce"],"expected_request":local["request"]}),
            )
            .await?;
            record = monitoring
                .hub
                .observe_submission(&record.id, &recovered)
                .map_err(execution_error)?;
            return Ok(ToolResult::success_json(
                json!({"monitor":record.view(),"recovery":recovered,"new_job_submitted":false,"scientific_admission_granted":false}),
            ));
        }
        let mut local_intent = None;
        if matches!(action.as_str(), "submit" | "recover") {
            let key = key
                .as_deref()
                .ok_or_else(|| execution_error("submit/recover requires submission_key"))?;
            let record = intent(&client, key, (action == "submit").then_some(&request))
                .map_err(execution_error)?;
            request["nonce"] = record["nonce"].clone();
            if action == "recover" {
                request["expected_request"] = record["request"].clone();
            }
            local_intent = Some(record);
        }
        // Persist owner + exact intent BEFORE a submission can reach SSH.
        // After a lost reply/restart the monitor only recovers this same nonce.
        let prepared = if action == "submit" && self.monitoring.is_some() {
            let context = ctx.ok_or_else(|| {
                execution_error("monitored submission requires its original conversation")
            })?;
            let owner = self.owner(context).await?;
            let record = self
                .monitoring
                .as_ref()
                .unwrap()
                .hub
                .prepare(
                    &client,
                    cluster.as_deref(),
                    local_intent.as_ref().unwrap(),
                    owner,
                    json!(&*context.session.as_ref().unwrap().plan.load_full()),
                )
                .map_err(execution_error)?;
            Some(record)
        } else {
            None
        };
        let mut original = run_remote_value(&client, &request).await?;
        if action == "recover" {
            // Hashes come from the original, integrity-checked local bytes,
            // even if the remote intent never existed. They do not prove a job ran.
            original["frozen_local_request"] =
                frozen_evidence(local_intent.as_ref().unwrap()).map_err(execution_error)?;
            original["new_job_submitted"] = json!(false);
            original["submission_retried"] = json!(false);
            original["automatic_resubmission_permitted"] = json!(false);
            if original["state"] == "UNKNOWN" {
                original["recovery_action"] = json!("operator_review_required");
                original["next_action"] = json!(
                    "Use inspect with this same cluster and submission_key to retrieve the original frozen script and request hashes. Missing remote intent does not prove historical non-submission. Preserve UNKNOWN; do not reconstruct intent files or automatically resubmit."
                );
            }
        }
        if let Some(record) = local_intent {
            if let Some(resolved) = crate::followup::resolve(&client, &record, &original)
                .await
                .map_err(execution_error)?
            {
                if let Some(prepared) = &prepared {
                    self.monitoring
                        .as_ref()
                        .unwrap()
                        .hub
                        .observe_submission(&prepared.id, &resolved)
                        .map_err(execution_error)?;
                }
                return Ok(ToolResult::success_json(resolved));
            }
        }
        if let Some(prepared) = prepared {
            let record = self
                .monitoring
                .as_ref()
                .unwrap()
                .hub
                .observe_submission(&prepared.id, &original)
                .map_err(execution_error)?;
            original["monitor"] = json!(record.view());
            original["monitoring_registered"] = json!(true);
            original["next_action"] = json!(if record.state == "not_submitted" {
                "No real submission was attempted. Inspect the validation diagnostic. This key remains bound to its original script/resources; this response does not grant permission to run a computation."
            } else if record.state == "submission_uncertain" {
                "Submission outcome is unresolved; no running job is verified. Operator attention is required. Recover only the same key/nonce; do not blindly resubmit or change keys to bypass UNKNOWN."
            } else {
                "The gateway durably monitors this exact submission and will wake this original conversation after verified terminal accounting. You may end this turn while the job runs. Do not repeatedly poll or resubmit. A terminal notification requires validation of existing logs, frozen output contracts/hashes and QC before declaring task readiness; no new computation is authorized by the notification."
            });
        }
        if original["state"] == "REJECTED" {
            return Ok(ToolResult::error(original.to_string()));
        }
        Ok(ToolResult::success_json(original))
    }
}

#[async_trait]
impl ToolFunction for JobsTool {
    type Input = JobsInput;
    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        self.invoke(input, None).await
    }
    async fn execute_with_context(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        self.invoke(serde_json::from_value(input)?, Some(ctx)).await
    }
}

fn safe_key(key: &str) -> Result<(), String> {
    if key.is_empty()
        || key.len() > 128
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        || key == "."
        || key == ".."
    {
        return Err(
            "submission_key must contain 1-128 ASCII letters, digits, underscore, dash or dot"
                .into(),
        );
    }
    Ok(())
}

pub(super) fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
    }
    check_private_directory(path)
}

fn check_private_directory(path: &Path) -> Result<(), String> {
    let info = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    // The state root is validated by HpcClient; additionally refuse public or
    // substituted child directories instead of changing their permissions.
    let parent = fs::metadata(path.parent().ok_or("State directory has no parent")?)
        .map_err(|e| e.to_string())?;
    if !info.is_dir() || info.permissions().mode() & 0o077 != 0 || info.uid() != parent.uid() {
        return Err("Job intent directory must be private, owned and nonsymlink".into());
    }
    Ok(())
}

pub(super) fn local_record_path(client: &HpcClient, key: &str) -> Result<PathBuf, String> {
    let destination = local_record_location(client, key)?;
    ensure_private_directory(destination.parent().ok_or("Job intent has no parent")?)?;
    Ok(destination)
}

fn local_record_location(client: &HpcClient, key: &str) -> Result<PathBuf, String> {
    safe_key(key)?;
    let root = client.state_dir().join("job-intents");
    match fs::symlink_metadata(&root) {
        Ok(_) => check_private_directory(&root)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    // Bind keys to the SSH endpoint: another configured account cannot adopt
    // a previous account's submission by accidentally reusing its local key.
    let identity = format!("{}\0{}", client.config().ssh_host, key);
    Ok(root.join(format!("{}.json", digest(identity.as_bytes()))))
}

pub(super) fn read_local_record(path: &Path) -> Result<Option<Value>, String> {
    use std::io::Read;
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path);
    let mut file = match opened {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let info = file.metadata().map_err(|e| e.to_string())?;
    let parent_info = fs::metadata(path.parent().ok_or("Job intent has no parent")?)
        .map_err(|e| e.to_string())?;
    if !info.is_file()
        || info.permissions().mode() & 0o077 != 0
        || info.uid() != parent_info.uid()
        || info.len() > 8 * 1024 * 1024
    {
        return Err("Unsafe or oversized local job intent".into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("Local job intent grew beyond its bound".into());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("Invalid durable local job intent: {e}"))
}

fn publish_local_record(destination: &Path, value: &Value) -> Result<Value, String> {
    // A competing process may publish after our initial open returned ENOENT.
    // Link a fully flushed private candidate into place; a late publisher reads
    // the complete winner rather than interpreting an old absence as failure.
    let temporary = destination.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    output
        .write_all(&serde_json::to_vec(value).map_err(|e| e.to_string())?)
        .and_then(|_| output.sync_all())
        .map_err(|e| e.to_string())?;
    let linked = fs::hard_link(&temporary, destination);
    fs::remove_file(&temporary).map_err(|e| e.to_string())?;
    match linked {
        Ok(()) => {
            fs::File::open(destination.parent().ok_or("Job intent has no parent")?)
                .and_then(|dir| dir.sync_all())
                .map_err(|e| e.to_string())?;
            Ok(value.clone())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_local_record(destination)?
                .ok_or_else(|| "Published local submission intent disappeared".into())
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn intent(
    client: &HpcClient,
    key: &str,
    request: Option<&Value>,
) -> Result<Value, String> {
    let destination = local_record_location(client, key)?;
    let request_hash = request
        .map(|v| serde_json::to_vec(v).map(|bytes| digest(&bytes)))
        .transpose()
        .map_err(|e| e.to_string())?;
    let record = match read_local_record(&destination) {
        Ok(Some(value)) => value,
        Ok(None) if request.is_some() => {
            ensure_private_directory(destination.parent().ok_or("Job intent has no parent")?)?;
            let nonce = uuid::Uuid::new_v4().simple().to_string();
            let value = json!({"version":1,"ssh_host":client.config().ssh_host,
                "submission_key":key,"nonce":nonce,"request_sha256":request_hash,
                "request":request});
            publish_local_record(&destination, &value)?
        }
        Ok(None) => return Err("No durable local submission intent exists for this key".into()),
        Err(e) => return Err(format!("Cannot recover local submission intent: {e}")),
    };
    if record["version"] != 1
        || record["ssh_host"] != client.config().ssh_host
        || record["submission_key"] != key
        || record["nonce"].as_str().is_none_or(|s| {
            s.len() != 32
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
    {
        return Err("Local submission intent identity mismatch".into());
    }
    if !record["request"].is_object()
        || record["request_sha256"]
            != digest(&serde_json::to_vec(&record["request"]).map_err(|e| e.to_string())?)
    {
        return Err(
            "Local submission request hash mismatch; preserve the original record for review"
                .into(),
        );
    }
    if let Some(expected) = request_hash {
        if record["request_sha256"] != expected {
            return Err(
                "submission_key is already bound to a different script or resources".into(),
            );
        }
    }
    Ok(record)
}

fn frozen_evidence(record: &Value) -> Result<Value, String> {
    let mut metadata = record["request"].clone();
    let script = metadata
        .as_object_mut()
        .and_then(|request| request.remove("script"))
        .ok_or("Frozen local request has no script")?;
    let script = script.as_str().ok_or("Frozen local script is not text")?;
    let resources: serde_json::Map<String, Value> = metadata
        .as_object()
        .ok_or("Frozen local request is not an object")?
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "tool" | "action" | "submission_key"))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    Ok(json!({
        "source":"durable_local_intent",
        "ssh_host":record["ssh_host"], "submission_key":record["submission_key"],
        "nonce":record["nonce"], "request_sha256":record["request_sha256"],
        "request_sha256_verified":true, "request_metadata":metadata,
        "script_sha256":digest(script.as_bytes()), "script_size_bytes":script.len(),
        "resources_sha256":digest(&serde_json::to_vec(&resources).map_err(|e|e.to_string())?),
        "resources":resources,
        "hash_encoding":"SHA256 of UTF-8 script bytes; request/resources use serde_json compact sorted-key JSON",
        "historical_submission_proven":false
    }))
}

fn inspect_intent(
    record: &Value,
    offset: Option<u64>,
    max_bytes: Option<u32>,
) -> Result<ToolResult, ToolError> {
    let evidence = frozen_evidence(record).map_err(execution_error)?;
    let script = record["request"]["script"].as_str().unwrap().as_bytes();
    let offset = offset.unwrap_or(0);
    let limit = max_bytes.unwrap_or(65536);
    if offset > script.len() as u64 || !(1..=1048576).contains(&limit) {
        return Err(ToolError::ValidationFailed {
            message:
                "inspect requires offset within the script and max_bytes between 1 and 1048576"
                    .into(),
        });
    }
    let start = offset as usize;
    let end = (start + limit as usize).min(script.len());
    Ok(ToolResult::success_json(json!({
        "state":"LOCAL_INTENT", "frozen_local_request":evidence,
        "script_chunk":{"encoding":"base64", "base64":base64::engine::general_purpose::STANDARD.encode(&script[start..end]),
            "offset":offset, "next_offset":end, "bytes":end-start, "truncated":end<script.len()},
        "remote_state":"NOT_QUERIED", "remote_operation_attempted":false,
        "new_job_submitted":false, "submission_retried":false,
        "original_outcome_resolved":false, "scientific_admission_granted":false
    })))
}

async fn run_remote(client: &HpcClient, request: &Value) -> Result<ToolResult, ToolError> {
    Ok(ToolResult::success_json(
        run_remote_value(client, request).await?,
    ))
}

async fn run_remote_value(client: &HpcClient, request: &Value) -> Result<Value, ToolError> {
    let value = client
        .run_json(REMOTE_JOBS_HELPER, request, false)
        .await
        .map_err(execution_error)?;
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(execution_error(
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("HPC operation returned no success envelope"),
        ));
    }
    Ok(value)
}

pub fn registrations(client: Arc<HpcClient>) -> Vec<ToolRegistration> {
    vec![
        ClusterTool {
            client: client.clone(),
        }
        .into(),
        JobsTool {
            client,
            monitoring: None,
        }
        .into(),
    ]
}

pub fn monitored_registrations(
    client: Arc<HpcClient>,
    monitoring: Monitoring,
) -> Vec<ToolRegistration> {
    vec![
        ClusterTool {
            client: client.clone(),
        }
        .into(),
        JobsTool {
            client,
            monitoring: Some(monitoring),
        }
        .into(),
    ]
}

#[cfg(test)]
#[path = "jobs_admission_tests.rs"]
mod admission_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn output(result: ToolResult) -> Value {
        serde_json::from_str(&result.text_content()).unwrap()
    }

    #[tokio::test]
    async fn inspect_reads_exact_legacy_script_in_byte_chunks_without_transport_or_rewrites() {
        let temporary = tempfile::tempdir().unwrap();
        let mut config = client(&temporary.path().join("state"), "toy")
            .config()
            .clone();
        config.ssh_command = "/must-not-run-ssh".into();
        let tool = JobsTool {
            client: Arc::new(HpcClient::new(config).unwrap()),
            monitoring: None,
        };
        // Invalid old controls remain inspectable: evidence is never normalized
        // into a different request or replaced with a reconstructed script.
        let script = "#!/bin/sh\r\n# 原始脚本\nprintf 'null'\n";
        let request = json!({"tool":"jobs", "action":"submit", "submission_key":"legacy",
            "script":script, "working_directory":"/old", "partition":"cpu", "array":"null"});
        let record = intent(&tool.client, "legacy", Some(&request)).unwrap();
        let path = local_record_path(&tool.client, "legacy").unwrap();
        let original_bytes = fs::read(&path).unwrap();
        let mut offset = 0;
        let mut recovered = Vec::new();
        loop {
            let input = json!({"action":"inspect", "submission_key":"legacy", "offset":offset, "max_bytes":1});
            let value = output(
                tool.invoke(serde_json::from_value(input).unwrap(), None)
                    .await
                    .unwrap(),
            );
            let evidence = &value["frozen_local_request"];
            assert_eq!(evidence["nonce"], record["nonce"]);
            assert_eq!(evidence["request_sha256"], record["request_sha256"]);
            assert_eq!(evidence["script_sha256"], digest(script.as_bytes()));
            assert_eq!(evidence["resources"]["array"], "null");
            let mut reconstructed = evidence["request_metadata"].clone();
            reconstructed["script"] = json!(script);
            assert_eq!(
                digest(&serde_json::to_vec(&reconstructed).unwrap()),
                record["request_sha256"]
            );
            assert_eq!(value["remote_operation_attempted"], false);
            assert_eq!(value["original_outcome_resolved"], false);
            let chunk = &value["script_chunk"];
            recovered.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(chunk["base64"].as_str().unwrap())
                    .unwrap(),
            );
            offset = chunk["next_offset"].as_u64().unwrap();
            if chunk["truncated"] == false {
                break;
            }
        }
        assert_eq!(recovered, script.as_bytes());
        assert_eq!(fs::read(path).unwrap(), original_bytes);
        assert!(!tool.client.state_dir().join("job-monitors").exists());
    }

    #[tokio::test]
    async fn inspection_of_absent_key_is_read_only_and_ranges_are_bounded() {
        let temporary = tempfile::tempdir().unwrap();
        let client = Arc::new(client(&temporary.path().join("state"), "toy"));
        let tool = JobsTool {
            client,
            monitoring: None,
        };
        let input = json!({"action":"inspect", "submission_key":"absent"});
        assert!(
            tool.invoke(serde_json::from_value(input).unwrap(), None)
                .await
                .is_err()
        );
        assert_eq!(fs::read_dir(tool.client.state_dir()).unwrap().count(), 0);
        let record = intent(&tool.client, "bounded", Some(&json!({"script":"abc"}))).unwrap();
        for (offset, limit) in [(4, 1), (u64::MAX, 1), (0, 0), (0, 1048577)] {
            assert!(inspect_intent(&record, Some(offset), Some(limit)).is_err());
        }
        let eof = output(inspect_intent(&record, Some(3), Some(1)).unwrap());
        assert_eq!(eof["script_chunk"]["bytes"], 0);
    }

    #[tokio::test]
    async fn recovery_and_inspection_reject_replacement_payloads_before_transport() {
        let tool = JobsTool {
            client: Arc::new(HpcClient::definition_only()),
            monitoring: None,
        };
        for action in ["recover", "inspect"] {
            for (field, value) in [
                ("script", json!("replacement")),
                ("memory_mb", json!(64)),
                ("working_directory", json!("/different")),
            ] {
                let mut input = json!({"action":action, "submission_key":"legacy"});
                input[field] = value;
                let error = tool
                    .invoke(serde_json::from_value(input).unwrap(), None)
                    .await
                    .unwrap_err();
                assert!(matches!(error, ToolError::ValidationFailed { .. }));
                assert!(error.to_string().contains(field));
            }
        }
    }

    #[test]
    fn local_request_tampering_is_rejected_before_recovery_or_evidence_export() {
        let temporary = tempfile::tempdir().unwrap();
        let client = client(&temporary.path().join("state"), "toy");
        let mut record = intent(&client, "original", Some(&json!({"script":"original"}))).unwrap();
        record["request"]["script"] = json!("reconstructed");
        let path = local_record_path(&client, "original").unwrap();
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(
            intent(&client, "original", None)
                .unwrap_err()
                .contains("hash mismatch")
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[tokio::test]
    async fn missing_remote_intent_exposes_frozen_hashes_without_changing_unknown() {
        let temporary = tempfile::tempdir().unwrap();
        let ssh = temporary.path().join("fake-ssh");
        fs::write(&ssh, r#"#!/usr/bin/env python3
import json,sys
request=json.load(sys.stdin)['input']
assert request['action']=='recover'
assert request['expected_request']['array']=='null'
assert 'script' not in request
print(json.dumps({'ok':True, 'state':'UNKNOWN', 'reason':'remote_intent_missing', 'nonce':request['nonce']}))
"#).unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        let mut config = client(&temporary.path().join("state"), "toy")
            .config()
            .clone();
        config.ssh_command = ssh.to_string_lossy().into_owned();
        let tool = JobsTool {
            client: Arc::new(HpcClient::new(config).unwrap()),
            monitoring: None,
        };
        let record = intent(
            &tool.client,
            "legacy",
            Some(&json!({"script":"original", "array":"null"})),
        )
        .unwrap();
        let path = local_record_path(&tool.client, "legacy").unwrap();
        let bytes = fs::read(&path).unwrap();
        let result = output(
            tool.invoke(
                serde_json::from_value(json!({"action":"recover", "submission_key":"legacy"}))
                    .unwrap(),
                None,
            )
            .await
            .unwrap(),
        );
        assert_eq!(result["state"], "UNKNOWN");
        assert_eq!(result["reason"], "remote_intent_missing");
        assert_eq!(
            result["frozen_local_request"]["request_sha256"],
            record["request_sha256"]
        );
        assert_eq!(
            result["frozen_local_request"]["script_sha256"],
            digest(b"original")
        );
        assert_eq!(result["automatic_resubmission_permitted"], false);
        assert_eq!(result["new_job_submitted"], false);
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[tokio::test]
    async fn invalid_submission_never_binds_key_and_corrected_request_reuses_it() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        let ssh = temporary.path().join("fake-ssh");
        // Run the real pure helper validator; the submission branch is only a
        // synthetic reply and cannot invoke a scheduler or contact a cluster.
        fs::write(
            &ssh,
            r#"#!/usr/bin/env python3
import json,sys
envelope=json.load(sys.stdin)
request=envelope['input']
if request['action']=='validate_submission_request':
    exec(envelope['code'], {'REQUEST':request})
else:
    assert request['action']=='submit'
    print(json.dumps({'ok':True,'state':'SUBMITTED','job_id':'123'}))
"#,
        )
        .unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        let mut config = client(&state, "toy").config().clone();
        config.ssh_command = ssh.to_string_lossy().into_owned();
        let tool = JobsTool {
            client: Arc::new(HpcClient::new(config).unwrap()),
            monitoring: None,
        };
        let mut request = json!({"action":"submit", "submission_key":"one", "script":"#!/bin/sh\ntrue\n", "working_directory":"/toy", "array":"null"});
        let error = tool
            .invoke(serde_json::from_value(request.clone()).unwrap(), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("JSON null"));
        assert!(!state.join("job-intents").exists());
        assert!(!state.join("job-monitors").exists());
        // Non-null malformed controls are also rejected before intent creation.
        request["array"] = json!("0;evil");
        let rejected = tool
            .invoke(serde_json::from_value(request.clone()).unwrap(), None)
            .await
            .unwrap();
        assert_eq!(rejected.is_error, Some(true));
        assert!(!state.join("job-intents").exists());
        request["array"] = Value::Null;
        let submitted = tool
            .invoke(serde_json::from_value(request).unwrap(), None)
            .await
            .unwrap();
        assert_ne!(submitted.is_error, Some(true));
        assert!(intent(&tool.client, "one", None).is_ok());
    }

    #[tokio::test]
    async fn preflight_transport_failure_does_not_consume_submission_key() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        let mut config = client(&state, "toy").config().clone();
        config.ssh_command = "/bin/false".into();
        let tool = JobsTool {
            client: Arc::new(HpcClient::new(config).unwrap()),
            monitoring: None,
        };
        let input = serde_json::from_value(json!({"action":"submit", "submission_key":"one", "script":"#!/bin/sh\ntrue\n", "working_directory":"/toy"})).unwrap();
        assert!(tool.invoke(input, None).await.is_err());
        assert!(!state.join("job-intents").exists());
        assert!(!state.join("job-monitors").exists());
    }

    #[test]
    fn submission_keys_cannot_escape_record_directory() {
        for key in ["", ".", "..", "../x", "hello/x", "\n", "x;rm", "中文"] {
            assert!(safe_key(key).is_err());
        }
        assert!(safe_key("analysis-v1_20261024.01").is_ok());
    }

    fn client(state: &Path, host: &str) -> HpcClient {
        HpcClient::new(crate::HpcConfig {
            ssh_host: host.into(),
            data_ssh_host: None,
            pi_ssh_host: None,
            ssh_command: "ssh".into(),
            remote_python: "python3".into(),
            state_dir: state.to_path_buf(),
            default_partition: "cpu".into(),
            default_account: None,
        })
        .unwrap()
    }

    #[test]
    fn local_intent_is_reused_across_client_restart_and_endpoint_scoped() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        // HpcClient validates/creates the private persistent state root.
        let first = client(&state, "toy-a");
        let request = json!({"script":"#!/bin/sh\ntrue\n", "partition":"cpu"});
        let record = intent(&first, "stable-1", Some(&request)).unwrap();
        let restarted = client(&state, "toy-a");
        assert_eq!(
            intent(&restarted, "stable-1", Some(&request)).unwrap(),
            record
        );
        assert_eq!(intent(&restarted, "stable-1", None).unwrap(), record);
        assert!(intent(&restarted, "stable-1", Some(&json!({"script":"changed"}))).is_err());
        let other = client(&state, "toy-b");
        assert_ne!(
            intent(&other, "stable-1", Some(&request)).unwrap()["nonce"],
            record["nonce"]
        );
        assert!(intent(&restarted, "unknown-key", None).is_err());
    }

    #[test]
    fn concurrent_local_intent_publish_has_one_complete_winner() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        let client = Arc::new(client(&state, "toy"));
        let start = Arc::new(std::sync::Barrier::new(8));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let client = client.clone();
            let start = start.clone();
            workers.push(std::thread::spawn(move || {
                start.wait();
                intent(&client, "concurrent", Some(&json!({"script":"toy"}))).unwrap()
            }));
        }
        let records: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert!(records.iter().all(|record| record == &records[0]));
    }

    #[test]
    fn winner_published_after_initial_absence_is_adopted_by_late_publisher() {
        let temporary = tempfile::tempdir().unwrap();
        let client = client(&temporary.path().join("state"), "toy");
        let destination = local_record_path(&client, "late-publisher").unwrap();
        // Exactly the formerly failing ordering: a caller observes ENOENT,
        // another caller publishes, then the first caller attempts publication.
        assert_eq!(read_local_record(&destination).unwrap(), None);
        let winner = intent(&client, "late-publisher", Some(&json!({"script":"toy"}))).unwrap();
        let mut losing_candidate = winner.clone();
        losing_candidate["nonce"] = json!("00000000000000000000000000000000");
        assert_eq!(
            publish_local_record(&destination, &losing_candidate).unwrap(),
            winner
        );
        assert_eq!(read_local_record(&destination).unwrap(), Some(winner));
    }

    #[test]
    fn escaped_script_intent_reopens_within_the_transport_bound() {
        let temporary = tempfile::tempdir().unwrap();
        let client = client(&temporary.path().join("state"), "toy");
        let request = json!({"script": format!("#!/bin/sh\n#{}", "\u{1}".repeat(800 * 1024))});
        let first = intent(&client, "escaped", Some(&request)).unwrap();
        assert_eq!(intent(&client, "escaped", None).unwrap(), first);
    }

    #[test]
    fn selected_cluster_scopes_submission_keys_to_configured_endpoint() {
        let temporary = tempfile::tempdir().unwrap();
        let base = client(&temporary.path().join("state"), "siyuan-toy");
        let mut config = base.config().clone();
        config.pi_ssh_host = Some("pi-toy".into());
        config.default_partition = "64c512g".into();
        let base = HpcClient::new(config).unwrap();
        let siyuan = base.for_cluster(Some("siyuan")).unwrap();
        let pi = base.for_cluster(Some("pi")).unwrap();
        assert_eq!(siyuan.config().ssh_host, "siyuan-toy");
        assert_eq!(pi.config().ssh_host, "pi-toy");
        assert_eq!(pi.config().default_partition, "cpu");
        let request = json!({"script":"toy"});
        let first = intent(&base, "same-key", Some(&request)).unwrap();
        assert_eq!(intent(&siyuan, "same-key", Some(&request)).unwrap(), first);
        assert_ne!(
            intent(&pi, "same-key", Some(&request)).unwrap()["nonce"],
            first["nonce"]
        );
        assert!(base.for_cluster(Some("attacker@arbitrary-host")).is_err());
    }
}
