//! Direct SSH execution: one task, one remote host, no scheduler.
//!
//! The scheduler-free substrate already exists — [`SshTarget`] runs remote
//! commands and [`SshArtifactStore`] moves artifacts over the same ssh
//! channel. This module adds the one missing piece: a [`TaskTransport`]
//! that starts the task on the remote host and derives its terminal state
//! from the remote process itself rather than from a scheduler's accounting.
//!
//! Without a scheduler there is no non-idempotent submission. The launch
//! script takes an exclusive directory lock, so replaying it after a lost
//! connection can never start the task twice. That removes the entire
//! uncertain-submission protocol the Slurm backend needs: a lost connection
//! here only means "read the remote state file", never "a job may exist and
//! must be adopted".
//!
//! The remote attempt directory is the durable evidence:
//!
//! ```text
//! <scratch>/<attempt-key>/
//!   task.json      manifest
//!   launch.sh      supervisor
//!   run.pid        "PID PGID"
//!   exit.code      terminal exit code, written atomically
//!   stdout.log     task stdout
//!   stderr.log     task stderr
//!   attempt.lock   exclusive launch marker
//! ```
//!
//! `exit.code` exists exactly when the supervisor finished the payload and
//! recorded its status. A supervisor that dies before writing it leaves the
//! terminal state unknown, and the transport reports that instead of
//! guessing an exit code.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::contract::{TaskAttemptReceipt, TaskDispatch, task_timeout};
use super::process::{ProcessTaskSpec, parse_process_task, process_task_environment};
use super::slurm::{
    SshTarget, attempt_key, ensure_ssh_success, hex_encode, match_uploaded_inputs,
    remote_task_dir, render_collect_script, resolve_remote_prefix, sh_single_quote,
    ssh_stdout_text,
};
use super::traits::{TaskLease, TaskTransport};
use crate::dag::DagError;
use crate::dag::node_event::NodeReporter;
use crate::value::{FileFingerprint, FileRef};

/// Operator-owned settings for the direct SSH backend.
///
/// There is no partition, account, QOS or GPU allocation here: without a
/// scheduler there is nothing to allocate. A resource request on a direct
/// target is a request, never a guarantee.
#[derive(Debug, Clone)]
pub struct SshSettings {
    /// ssh destination of the host that runs the task.
    pub ssh: SshTarget,
    /// Remote root for task workspaces; a leading `~/` expands against the
    /// remote home. Every remote path is absolute before it reaches a
    /// command, because a quoted shell argument never expands `~`.
    pub scratch_dir: String,
    /// Wall-clock bound applied when a task requests no explicit duration.
    pub default_time: Duration,
}

#[derive(Clone)]
struct PendingSshAttempt {
    dispatch: TaskDispatch,
    process: ProcessTaskSpec,
    task_dir: String,
    /// Identity of the exact heredoc payload admitted to the remote host. Its
    /// trailing LF is part of `task.json`; the POSIX path stays mutable.
    manifest_fingerprint: FileFingerprint,
    submitted_at: Instant,
}

impl std::fmt::Debug for PendingSshAttempt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingSshAttempt")
            .field("task_id", &self.dispatch.spec.id)
            .field("task_dir", &self.task_dir)
            .finish()
    }
}

/// A [`TaskTransport`] that runs `process` tasks directly on a remote host
/// reached over ssh, with no scheduler in the path.
#[derive(Debug)]
pub struct SshTaskTransport {
    settings: SshSettings,
    pending: Mutex<BTreeMap<String, PendingSshAttempt>>,
    cancelled: Mutex<BTreeSet<String>>,
    scratch_home: Mutex<Option<String>>,
}

impl SshTaskTransport {
    pub fn new(settings: SshSettings) -> Self {
        Self {
            settings,
            pending: Mutex::new(BTreeMap::new()),
            cancelled: Mutex::new(BTreeSet::new()),
            scratch_home: Mutex::new(None),
        }
    }

    fn poisoned_state() -> DagError {
        DagError::Schedule("ssh transport state poisoned".into())
    }

    /// The wall-clock horizon of one attempt: the task's own request when it
    /// has one, the operator default otherwise.
    fn horizon(&self, dispatch: &TaskDispatch) -> Duration {
        task_timeout(&dispatch.resources).unwrap_or(self.settings.default_time)
    }

    /// Start the task on the remote host.
    ///
    /// The whole launch is one ssh round trip: create the attempt directory,
    /// take the exclusive lock, drop the manifest and the supervisor, and
    /// detach the supervisor into its own session so it survives the
    /// connection closing.
    async fn launch(
        &self,
        process: &ProcessTaskSpec,
        task_dir: &str,
        manifest: &str,
        environment: &BTreeMap<String, String>,
        inputs_by_name: &BTreeMap<String, Vec<PathBuf>>,
    ) -> Result<(), DagError> {
        let script = render_launch_script(manifest, process, task_dir, environment, inputs_by_name);
        let output = self.settings.ssh.run_script_once(&script).await?;
        ensure_ssh_success(output, "ssh task launch").map(|_| ())
    }

    /// Read the terminal exit code of a launched attempt.
    ///
    /// The wait is a single blocking ssh call: the remote side polls for the
    /// supervisor's atomic `exit.code` write. One connection per attempt
    /// keeps the remote side free of a polling loop and gives the exact exit
    /// status without a scheduler's accounting.
    async fn await_exit_code(
        &self,
        attempt: &PendingSshAttempt,
        reporter: &NodeReporter,
    ) -> Result<i32, DagError> {
        let exit_path = format!("{}/exit.code", attempt.task_dir);
        let wait = format!(
            "set -u\nwhile [ ! -s {path} ]; do sleep 1; done\ncat -- {path}\n",
            path = sh_single_quote(&exit_path)
        );
        let horizon = self.horizon(&attempt.dispatch);
        // The transport waits a little past the task's own horizon so the
        // supervisor still gets to record its exit code; the executor's own
        // deadline remains the authority for cancelling.
        let deadline = horizon + Duration::from_secs(30);
        // Every arm below returns, so no loop is needed here.
        match self
            .settings
            .ssh
            .run_script_with_timeout(&wait, deadline)
            .await
        {
            Ok(output) => {
                let stdout = ssh_stdout_text(output, "ssh exit code")?;
                return parse_exit_code(&stdout);
            }
            Err(error) => {
                // The connection may have died while the task kept
                // running. The state file is the durable answer: it is
                // written by the supervisor, not by this connection.
                match self.read_exit_code(&exit_path).await {
                    Ok(code) => return Ok(code),
                    Err(read_error) => {
                        reporter.worker(
                            "waiting_for_remote_task",
                            "ssh",
                            &attempt.dispatch.spec.id,
                            "unknown",
                            "remote",
                        );
                        return Err(DagError::Schedule(format!(
                            "ssh wait for `{}` failed and its exit code is unreadable: {error}; {read_error}",
                            attempt.dispatch.spec.id
                        )));
                    }
                }
            }
        }
    }

    /// One bounded, read-only probe of the supervisor's exit code.
    async fn read_exit_code(&self, exit_path: &str) -> Result<i32, DagError> {
        let probe = format!(
            "set -u\nif [ -s {path} ]; then cat -- {path}; else exit 3; fi\n",
            path = sh_single_quote(exit_path)
        );
        let output = self
            .settings
            .ssh
            .run_script_with_timeout(&probe, Duration::from_secs(30))
            .await?;
        if !output.status.success() {
            return Err(DagError::Schedule(
                "ssh exit code is not yet recorded".into(),
            ));
        }
        parse_exit_code(&ssh_stdout_text(output, "ssh exit code")?)
    }

    /// Collect the declared outputs on the remote side.
    ///
    /// The glob protocol is the one the Slurm backend already uses: the
    /// remote side names each match, reports its real size and digest, and
    /// the coordinator downloads them through the artifact store.
    async fn collect_outputs(
        &self,
        attempt: &PendingSshAttempt,
    ) -> Result<BTreeMap<u8, Vec<FileRef>>, DagError> {
        let script = render_collect_script(&attempt.process, &attempt.task_dir);
        let stdout = ssh_stdout_text(
            self.settings.ssh.run_script_once(&script).await?,
            "ssh output collection",
        )?;
        let mut by_port: BTreeMap<u8, Vec<FileRef>> = BTreeMap::new();
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
            by_port
                .entry(port)
                .or_default()
                .push(FileRef {
                    path: format!("{}/output-{port}-{index}", attempt.task_dir),
                    format,
                    fingerprint,
                });
        }
        for output in &attempt.process.outputs {
            if !by_port.contains_key(&output.port) {
                return Err(DagError::Schedule(format!(
                    "ssh output port {} matched no files for `{}`",
                    output.port, output.pattern
                )));
            }
        }
        Ok(by_port)
    }

    /// Measure the two remote log files so the receipt carries real
    /// fingerprints. The coordinator never reads a log it cannot bound.
    async fn capture_logs(
        &self,
        attempt: &PendingSshAttempt,
    ) -> (Option<FileRef>, Option<FileRef>, Option<String>) {
        let dir = sh_single_quote(&attempt.task_dir);
        let mut script = String::from("set -u\nfor stream in stdout stderr; do\n");
        script.push_str("  file=");
        script.push_str(&dir);
        script.push_str("/$stream.log\n");
        script.push_str("  [ -f \"$file\" ] || continue\n");
        script.push_str("  size=$(stat -c %s -- \"$file\") || continue\n");
        script.push_str("  digest=$(sha256sum -- \"$file\" | cut -d' ' -f1) || continue\n");
        script.push_str(
            "  printf 'AUTONOMICS_LOG %s %s %s\\n' \"$stream\" \"$size\" \"$digest\"\ndone\n",
        );
        let measured = self
            .settings
            .ssh
            .run_script_once(&script)
            .await
            .and_then(|output| ssh_stdout_text(output, "ssh log measurement"));
        match measured {
            Ok(text) => (
                parse_log_reference(&text, "stdout", &attempt.task_dir),
                parse_log_reference(&text, "stderr", &attempt.task_dir),
                None,
            ),
            Err(error) => {
                // A log that cannot be measured must never replace a known
                // terminal outcome.
                tracing::warn!(task = %attempt.dispatch.spec.id, %error, "cannot measure ssh logs");
                (None, None, Some(error.to_string()))
            }
        }
    }

    /// Stop a running attempt and prove it stopped.
    ///
    /// There is no scheduler job id to cancel, so the supervisor's recorded
    /// process group is the handle. The proof is weaker than a scheduler's
    /// whole-subtree accounting: a descendant that escaped the group is not
    /// covered, and the transport says so rather than overstating it.
    async fn stop_attempt(&self, attempt: &PendingSshAttempt) -> Result<(), DagError> {
        let exit_path = format!("{}/exit.code", attempt.task_dir);
        // A task that already reached its terminal state has nothing to
        // stop, and overwriting its recorded exit code would misreport it.
        if self.read_exit_code(&exit_path).await.is_ok() {
            return Ok(());
        }
        let (pid, pgid) = self.await_process_identity(attempt).await?;
        let group = pgid == pid;

        let budget = Duration::from_secs(60);
        let deadline = Instant::now() + budget;
        // A supervisor that got its own session owns its process group, so
        // the group is the handle. Without one, only the supervisor itself
        // can be addressed and descendants may survive.
        let target = if group {
            format!("-{pgid}")
        } else {
            format!("{pid}")
        };
        let terminate = format!("kill -TERM {target} 2>/dev/null || true\n");
        self.settings.ssh.run_script_once(&terminate).await?;

        while Instant::now() < deadline {
            if !self.process_alive(pid).await? {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if self.process_alive(pid).await? {
            let kill = format!("kill -KILL {target} 2>/dev/null || true\n");
            self.settings.ssh.run_script_once(&kill).await?;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if self.process_alive(pid).await? {
            return Err(DagError::Schedule(format!(
                "cancel requested but stop unconfirmed for ssh attempt `{}` within {budget:?}; retain remote workspace and pending lease",
                attempt.dispatch.spec.id
            )));
        }
        // The supervisor is gone, so this cannot be overwritten by a late
        // write. A later wait reads the cancellation, not a stale success.
        let cancel = format!(
            "set -u\nprintf '130\\n' > {path}.tmp && mv -f {path}.tmp {path}\n",
            path = sh_single_quote(&exit_path)
        );
        self.settings
            .ssh
            .run_script_once(&cancel)
            .await
            .and_then(|output| ensure_ssh_success(output, "ssh cancel record"))
            .map(|_| ())
            .map_err(|error| {
                DagError::Schedule(format!(
                    "ssh attempt `{}` stopped but its cancellation could not be recorded: {error}",
                    attempt.dispatch.spec.id
                ))
            })
    }

    /// Read the supervisor's recorded process identity.
    ///
    /// The supervisor writes it before the payload starts, so a cancel that
    /// arrives immediately after the launch still finds it; the wait is
    /// bounded because an attempt that never started has no identity at all.
    async fn await_process_identity(
        &self,
        attempt: &PendingSshAttempt,
    ) -> Result<(i64, i64), DagError> {
        let pid_path = format!("{}/run.pid", attempt.task_dir);
        let read = format!(
            "set -u\nif [ -s {path} ]; then cat -- {path}; else exit 3; fi\n",
            path = sh_single_quote(&pid_path)
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let output = self
                .settings
                .ssh
                .run_script_with_timeout(&read, Duration::from_secs(30))
                .await?;
            if output.status.success() {
                let raw = ssh_stdout_text(output, "ssh process identity")?;
                let mut fields = raw.split_whitespace();
                let pid: i64 = fields
                    .next()
                    .and_then(|value| value.parse().ok())
                    .ok_or_else(|| {
                        DagError::Schedule("ssh attempt recorded no process id".into())
                    })?;
                let pgid: i64 = fields.next().and_then(|value| value.parse().ok()).unwrap_or(pid);
                return Ok((pid, pgid));
            }
            if Instant::now() >= deadline {
                return Err(DagError::Schedule(
                    "ssh attempt has no recorded process identity; it may never have started"
                        .into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Whether the recorded process still exists on the remote host.
    async fn process_alive(&self, pid: i64) -> Result<bool, DagError> {
        let probe = format!("kill -0 {pid} 2>/dev/null && printf 'alive' || printf 'gone'\n");
        let output = self
            .settings
            .ssh
            .run_script_with_timeout(&probe, Duration::from_secs(30))
            .await?;
        Ok(ssh_stdout_text(output, "ssh process probe")?.trim() == "alive")
    }
}

#[async_trait::async_trait]
impl TaskTransport for SshTaskTransport {
    async fn submit(
        &self,
        dispatch: TaskDispatch,
        artifacts: Vec<FileRef>,
    ) -> Result<TaskLease, DagError> {
        dispatch.resources.validate()?;
        let process = parse_process_task(&dispatch)?;
        if process.container.is_some() {
            return Err(DagError::Schedule(
                "container execution is not available on the direct SSH backend; use a scheduler-backed target for container workloads".into(),
            ));
        }
        // The worker identifies its own inputs by the uploaded artifact
        // identities, not by the staged paths they occupy.
        let mut input_artifacts_by_name: BTreeMap<String, Vec<FileRef>> = BTreeMap::new();
        let mut remaining = artifacts.iter();
        for binding in &dispatch.spec.inputs {
            input_artifacts_by_name.insert(
                binding.name.clone(),
                remaining
                    .by_ref()
                    .take(binding.staged_paths.len())
                    .cloned()
                    .collect(),
            );
        }
        let inputs_by_name = match_uploaded_inputs(&dispatch, artifacts)?;
        let scratch = resolve_remote_prefix(
            &self.settings.ssh,
            &self.settings.scratch_dir,
            &self.scratch_home,
        )
        .await?;
        let key = attempt_key(&dispatch)?;
        let task_dir = remote_task_dir(&scratch, &key);

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

        let mut dispatch = dispatch;
        dispatch.spec.spec["_autonomics_input_artifacts"] =
            serde_json::to_value(&input_artifacts_by_name)
                .map_err(|error| DagError::Schedule(error.to_string()))?;
        let manifest = serde_json::to_string_pretty(&dispatch)
            .map_err(|error| DagError::Schedule(format!("cannot serialize ssh task manifest: {error}")))?;
        // A quoted heredoc preserves every manifest byte and appends exactly
        // one LF before its delimiter. Bind the receipt to those submitted
        // bytes, never to a hash learned by reopening the remote path.
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

        self.launch(
            &process,
            &task_dir,
            &manifest,
            &environment,
            &inputs_by_name,
        )
        .await?;

        let lease = TaskLease {
            lease_id: key.clone(),
            task_id: dispatch.spec.id.clone(),
        };
        self.pending
            .lock()
            .map_err(|_| Self::poisoned_state())?
            .insert(
                lease.lease_id.clone(),
                PendingSshAttempt {
                    dispatch,
                    process,
                    task_dir,
                    manifest_fingerprint,
                    submitted_at: Instant::now(),
                },
            );
        Ok(lease)
    }

    async fn wait(&self, lease: &TaskLease) -> Result<TaskAttemptReceipt, DagError> {
        self.wait_observed(lease, &NodeReporter::noop()).await
    }

    async fn wait_observed(
        &self,
        lease: &TaskLease,
        reporter: &NodeReporter,
    ) -> Result<TaskAttemptReceipt, DagError> {
        if self
            .cancelled
            .lock()
            .map_err(|_| Self::poisoned_state())?
            .contains(&lease.lease_id)
        {
            return Err(DagError::Schedule(format!(
                "ssh lease `{}` is unknown, cancelled, or already consumed",
                lease.lease_id
            )));
        }
        let attempt = self
            .pending
            .lock()
            .map_err(|_| Self::poisoned_state())?
            .get(&lease.lease_id)
            .cloned()
            .ok_or_else(|| {
                DagError::Schedule(format!(
                    "ssh lease `{}` is unknown, cancelled, or already consumed",
                    lease.lease_id
                ))
            })?;
        if attempt.dispatch.spec.id != lease.task_id {
            return Err(DagError::Schedule("ssh lease task identity changed".into()));
        }

        reporter.worker(
            "executing",
            "ssh",
            &lease.lease_id,
            "running",
            "remote",
        );
        let exit_code = self.await_exit_code(&attempt, reporter).await?;
        let (status_name, exit_code) = map_exit_code(exit_code);
        reporter.worker(
            "collecting_outputs",
            "ssh",
            &lease.lease_id,
            "terminal",
            "remote",
        );

        let mut artifacts_by_port = BTreeMap::new();
        if status_name == "success" {
            artifacts_by_port = self.collect_outputs(&attempt).await?;
        }
        let output_artifacts = artifacts_by_port
            .values()
            .flat_map(|artifacts| artifacts.iter().cloned())
            .collect::<Vec<_>>();

        let (stdout_log, stderr_log, log_capture_error) = self.capture_logs(&attempt).await;

        self.pending
            .lock()
            .map_err(|_| Self::poisoned_state())?
            .remove(&lease.lease_id);
        Ok(TaskAttemptReceipt {
            task_id: attempt.dispatch.spec.id,
            executor: "ssh".into(),
            status: status_name.into(),
            elapsed_ms: attempt
                .submitted_at
                .elapsed()
                .as_millis()
                .min(u64::MAX as u128) as u64,
            exit_code,
            workspace: attempt.task_dir.clone(),
            task_manifest: FileRef {
                path: format!("{}/task.json", attempt.task_dir),
                format: Some("json".into()),
                fingerprint: Some(attempt.manifest_fingerprint.clone()),
            },
            stdout_log,
            stderr_log,
            log_capture_error,
            output_artifacts,
            output_artifacts_by_port: artifacts_by_port,
        })
    }

    async fn cancel(&self, lease: &TaskLease) -> Result<(), DagError> {
        let attempt = self
            .pending
            .lock()
            .map_err(|_| Self::poisoned_state())?
            .get(&lease.lease_id)
            .cloned()
            .ok_or_else(|| {
                DagError::Schedule(format!(
                    "ssh lease `{}` is unknown, cancelled, or already consumed",
                    lease.lease_id
                ))
            })?;
        self.stop_attempt(&attempt).await?;
        self.cancelled
            .lock()
            .map_err(|_| Self::poisoned_state())?
            .insert(lease.lease_id.clone());
        self.pending
            .lock()
            .map_err(|_| Self::poisoned_state())?
            .remove(&lease.lease_id);
        Ok(())
    }
}

/// Render the one-shot remote launch script.
///
/// The outer script is idempotent: the exclusive `attempt.lock` directory is
/// the marker, so a replay after a lost connection observes the running
/// attempt instead of starting a second one. The inner script is the
/// supervisor — it records its own process identity, runs the payload and
/// writes the exit code atomically.
fn render_launch_script(
    manifest: &str,
    process: &ProcessTaskSpec,
    task_dir: &str,
    environment: &BTreeMap<String, String>,
    inputs_by_name: &BTreeMap<String, Vec<PathBuf>>,
) -> String {
    let supervisor = render_supervisor(process, task_dir, environment, inputs_by_name);
    let manifest_delimiter = super::slurm::heredoc_delimiter("AUTONOMICS_MANIFEST_EOF", manifest);
    let supervisor_delimiter = super::slurm::heredoc_delimiter("AUTONOMICS_LAUNCH_EOF", &supervisor);
    let mut script = String::from("set -eu\numask 077\nexport LC_ALL=C\n");
    script.push_str(&format!("task_dir={}\n", sh_single_quote(task_dir)));
    script.push_str("mkdir -p -- \"$task_dir\"\ncd -- \"$task_dir\"\n");
    script.push_str(
        "# The lock is the exclusive launch marker: a replay after a lost\n\
         # connection must never start a second attempt.\n\
         if ! mkdir -- \"$task_dir/attempt.lock\" 2>/dev/null; then exit 0; fi\n",
    );
    script.push_str(&format!(
        "cat > \"$task_dir/task.json\" <<'{delimiter}'\n{manifest}\n{delimiter}\n",
        delimiter = manifest_delimiter,
        manifest = manifest
    ));
    script.push_str(&format!(
        "cat > \"$task_dir/launch.sh\" <<'{delimiter}'\n{supervisor}\n{delimiter}\n",
        delimiter = supervisor_delimiter,
        supervisor = supervisor
    ));
    script.push_str("chmod 0700 \"$task_dir/launch.sh\"\n");
    // The supervisor must outlive this connection: it gets its own session
    // so no controlling terminal can signal it when the ssh channel closes.
    script.push_str(
        "if command -v setsid >/dev/null 2>&1; then\n\
         \x20 setsid bash \"$task_dir/launch.sh\" </dev/null >\"$task_dir/stdout.log\" 2>\"$task_dir/stderr.log\" &\n\
         else\n\
         \x20 nohup bash \"$task_dir/launch.sh\" </dev/null >\"$task_dir/stdout.log\" 2>\"$task_dir/stderr.log\" &\n\
         fi\n\
         exit 0\n",
    );
    script
}

/// Render the supervisor that owns the payload's lifetime.
///
/// It records its own pid and process group before the payload starts, so a
/// cancel can address the whole group, and it writes the payload's exit code
/// atomically so a reader never observes a half-written status.
fn render_supervisor(
    process: &ProcessTaskSpec,
    task_dir: &str,
    environment: &BTreeMap<String, String>,
    inputs_by_name: &BTreeMap<String, Vec<PathBuf>>,
) -> String {
    let mut script = String::from("#!/bin/bash\numask 077\n");
    script.push_str(&format!("task_dir={}\n", sh_single_quote(task_dir)));
    // `$$` is this supervisor's pid. The process group comes from procfs
    // because `ps` is not guaranteed on a compute host.
    script.push_str(
        "proc_stat=$(cat /proc/$$/stat 2>/dev/null || printf '')\n\
         proc_rest=${proc_stat#*) }\n\
         pgid=$(printf '%s' \"$proc_rest\" | cut -d' ' -f3)\n\
         printf '%s %s\\n' \"$$\" \"${pgid:-$$}\" > \"$task_dir/run.pid.tmp\"\n\
         mv -f \"$task_dir/run.pid.tmp\" \"$task_dir/run.pid\"\n",
    );
    script.push_str("cd -- \"$task_dir\" || exit 90\n");
    // The payload's own status must never abort the supervisor before it
    // records that status.
    script.push_str("set +e\n");
    for (name, value) in environment {
        script.push_str(&format!("export {name}={}\n", sh_single_quote(value)));
    }
    // The payload runs in a subshell: its own `exit` must terminate the
    // payload, not the supervisor, or the exit code would never be recorded.
    script.push_str("(\n");
    script.push_str(&super::process::render_process_payload(
        process,
        environment,
        inputs_by_name,
    ));
    script.push_str(")\n");
    script.push_str("code=$?\nset -e\n");
    script.push_str(
        "printf '%s\\n' \"$code\" > \"$task_dir/exit.code.tmp\"\n\
         mv -f \"$task_dir/exit.code.tmp\" \"$task_dir/exit.code\"\n",
    );
    script
}

/// Read one exit code from the supervisor's atomic write.
fn parse_exit_code(raw: &str) -> Result<i32, DagError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(DagError::Schedule(
            "ssh attempt recorded no exit code".into(),
        ));
    }
    text.parse::<i32>().map_err(|error| {
        DagError::Schedule(format!("ssh attempt recorded an unreadable exit code: {error}"))
    })
}

/// Map a remote exit code onto the same terminal vocabulary the Slurm
/// backend publishes.
fn map_exit_code(exit_code: i32) -> (&'static str, i32) {
    match exit_code {
        0 => ("success", 0),
        124 => ("timeout", 124),
        130 => ("cancelled", 130),
        code => ("failed", code),
    }
}

/// Read one measured log reference out of the measurement protocol.
fn parse_log_reference(raw: &str, stream: &str, task_dir: &str) -> Option<FileRef> {
    for line in raw.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("AUTONOMICS_LOG") {
            continue;
        }
        if fields.next() != Some(stream) {
            continue;
        }
        let size = fields.next().and_then(|value| value.parse::<u64>().ok())?;
        let digest = fields.next()?;
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        return Some(FileRef {
            path: format!("{task_dir}/{stream}.log"),
            format: Some("text".into()),
            fingerprint: Some(FileFingerprint {
                size,
                mtime_ns: 0,
                content_hash: Some(digest.to_ascii_lowercase()),
                immutable_remote: false,
            }),
        });
    }
    None
}
