//! The sbatch lifecycle: submit (uncertainty-safe), observe via
//! squeue/sacct, cancel with stop proofs, and collect outputs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::contract::{TaskAttemptReceipt, TaskDispatch, TaskExecution};
use super::super::heartbeat;
use super::super::process::{
    ProcessTaskSpec, parse_process_task, process_task_environment, render_process_script,
    sanitize_path_component,
};
use super::super::traits::{TaskArtifactStore, TaskExecutor, TaskLease, TaskTransport};
use super::settings::SlurmSettings;
use super::ssh::{
    SshTarget, ensure_ssh_success, hex_encode, nonce_hex, queue_stdout_text, sh_single_quote,
    slurm_job_name, ssh_stdout_text,
};
use super::submission::{
    self, SlurmRecordedSubmission, SlurmSubmissionIntent, SlurmSubmissionRecorder,
};
use crate::dag::DagError;
use crate::dag::node_event::JobResult;
use crate::value::{DomainId, FileFingerprint, FileRef};

// ── transport ────────────────────────────────────────────────────────────
#[derive(Clone)]
struct PendingSlurmJob {
    dispatch: TaskDispatch,
    process: ProcessTaskSpec,
    task_dir: String,
    /// Identity of the exact heredoc payload admitted before the SSH submit.
    /// Its trailing LF is part of task.json; the POSIX path remains mutable.
    manifest_fingerprint: FileFingerprint,
    submitted_at: std::time::Instant,
    intent: SlurmSubmissionIntent,
    record: Option<std::sync::Arc<dyn SlurmRecordedSubmission>>,
}
impl std::fmt::Debug for PendingSlurmJob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingSlurmJob")
            .field("task_id", &self.dispatch.spec.id)
            .field("task_dir", &self.task_dir)
            .finish()
    }
}
/// A [`TaskTransport`] that runs `process` tasks as sbatch jobs on a Slurm
/// cluster reached over ssh.
#[derive(Debug)]
pub struct SlurmTaskTransport {
    pub(crate) settings: SlurmSettings,
    pending: Mutex<HashMap<String, PendingSlurmJob>>,
    cancelled: Mutex<HashSet<String>>,
    pub(crate) scratch_home: Mutex<Option<String>>,
    /// Cached accounting-liveness verdict `(observed_at, alive)`: an empty
    /// sacct result may only be read as "no such job" while accounting is
    /// demonstrably recording this user's jobs.
    accounting_alive: Mutex<Option<(std::time::Instant, bool)>>,
    recorder: Option<std::sync::Arc<dyn SlurmSubmissionRecorder>>,
}
impl SlurmTaskTransport {
    pub fn new(settings: SlurmSettings) -> Self {
        Self {
            settings,
            pending: Mutex::new(HashMap::new()),
            cancelled: Mutex::new(HashSet::new()),
            scratch_home: Mutex::new(None),
            accounting_alive: Mutex::new(None),
            recorder: None,
        }
    }
    pub fn with_submission_recorder(
        mut self,
        recorder: std::sync::Arc<dyn SlurmSubmissionRecorder>,
    ) -> Self {
        self.recorder = Some(recorder);
        self
    }
    async fn submit_job(
        &self,
        mut dispatch: TaskDispatch,
        artifacts: Vec<FileRef>,
    ) -> Result<(TaskLease, PendingSlurmJob), DagError> {
        if let Some(recorder) = &self.recorder {
            recorder.ensure_ready()?;
        }
        dispatch.resources.validate()?;
        let token = |value: &str| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        };
        if !token(&self.settings.partition)
            || self.settings.qos.as_ref().is_some_and(|qos| !token(qos))
            || self
                .settings
                .account
                .as_ref()
                .is_some_and(|account| !token(account))
        {
            return Err(DagError::Schedule(
                "invalid Slurm partition, account or QOS".into(),
            ));
        }
        if self.settings.gpus == Some(0) {
            return Err(DagError::Schedule(
                "Slurm GPU count must be positive".into(),
            ));
        }
        let process = parse_process_task(&dispatch)?;
        let mut input_artifacts_by_name = BTreeMap::<String, Vec<FileRef>>::new();
        let mut input_artifacts = artifacts.iter();
        for binding in &dispatch.spec.inputs {
            input_artifacts_by_name.insert(
                binding.name.clone(),
                input_artifacts
                    .by_ref()
                    .take(binding.staged_paths.len())
                    .cloned()
                    .collect(),
            );
        }
        let container_binding = match &process.container {
            Some(container) => {
                let settings = self.settings.apptainer.as_ref().ok_or_else(|| {
                    DagError::Schedule(
                        "Apptainer process requested without an operator SIF manifest".into(),
                    )
                })?;
                let image = settings.resolve(container)?;
                Some(
                    serde_json::json!({"program": settings.program, "sif": image,
                    "mode": container.mode, "network": container.network}),
                )
            }
            None => None,
        };
        let inputs_by_name = match_uploaded_inputs(&dispatch, artifacts)?;
        // Resolve the scratch dir to an absolute remote path (home is
        // resolved once and cached): sbatch directive values cannot carry
        // `~` or quotes, and compute nodes mount the same filesystem, so an
        // absolute scratch path works from both sides.
        let scratch = resolve_remote_prefix(
            &self.settings.ssh,
            &self.settings.scratch_dir,
            &self.scratch_home,
        )
        .await?;
        let attempt_key = attempt_key(&dispatch)?;
        let task_dir = remote_task_dir(&scratch, &attempt_key);
        let mut environment =
            process_task_environment(&dispatch.spec.id, &task_dir, &process.env, &inputs_by_name)?;
        environment.insert(
            "AUTONOMICS_INPUT_ARTIFACTS".into(),
            serde_json::to_string(&input_artifacts_by_name).map_err(|error| {
                DagError::Schedule(format!("cannot encode transport input identities: {error}"))
            })?,
        );
        if environment
            .get("AUTONOMICS_INPUTS")
            .is_some_and(|value| value.len() > 64 * 1024)
            || environment
                .get("AUTONOMICS_INPUT_ARTIFACTS")
                .is_some_and(|value| value.len() > 64 * 1024)
        {
            return Err(DagError::Schedule(
                "transport input metadata exceeds its 64 KiB worker bound".into(),
            ));
        }
        dispatch.spec.spec["_autonomics_input_artifacts"] =
            serde_json::to_value(&input_artifacts_by_name)
                .map_err(|error| DagError::Schedule(error.to_string()))?;
        let manifest_result = if container_binding.is_some() || self.settings.gpus.is_some() {
            // Bind resolved executable and scheduler allocation into custody identity.
            let mut manifest_value = serde_json::to_value(&dispatch).map_err(|error| {
                DagError::Schedule(format!("cannot serialize slurm task manifest: {error}"))
            })?;
            if let Some(binding) = container_binding {
                manifest_value["apptainer_execution"] = binding;
            }
            if let Some(gpus) = self.settings.gpus {
                manifest_value["slurm_gpu_allocation"] = serde_json::json!({
                    "partition": self.settings.partition, "account": self.settings.account,
                    "gpus": gpus, "qos": self.settings.qos
                });
            }
            serde_json::to_string_pretty(&manifest_value)
        } else {
            // Preserve existing CPU-only, non-container submission identities.
            serde_json::to_string_pretty(&dispatch)
        };
        let manifest = manifest_result.map_err(|error| {
            DagError::Schedule(format!("cannot serialize slurm task manifest: {error}"))
        })?;
        let nonce = nonce_hex(8);
        let job_name = slurm_job_name(&dispatch.spec.id, &nonce);
        let sbatch_script = render_sbatch_script(
            &dispatch,
            &process,
            &task_dir,
            &environment,
            &inputs_by_name,
            &self.settings,
            &job_name,
        )?;
        let bootstrap = render_bootstrap_script(&task_dir, &manifest, &sbatch_script)?;
        // A quoted heredoc preserves all manifest bytes and appends exactly
        // one LF before its delimiter. Bind the receipt to those submitted
        // bytes, never a hash learned by reopening the remote path afterward.
        let manifest_fingerprint = FileFingerprint {
            size: manifest.len() as u64 + 1,
            mtime_ns: 0,
            content_hash: Some(hex_encode(
                &Sha256::new()
                    .chain_update(manifest.as_bytes())
                    .chain_update(b"\n")
                    .finalize(),
            )),
            immutable_remote: false,
        };
        let mut admitted = None;
        // Never replay submission after a transport failure. Only read the
        // durable job record; an absent record is explicitly UNKNOWN.
        let response = self
            .settings
            .ssh
            .run_submission_once(&bootstrap, || {
                let intent = self.persist_intent(
                    &dispatch.spec.id,
                    &job_name,
                    &nonce,
                    &task_dir,
                    &manifest,
                )?;
                let record = self
                    .recorder
                    .as_ref()
                    .map(|r| r.before_submit(&intent))
                    .transpose()?;
                let scope = submission::SubmissionScope::new(record.clone());
                admitted = Some((intent, record, scope));
                Ok(())
            })
            .await;
        let Some((intent, record, mut submission_scope)) = admitted else {
            return Err(response.err().unwrap_or_else(|| {
                DagError::Schedule("SSH submission payload was not admitted".into())
            }));
        };
        let cause = match &response {
            Ok(output) => format!("initial SSH exit {:?}", output.status.code()),
            Err(error) => error.to_string(),
        };
        // A diagnostic write failure must not skip read-only reconciliation of
        // a job that may already exist. Preserve the failure in the final error.
        let diagnostic =
            match intent.persist_response(&self.settings.local_artifact_root, &response) {
                Ok(path) => format!("private submission evidence: {}", path.display()),
                Err(error) => format!("submission evidence unavailable: {error}"),
            };
        let stdout = match response {
            Ok(output) if output.status.success() => ssh_stdout_text(output, "sbatch submission")?,
            Ok(output) if !matches!(output.status.code(), Some(255 | 92)) => {
                return Err(DagError::Schedule(format!(
                    "slurm submission UNKNOWN: {}; {cause}; {diagnostic}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            _ => {
                let identity = submission::identity(&task_dir, &manifest);
                let recovery = format!(
                    r#"cd {dir} &&
[ "$(cat submission.identity)" = '{identity}' ] &&
if [ -s submission.job-id ]; then cat submission.job-id; else
  records=$(sacct -n -P -X -o JobIDRaw,Comment%100) || exit 92
  ids=$(printf '%s\n' "$records" | awk -F '|' '$2 == "autonomics-{identity}" {{print $1}}') || exit 92
  case "$ids" in ''|*[!0-9]*) echo 'submission UNKNOWN' >&2; exit 92;; esac
  printf '%s\n' "$ids" > submission.job-id.recovered
  mv submission.job-id.recovered submission.job-id
  cat submission.job-id
fi"#,
                    dir = sh_single_quote(&task_dir)
                );
                let recovered = self.settings.ssh.run(&recovery).await.and_then(|output| {
                    ssh_stdout_text(
                        output,
                        "slurm submission UNKNOWN; retain workspace and reconcile intent",
                    )
                });
                match recovered {
                    Ok(id) if parse_submitted_job_id(&id, &dispatch.spec.id).is_ok() => id,
                    _ => match self
                        .adopt_submitted_job(&job_name, &format!("{cause}; {diagnostic}"))
                        .await?
                    {
                        Adoption::Found(id) => id,
                        Adoption::ClearToRetry => {
                            return Err(DagError::Schedule(format!(
                                "slurm submission UNKNOWN; empty listings do not authorize replay; retain workspace and reconcile intent; {cause}; {diagnostic}"
                            )));
                        }
                    },
                }
            }
        };
        let job_id = parse_submitted_job_id(&stdout, &dispatch.spec.id)?;
        submission_scope.disarm();
        Ok((
            TaskLease {
                lease_id: job_id,
                task_id: dispatch.spec.id.clone(),
            },
            PendingSlurmJob {
                dispatch,
                process,
                task_dir,
                manifest_fingerprint,
                submitted_at: std::time::Instant::now(),
                intent,
                record,
            },
        ))
    }
    /// Consult the cluster for jobs matching exactly `job_name` after a
    /// submission attempt whose outcome was lost. Two independent sources,
    /// and an empty result is only believed when accounting is demonstrably
    /// alive; see [`Self::query_name_candidates`] for the source details.
    async fn adopt_submitted_job(&self, job_name: &str, cause: &str) -> Result<Adoption, DagError> {
        let unknown = |detail: String| {
            DagError::Schedule(format!(
                "submission status unknown for job name `{job_name}` ({detail}); submit failure: {cause}"
            ))
        };
        let (squeue_ids, sacct_ids) = self.query_name_candidates(job_name).await?;
        let mut candidates = merge_candidates(&squeue_ids, &sacct_ids);
        if candidates.len() > 1 {
            if self.recorder.is_some() {
                return Err(unknown("ambiguous candidates require original ownership reconciliation; no cancellation attempted".into()));
            }
            // Every candidate carries this attempt's unique nonce, so they
            // can only be ours; leaving them queued would burn walltime
            // until the limit kills them. Best-effort stop, then refuse to
            // guess.
            self.best_effort_cancel_candidates(&candidates).await;
            return Err(unknown(format!(
                "{} candidate jobs matched: {candidates:?} (best-effort cancelled)",
                candidates.len()
            )));
        }
        if candidates.is_empty() {
            if !self
                .accounting_liveness()
                .await
                .map_err(|error| unknown(error.to_string()))?
            {
                // Accounting off or empty: "no record" cannot be
                // distinguished from "did not run", so a retry might
                // double-submit.
                return Err(unknown(
                    "accounting shows no recent records, so an empty query proves nothing".into(),
                ));
            }
            // A query issued microseconds after sbatch ran can miss the job
            // during the queue/accounting hand-off; settle, then re-query.
            tokio::time::sleep(Duration::from_secs(2)).await;
            let (squeue_ids, sacct_ids) = self.query_name_candidates(job_name).await?;
            candidates = merge_candidates(&squeue_ids, &sacct_ids);
            return match candidates.len() {
                1 => Ok(Adoption::Found(candidates.remove(0))),
                0 => Ok(Adoption::ClearToRetry),
                _ => {
                    self.best_effort_cancel_candidates(&candidates).await;
                    Err(unknown(format!(
                        "{} candidate jobs matched after re-query: {candidates:?} (best-effort cancelled)",
                        candidates.len()
                    )))
                }
            };
        }
        Ok(Adoption::Found(candidates.remove(0)))
    }
    /// Run both adoption queries; `Err` means the query channel itself
    /// failed, which is ambiguity — never evidence of zero.
    async fn query_name_candidates(
        &self,
        job_name: &str,
    ) -> Result<(Vec<String>, Vec<String>), DagError> {
        // `squeue --name=` is an unanchored regular expression over a field
        // we do not fully control (sanitized ids may contain `.`), so pull
        // the full listing and match client-side instead.
        let listing = ssh_stdout_text(
            self.settings.ssh.run("squeue -h -o \"%i %j\"").await?,
            "squeue adoption listing",
        )?;
        let mut squeue_ids = Vec::new();
        for line in listing.lines() {
            let line = line.trim();
            let mut fields = line.splitn(2, char::is_whitespace);
            let (Some(id), Some(name)) = (fields.next(), fields.next().map(str::trim)) else {
                continue;
            };
            if name == job_name {
                if let Some(base) = numeric_job_base_id(id) {
                    squeue_ids.push(base);
                }
            }
        }
        let records = ssh_stdout_text(
            self.settings
                .ssh
                .run(&format!(
                    "sacct -n -P -X --name={} -o JobID,State -S now-1hour",
                    sh_single_quote(job_name)
                ))
                .await?,
            "sacct adoption query",
        )?;
        let mut sacct_ids = Vec::new();
        for line in records.lines() {
            if let Some(base) = numeric_job_base_id(line.split('|').next().unwrap_or_default()) {
                sacct_ids.push(base);
            }
        }
        Ok((squeue_ids, sacct_ids))
    }
    async fn best_effort_cancel_candidates(&self, candidates: &[String]) {
        for id in candidates {
            match self.settings.ssh.run(&format!("scancel {id}")).await {
                Ok(output) if !output.status.success() => {
                    tracing::warn!(job = %id, "best-effort scancel of an ambiguous candidate failed");
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    job = %id,
                    error = %error,
                    "best-effort scancel could not reach the cluster"
                ),
            }
        }
    }
    /// True only when accounting demonstrably records this user's recent
    /// jobs. TTL-cached: the verdict changes slowly and this otherwise
    /// doubles the query load on every ambiguous submit.
    async fn accounting_liveness(&self) -> Result<bool, DagError> {
        const CANARY_TTL: Duration = Duration::from_secs(300);
        if let Some((observed, alive)) =
            *self.accounting_alive.lock().map_err(|_| poisoned_state())?
        {
            if observed.elapsed() < CANARY_TTL {
                return Ok(alive);
            }
        }
        let output = self
            .settings
            .ssh
            // `sacct --me` is unavailable on older site installations. Resolve
            // the authenticated remote account, never the coordinator's USER.
            .run("accounting_uid=$(id -u) && LC_ALL=C sacct -n -P -X -S now-1day -E now -u \"$accounting_uid\" -o JobID")
            .await?;
        if !output.status.success() {
            return Err(DagError::Schedule(format!(
                "accounting liveness probe failed with exit status {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let alive = String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| !line.trim().is_empty());
        *self.accounting_alive.lock().map_err(|_| poisoned_state())? =
            Some((std::time::Instant::now(), alive));
        Ok(alive)
    }
    /// Persist the submission intent BEFORE the ssh call: if the channel
    /// dies ambiguously, this record is what tells an operator (or a later
    /// session) that a job of this name may exist on the cluster. Write
    /// failure aborts the submission — going ahead without the record is
    /// how orphans happen.
    fn persist_intent(
        &self,
        task_id: &str,
        job_name: &str,
        nonce: &str,
        task_dir: &str,
        manifest: &str,
    ) -> Result<SlurmSubmissionIntent, DagError> {
        let intent = SlurmSubmissionIntent::new(
            &self.settings,
            task_id,
            job_name,
            nonce,
            task_dir,
            manifest,
        )?;
        intent.persist(&self.settings.local_artifact_root)?;
        Ok(intent)
    }
    async fn record_owned_stop(&self, job: &PendingSlurmJob, job_id: &str) -> Result<(), DagError> {
        if let Some(record) = &job.record {
            let facts = self.collect_submission_stop_evidence(&job.intent).await?;
            if facts["job_id"] != job_id {
                return Err(DagError::Schedule(
                    "stop evidence changed the submitted job id".into(),
                ));
            }
            record.stopped(&facts)?;
        }
        Ok(())
    }
    async fn observe_worker_heartbeat(
        &self,
        job: &PendingSlurmJob,
        job_id: &str,
        reporter: &crate::dag::node_event::NodeReporter,
        phase: &str,
    ) -> bool {
        let Some(expected) = job.manifest_fingerprint.content_hash.as_deref() else {
            return false;
        };
        let command = format!(
            "python3 -B -c {} {} {} {} {}",
            sh_single_quote(include_str!("read_heartbeat.py")),
            sh_single_quote(&format!(
                "{}/{}",
                job.task_dir,
                super::super::heartbeat::FILE_NAME
            )),
            sh_single_quote(&job.dispatch.spec.id),
            sh_single_quote(job_id),
            sh_single_quote(expected.strip_prefix("sha256:").unwrap_or(expected))
        );
        let observed = self
            .settings
            .ssh
            .run(&command)
            .await
            .and_then(|output| ssh_stdout_text(output, "worker heartbeat"))
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
        let Some(mut worker) = observed.filter(serde_json::Value::is_object) else {
            return false;
        };
        worker["evidence"] = "heartbeat".into();
        worker["backend"] = "slurm".into();
        worker["id"] = job_id.into();
        worker["scheduler_state"] = "RUNNING".into();
        reporter.execution(crate::dag::live::ExecutionObservation {
            phase: phase.into(),
            worker: Some(worker),
            ..Default::default()
        });
        true
    }
    /// Poll `sacct` until the job has accounting records. Accounting can
    /// lag the queue by a beat, so retry a few times before declaring the
    /// record lost. Array/step jobs report one record each — ALL records
    /// come back, and success requires every one of them to be clean.
    async fn wait_terminal_state(&self, job_id: &str) -> Result<(String, i32), DagError> {
        for _ in 0..5u32 {
            let output = self
                .settings
                .ssh
                .run(&format!(
                    "sacct -j {job_id} -n -P -X -o JobIDRaw,State%40,ExitCode"
                ))
                .await?;
            let stdout = ssh_stdout_text(output, "sacct")?;
            let records: Vec<_> = stdout
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            if records.len() == 1 {
                return parse_sacct_record(records[0], job_id);
            }
            if records.len() > 1 {
                return Err(DagError::Schedule(
                    "ambiguous sacct records; state UNKNOWN".into(),
                ));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err(DagError::Schedule(format!(
            "slurm accounting has no record for job `{job_id}`"
        )))
    }
    /// Collect declared outputs on the remote side and name them into the
    /// `output-{port}-{index}` convention the download protocol expects.
    async fn collect_outputs(
        &self,
        job: &PendingSlurmJob,
    ) -> Result<BTreeMap<u8, Vec<FileRef>>, DagError> {
        let script = render_collect_script(&job.process, &job.task_dir);
        let stdout = ssh_stdout_text(
            self.settings.ssh.run_script_once(&script).await?,
            "slurm output collection",
        )?;
        let mut by_port = BTreeMap::<u8, Vec<FileRef>>::new();
        for line in stdout.lines() {
            let mut fields = line.split_whitespace();
            if fields.next() != Some("AUTONOMICS_ARTIFACT") {
                continue;
            }
            let (Some(port), Some(index)) = (
                fields.next().and_then(|port| port.parse::<u8>().ok()),
                fields.next().and_then(|index| index.parse::<usize>().ok()),
            ) else {
                continue;
            };
            let extension = fields.next().unwrap_or("-");
            let format = (extension != "-").then(|| extension.to_ascii_lowercase());
            // The collect script reports each artifact's size and SHA-256,
            // so receipts carry real fingerprints: downstream incremental
            // invalidation can use them, and download verification kicks in
            // if these artifacts are ever pulled again as inputs.
            let fingerprint = match (
                fields.next().and_then(|size| size.parse::<u64>().ok()),
                fields.next(),
            ) {
                (Some(size), Some(digest))
                    if digest.len() == 64
                        && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
                {
                    Some(FileFingerprint::remote(
                        size,
                        Some(digest.to_ascii_lowercase()),
                    ))
                }
                _ => None,
            };
            by_port.entry(port).or_default().push(FileRef {
                domain: DomainId::local(),
                path: format!("{}/output-{port}-{index}", job.task_dir),
                format,
                fingerprint,
            });
        }
        for output in &job.process.outputs {
            if !output.optional && !by_port.contains_key(&output.port) {
                return Err(DagError::Schedule(format!(
                    "slurm output port {} matched no files for `{}`",
                    output.port, output.pattern
                )));
            }
        }
        Ok(by_port)
    }
}
fn poisoned_state() -> DagError {
    DagError::Schedule("slurm transport state poisoned".into())
}
/// Pair uploaded artifacts with declared inputs by declaration order —
/// the same contract [`super::super::remote::RemoteTaskExecutor`] uses when it
/// uploads (`input-{port}-{index}` names, one artifact per staged path).
pub(crate) fn match_uploaded_inputs(
    dispatch: &TaskDispatch,
    artifacts: Vec<FileRef>,
) -> Result<BTreeMap<String, Vec<PathBuf>>, DagError> {
    let mut remaining = artifacts.into_iter();
    let mut inputs_by_name = BTreeMap::new();
    for binding in &dispatch.spec.inputs {
        let mut staged = Vec::with_capacity(binding.staged_paths.len());
        for (index, _) in binding.staged_paths.iter().enumerate() {
            let artifact = remaining.next().ok_or_else(|| {
                DagError::Schedule(format!(
                    "slurm task `{}` is missing uploaded artifact for input `{}` index {index}",
                    dispatch.spec.id, binding.name
                ))
            })?;
            staged.push(PathBuf::from(artifact.path));
        }
        inputs_by_name.insert(binding.name.clone(), staged);
    }
    if remaining.next().is_some() {
        return Err(DagError::Schedule(format!(
            "slurm task `{}` received more uploaded artifacts than declared staged inputs",
            dispatch.spec.id
        )));
    }
    Ok(inputs_by_name)
}
/// Join a resolved (absolute) scratch dir with the sanitized task id.
/// The per-attempt remote namespace of one dispatched task.
///
/// The coordinator injects it before staging, so a retried attempt never
/// shares a directory with the attempt it replaces. Every backend that owns
/// a remote attempt directory resolves it the same way.
pub(crate) fn attempt_key(dispatch: &TaskDispatch) -> Result<String, DagError> {
    match dispatch.spec.spec.get("_autonomics_attempt_key") {
        None => Ok(dispatch.spec.id.clone()),
        Some(serde_json::Value::String(key))
            if key.len() == 72
                && key.starts_with("attempt-")
                && key[8..]
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) =>
        {
            Ok(key.clone())
        }
        Some(_) => Err(DagError::Schedule(
            "invalid remote task attempt namespace".into(),
        )),
    }
}
pub(crate) fn remote_task_dir(scratch_dir: &str, task_id: &str) -> String {
    format!(
        "{}/{}",
        scratch_dir.trim_end_matches('/'),
        sanitize_path_component(task_id)
    )
}
/// Expand a leading `~/` in a remote path against the resolved remote home,
/// resolving and caching the home once per owner. sbatch directive values
/// and single-quoted shell arguments never expand `~`, so no tilde-prefixed
/// path may cross an ssh command or a generated script — every path that
/// reaches the cluster must be absolute.
pub(crate) async fn resolve_remote_prefix(
    ssh: &SshTarget,
    path: &str,
    home: &Mutex<Option<String>>,
) -> Result<String, DagError> {
    if path != "~" && !path.starts_with("~/") {
        return Ok(path.to_string());
    }
    let rest = path.strip_prefix("~/").unwrap_or("");
    if let Some(resolved) = home.lock().unwrap().clone() {
        return Ok(join_remote_home(&resolved, rest));
    }
    let output = ensure_ssh_success(ssh.run("cd && pwd").await?, "resolve remote home")?;
    let resolved = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if resolved.is_empty() || !resolved.starts_with('/') {
        return Err(DagError::Schedule(format!(
            "cannot resolve remote home directory: {resolved:?}"
        )));
    }
    let resolved = {
        let mut guard = home.lock().unwrap();
        guard.get_or_insert(resolved).clone()
    };
    Ok(join_remote_home(&resolved, rest))
}
fn join_remote_home(home: &str, rest: &str) -> String {
    if rest.is_empty() {
        home.to_string()
    } else {
        format!("{home}/{rest}")
    }
}
fn parse_submitted_job_id(stdout: &str, task_id: &str) -> Result<String, DagError> {
    let line = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| {
            DagError::Schedule(format!(
                "sbatch returned no job id for task `{task_id}`: {stdout:?}"
            ))
        })?;
    let id = line.split(';').next().unwrap_or(line).trim();
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DagError::Schedule(format!(
            "sbatch returned an unusable job id for task `{task_id}`: {line:?}"
        )));
    }
    Ok(id.to_string())
}
fn parse_sacct_record(record: &str, expected_job_id: &str) -> Result<(String, i32), DagError> {
    let invalid = || DagError::Schedule(format!("invalid or nonterminal sacct record `{record}`"));
    let fields: Vec<_> = record.split('|').collect();
    if fields.len() != 3 || fields[0] != expected_job_id {
        return Err(invalid());
    }
    let raw_state = fields[1].trim();
    let state = if raw_state == "CANCELLED"
        || raw_state
            .strip_prefix("CANCELLED by ")
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
    {
        "CANCELLED"
    } else {
        raw_state
    };
    if !matches!(
        state,
        "COMPLETED"
            | "FAILED"
            | "TIMEOUT"
            | "CANCELLED"
            | "NODE_FAIL"
            | "OUT_OF_MEMORY"
            | "PREEMPTED"
            | "BOOT_FAIL"
            | "DEADLINE"
            | "REVOKED"
            | "TIME_LIMIT"
            | "SPECIAL_EXIT"
    ) {
        return Err(invalid());
    }
    let codes: Vec<_> = fields[2].split(':').collect();
    if codes.len() != 2
        || codes
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(invalid());
    }
    let code = codes[0].parse::<u8>().map_err(|_| invalid())?;
    let signal = codes[1].parse::<u8>().map_err(|_| invalid())?;
    if signal > 127 {
        return Err(invalid());
    }
    Ok((
        state.to_string(),
        if signal == 0 {
            i32::from(code)
        } else {
            128 + i32::from(signal)
        },
    ))
}
fn map_terminal_state(state: &str, exit_code: i32) -> (&'static str, i32) {
    match state {
        "COMPLETED" if exit_code == 0 => ("success", 0),
        "TIMEOUT" | "TIME_LIMIT" => ("timeout", 124),
        "CANCELLED" => ("cancelled", 130),
        _ => ("failed", if exit_code != 0 { exit_code } else { 1 }),
    }
}
/// Outcome of consulting the cluster after an ambiguous submission.
enum Adoption {
    /// Exactly one job matches this attempt's unique job name.
    Found(String),
    /// No candidate was visible; this is not authorization to replay submission.
    ClearToRetry,
}
/// Only the numeric base of a job id counts: `123`, `123.batch` and
/// `123.extern` are all job 123; anything else is not a job id.
fn numeric_job_base_id(raw: &str) -> Option<String> {
    let base = raw.trim().split('.').next().unwrap_or_default();
    if base.is_empty() || !base.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(base.to_string())
}
fn merge_candidates(squeue_ids: &[String], sacct_ids: &[String]) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    for id in squeue_ids.iter().chain(sacct_ids.iter()) {
        if !merged.contains(id) {
            merged.push(id.clone());
        }
    }
    merged
}
fn format_walltime(max_duration_ms: u64) -> String {
    let total_seconds = max_duration_ms.div_ceil(1000).max(1);
    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if days > 0 {
        format!("{days}-{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
}
/// Generate the sbatch job script: directives from the task resources and
/// operator settings, then the same environment contract and
/// template-rendered payload the local process transport would run.
pub(super) fn render_sbatch_script(
    dispatch: &TaskDispatch,
    process: &ProcessTaskSpec,
    task_dir: &str,
    environment: &BTreeMap<String, String>,
    inputs_by_name: &BTreeMap<String, Vec<PathBuf>>,
    settings: &SlurmSettings,
    job_name: &str,
) -> Result<String, DagError> {
    if settings.gpus == Some(0) {
        return Err(DagError::Schedule(
            "Slurm GPU count must be positive".into(),
        ));
    }
    let mut script = String::from("#!/bin/bash\n");
    script.push_str(&format!("#SBATCH --job-name={job_name}\n"));
    script.push_str(&format!("#SBATCH --partition={}\n", settings.partition));
    if let Some(gpus) = settings.gpus {
        script.push_str("#SBATCH --nodes=1\n#SBATCH --ntasks=1\n");
        script.push_str(&format!("#SBATCH --gres=gpu:{gpus}\n"));
    }
    if let Some(qos) = &settings.qos {
        script.push_str(&format!("#SBATCH --qos={qos}\n"));
    }
    if let Some(account) = &settings.account {
        script.push_str(&format!("#SBATCH --account={account}\n"));
    }
    script.push_str(&format!(
        "#SBATCH --cpus-per-task={}\n",
        dispatch.resources.cpus.unwrap_or(1)
    ));
    match dispatch.resources.memory_bytes {
        Some(memory_bytes) => script.push_str(&format!(
            "#SBATCH --mem={}M\n",
            memory_bytes.div_ceil(1024 * 1024)
        )),
        None => script.push_str(&format!("#SBATCH --mem-per-cpu={}\n", settings.mem_per_cpu)),
    }
    let walltime = dispatch
        .resources
        .max_duration_ms
        .map_or_else(|| settings.default_time.clone(), format_walltime);
    script.push_str(&format!("#SBATCH --time={walltime}\n"));
    // `task_dir` is absolute (resolved during submit), so the directive can
    // carry it literally; `%j` is Slurm's job-id filename pattern.
    script.push_str(&format!("#SBATCH --output={task_dir}/slurm-%j.out\n"));
    script.push_str(&format!("#SBATCH --error={task_dir}/slurm-%j.err\n"));
    script.push_str("\numask 077\nexport LC_ALL=C\n");
    if process.container.is_some() {
        let apptainer = settings
            .apptainer
            .as_ref()
            .ok_or_else(|| DagError::Schedule("missing operator Apptainer configuration".into()))?;
        script.push_str(&super::super::apptainer::render(
            apptainer,
            process,
            task_dir,
            environment,
            inputs_by_name,
        )?);
        return Ok(script);
    }
    for (name, value) in environment {
        script.push_str(&format!("export {name}={}\n", sh_single_quote(value)));
    }
    script.push_str(&format!("cd {} || exit 90\n", sh_single_quote(task_dir)));
    script.push_str(&super::super::process::render_process_payload(
        process,
        environment,
        inputs_by_name,
    ));
    Ok(script)
}
pub(crate) fn heredoc_delimiter(base: &str, contents: &str) -> String {
    let mut delimiter = base.to_string();
    let mut sequence = 0usize;
    while contents.lines().any(|line| line == delimiter) {
        sequence += 1;
        delimiter = format!("{base}_{sequence}");
    }
    delimiter
}
/// One ssh round trip: create the task directory, drop the manifest and the
/// generated job script (quoted heredocs — content stays literal), submit,
/// and print the job id.
pub(super) fn render_bootstrap_script(
    task_dir: &str,
    manifest_json: &str,
    sbatch_script: &str,
) -> Result<String, DagError> {
    // Bash ignores NUL bytes while reading shell input. Collision checks on
    // the original Rust string cannot safely delimit such a body.
    if [task_dir, manifest_json, sbatch_script]
        .iter()
        .any(|value| value.as_bytes().contains(&0))
    {
        return Err(DagError::Schedule(
            "Slurm bootstrap content must not contain NUL bytes".into(),
        ));
    }
    // Task bodies are data here. A task containing the delimiter must never
    // terminate its quoted heredoc and execute on the login-node bootstrap.
    let manifest_delimiter = heredoc_delimiter("AUTONOMICS_MANIFEST_EOF", manifest_json);
    let batch_delimiter = heredoc_delimiter("AUTONOMICS_SBATCH_EOF", sbatch_script);
    let quoted_dir = sh_single_quote(task_dir);
    let identity = submission::identity(task_dir, manifest_json);
    let mut script = format!(
        r#"set -e
mkdir -p {quoted_dir}
cd {quoted_dir}
# mkdir is the durable, exclusive intent: never remove it automatically.
if ! mkdir submission.intent 2>/dev/null; then
  [ "$(cat submission.identity 2>/dev/null)" = '{identity}' ] || {{ echo 'submission UNKNOWN: identity mismatch' >&2; exit 91; }}
  cat submission.job-id || {{ echo 'submission UNKNOWN: reconcile original intent' >&2; exit 92; }}
  exit 0
fi
printf '%s\n' '{identity}' > submission.identity
cat > task.json <<'{manifest_delimiter}'
"#
    );
    script.push_str(manifest_json);
    script.push_str(&format!("\n{manifest_delimiter}\n"));
    script.push_str(&format!("cat > job.sbatch <<'{batch_delimiter}'\n"));
    script.push_str(sbatch_script);
    script.push_str(&format!("{batch_delimiter}\n"));
    script.push_str(&format!(
        r#"# The scheduler comment correlates uncertain submissions without resubmitting.
sbatch --parsable --comment=autonomics-{identity} job.sbatch > submission.job-id.pending
mv submission.job-id.pending submission.job-id
cat submission.job-id
"#
    ));
    Ok(script)
}
/// Generate the remote output-collection script: for each declared port,
/// glob the declared pattern deterministically and copy each match to
/// `output-{port}-{index}`, reporting findings as protocol lines. A port
/// with no matches simply reports nothing — the transport treats a missing
/// port as the "matched no files" error, mirroring the local transport.
pub(crate) fn render_collect_script(process: &ProcessTaskSpec, task_dir: &str) -> String {
    let mut script = String::from("set -u\nexport LC_ALL=C\n");
    script.push_str(&format!("cd {} || exit 90\n", sh_single_quote(task_dir)));
    for output in &process.outputs {
        script.push_str(&format!(
            r#"sequence=0
while IFS= read -r match; do
  [ -f "$match" ] || continue
  cp -L -- "$match" "output-{port}-$sequence"
  extension="${{match##*.}}"
  [ "$extension" = "$match" ] && extension="-"
  size=$(stat -c %s "output-{port}-$sequence")
  digest=$(sha256sum "output-{port}-$sequence" | cut -d' ' -f1)
  printf 'AUTONOMICS_ARTIFACT {port} %s %s %s %s\n' "$sequence" "${{extension,,}}" "$size" "$digest"
  sequence=$((sequence + 1))
done < <(compgen -G {pattern} | sort)
"#,
            port = output.port,
            pattern = sh_single_quote(&output.pattern),
        ));
    }
    script
}
#[async_trait::async_trait]
impl TaskTransport for SlurmTaskTransport {
    fn submission_failure_may_be_running(&self, error: &DagError) -> bool {
        !matches!(
            error,
            DagError::SubmissionNotSent {
                local_ssh_stopped: true,
                ..
            }
        )
    }
    fn ensure_ready(&self) -> Result<(), DagError> {
        if let Some(recorder) = &self.recorder {
            recorder.ensure_ready()?;
        }
        Ok(())
    }
    async fn submit(
        &self,
        dispatch: TaskDispatch,
        artifacts: Vec<FileRef>,
    ) -> Result<TaskLease, DagError> {
        let (lease, job) = self.submit_job(dispatch, artifacts).await?;
        self.pending
            .lock()
            .map_err(|_| poisoned_state())?
            .insert(lease.lease_id.clone(), job);
        Ok(lease)
    }
    async fn wait(&self, lease: &TaskLease) -> Result<TaskAttemptReceipt, DagError> {
        self.wait_observed(lease, &crate::dag::node_event::NodeReporter::noop())
            .await
    }
    async fn wait_observed(
        &self,
        lease: &TaskLease,
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<TaskAttemptReceipt, DagError> {
        if self
            .cancelled
            .lock()
            .map_err(|_| poisoned_state())?
            .contains(&lease.lease_id)
        {
            return Err(DagError::Schedule(format!(
                "slurm lease `{}` is unknown, cancelled, or already consumed",
                lease.lease_id
            )));
        }
        let job = self
            .pending
            .lock()
            .map_err(|_| poisoned_state())?
            .get(&lease.lease_id)
            .cloned()
            .ok_or_else(|| {
                DagError::Schedule(format!(
                    "slurm lease `{}` is unknown, cancelled, or already consumed",
                    lease.lease_id
                ))
            })?;
        let job_id = lease.lease_id.as_str();
        if job.dispatch.spec.id != lease.task_id {
            return Err(DagError::Schedule(
                "Slurm lease task identity changed".into(),
            ));
        }
        let mut observation_scope = submission::SubmissionScope::new(job.record.clone());
        // Probe the exact lease; a scheduler-running observation is distinct
        // from payload progress and from a full stopped-subtree proof.
        // An empty queue listing still requires accounting confirmation.
        loop {
            let probe = self
                .settings
                .ssh
                .run(&format!("LC_ALL=C squeue -h -j {job_id} -o %T"))
                .await
                .and_then(|output| queue_stdout_text(output, "squeue"));
            let state = match probe {
                Ok(state) => state,
                Err(error) => {
                    reporter.worker(
                        "waiting_for_scheduler",
                        "slurm",
                        job_id,
                        "unknown",
                        "scheduler",
                    );
                    return Err(error);
                }
            };
            let state = state.trim();
            if state.is_empty() {
                reporter.worker(
                    "waiting_for_accounting",
                    "slurm",
                    job_id,
                    "unknown",
                    "scheduler",
                );
                break;
            }
            let (phase, observed) = match state {
                "RUNNING" | "COMPLETING" => ("executing", "running"),
                "PENDING" | "CONFIGURING" | "SUSPENDED" => ("waiting_for_scheduler", "pending"),
                _ => ("waiting_for_scheduler", "unknown"),
            };
            if observed != "running"
                || !self
                    .observe_worker_heartbeat(&job, job_id, reporter, phase)
                    .await
            {
                reporter.worker(phase, "slurm", job_id, observed, "scheduler");
            }
            tokio::time::sleep(self.settings.poll_interval).await;
        }
        let (state, sacct_exit) = match self.wait_terminal_state(job_id).await {
            Ok(value) => value,
            Err(error) => {
                reporter.worker(
                    "waiting_for_accounting",
                    "slurm",
                    job_id,
                    "unknown",
                    "scheduler",
                );
                return Err(error);
            }
        };
        let (status_name, exit_code) = map_terminal_state(&state, sacct_exit);
        reporter.worker(
            "collecting_outputs",
            "slurm",
            job_id,
            "terminal",
            "scheduler",
        );
        // Formal consumers require independently validated whole-allocation
        // evidence before outputs can be collected or pending state removed.
        reporter.phase(
            "checking_worker_stop",
            "verifying the remote worker subtree has stopped",
        );
        self.record_owned_stop(&job, job_id).await?;
        observation_scope.disarm();
        let mut artifacts_by_port = BTreeMap::new();
        if status_name == "success" {
            artifacts_by_port = self.collect_outputs(&job).await?;
        }
        let output_artifacts = artifacts_by_port
            .values()
            .flat_map(|artifacts| artifacts.iter().cloned())
            .collect::<Vec<_>>();
        // Accounting has confirmed termination before logs are opened. Keep
        // raw scheduler logs in place and transport bounded, hash-bound copies.
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Logs {
            stdout: Option<FileRef>,
            stderr: Option<FileRef>,
        }
        let command = format!(
            "python3 -B -c {} {} {}",
            sh_single_quote(include_str!("slurm_logs.py")),
            sh_single_quote(&job.task_dir),
            sh_single_quote(job_id)
        );
        let logs = self
            .settings
            .ssh
            .run_script_once(&command)
            .await
            .and_then(|output| ssh_stdout_text(output, "Slurm log capture"))
            .and_then(|raw| {
                serde_json::from_str::<Logs>(&raw).map_err(|error| {
                    DagError::Schedule(format!("invalid Slurm log response: {error}"))
                })
            });
        let (stdout_log, stderr_log, log_capture_error) = match logs {
            Ok(logs) => (logs.stdout, logs.stderr, None),
            Err(error) => {
                // Failure to observe logs must never replace a real exit code
                // or turn a known terminal outcome into UNKNOWN.
                tracing::warn!(task = %job.dispatch.spec.id, %error, "cannot capture Slurm logs");
                (None, None, Some(error.to_string()))
            }
        };
        self.pending
            .lock()
            .map_err(|_| poisoned_state())?
            .remove(&lease.lease_id);
        let task_manifest_path = format!("{}/task.json", job.task_dir);
        Ok(TaskAttemptReceipt {
            task_id: job.dispatch.spec.id,
            executor: "slurm".into(),
            status: status_name.into(),
            elapsed_ms: job.submitted_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
            exit_code,
            workspace: job.task_dir,
            task_manifest: FileRef {
                domain: DomainId::local(),
                path: task_manifest_path,
                format: Some("json".into()),
                fingerprint: Some(job.manifest_fingerprint),
            },
            stdout_log,
            stderr_log,
            log_capture_error,
            output_artifacts,
            output_artifacts_by_port: artifacts_by_port,
        })
    }
    /// Request cancellation and confirm both queue absence and exact terminal
    /// allocation accounting. Unknown stop retains the pending lease.
    async fn cancel(&self, lease: &TaskLease) -> Result<(), DagError> {
        if lease.lease_id.is_empty() || !lease.lease_id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(DagError::Schedule("invalid slurm lease id".into()));
        }
        let job = self
            .pending
            .lock()
            .map_err(|_| poisoned_state())?
            .get(&lease.lease_id)
            .cloned();
        let mut observation_scope =
            submission::SubmissionScope::new(job.as_ref().and_then(|j| j.record.clone()));
        if self.recorder.is_some() {
            let job = job.as_ref().ok_or_else(|| {
                DagError::Schedule("recorded cancellation has no owned pending job".into())
            })?;
            if job.dispatch.spec.id != lease.task_id {
                return Err(DagError::Schedule(
                    "Slurm cancellation task identity changed".into(),
                ));
            }
        }
        let budget = Duration::from_secs(60).min(self.settings.poll_interval.saturating_mul(20));
        let deadline = std::time::Instant::now() + budget;
        loop {
            // Recheck before *each* scancel. A changed/reused scheduler ID
            // must not become another cancellation just because an earlier
            // observation belonged to us.
            if self.recorder.is_some() {
                let observed = self
                    .inspect_submission_intent(&job.as_ref().unwrap().intent)
                    .await?;
                if observed.job_id.as_deref() != Some(lease.lease_id.as_str()) {
                    return Err(DagError::Schedule(
                        "Slurm cancellation ownership remains unknown".into(),
                    ));
                }
            }
            // scancel is idempotent; a job that already stopped may reject it.
            let _ = self
                .settings
                .ssh
                .run(&format!("scancel {}", lease.lease_id))
                .await;
            let queue = self
                .settings
                .ssh
                .run(&format!("LC_ALL=C squeue -h -j {} -o %T", lease.lease_id))
                .await
                .and_then(|output| queue_stdout_text(output, "cancel queue confirmation"));
            let accounting = self
                .settings
                .ssh
                .run(&format!(
                    "sacct -j {} -n -P -X -o JobIDRaw,State%40,ExitCode",
                    lease.lease_id
                ))
                .await
                .and_then(|output| ssh_stdout_text(output, "cancel accounting confirmation"));
            let terminal = accounting
                .ok()
                .and_then(|text| {
                    let rows: Vec<_> = text
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .collect();
                    if rows.len() == 1 {
                        parse_sacct_record(rows[0], &lease.lease_id).ok()
                    } else {
                        None
                    }
                })
                .is_some();
            if terminal && queue.is_ok_and(|text| text.trim().is_empty()) {
                if let Some(job) = &job {
                    self.record_owned_stop(job, &lease.lease_id).await?;
                }
                observation_scope.disarm();
                self.pending
                    .lock()
                    .map_err(|_| poisoned_state())?
                    .remove(&lease.lease_id);
                self.cancelled
                    .lock()
                    .map_err(|_| poisoned_state())?
                    .insert(lease.lease_id.clone());
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(DagError::Schedule(format!(
                    "cancel requested but stop unconfirmed for slurm job `{}` within {budget:?}; retain remote workspace and pending lease",
                    lease.lease_id
                )));
            }
            tokio::time::sleep(self.settings.poll_interval).await;
        }
    }
}
