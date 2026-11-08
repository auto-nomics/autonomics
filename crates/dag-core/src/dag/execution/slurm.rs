//! The Slurm task backend: a transport that submits `process` tasks as
//! sbatch jobs over ssh to an HPC login node, waits for terminal state via
//! `squeue`/`sacct`, and moves artifacts through the same ssh channel.
//!
//! Mirrors [`super::process::ProcessTaskTransport`]: the remote job sees the
//! identical `AUTONOMICS_*` environment contract, staged inputs, and
//! output-glob collection, but runs under Slurm admission instead of the
//! local resource budget. Wrap the transport and [`SshArtifactStore`] in the
//! existing [`RemoteTaskExecutor`] (see [`slurm_backed_executor`]) — the
//! coordinator protocol is transport-agnostic.
//!
//! Everything here is host/operator-owned code: credentials stay in the
//! operator's ssh setup (`BatchMode` and strict host keys are forced, so a
//! missing key fails loudly instead of hanging on a password prompt).
//! Multiplexing and persistence are disabled for each invocation: a shared
//! background master must not outlive the task transport's managed lifetime.
//! No agent-facing surface can reach this module.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::contract::{TaskAttemptReceipt, TaskDispatch, TaskExecution};
use super::process::{
    ProcessTaskSpec, parse_process_task, process_task_environment, render_process_script,
    sanitize_path_component,
};
use super::remote::RemoteTaskExecutor;
use super::traits::{TaskArtifactStore, TaskExecutor, TaskLease, TaskTransport};
use crate::dag::DagError;
use crate::dag::node_event::JobResult;
use crate::value::{DomainId, FileFingerprint, FileRef};

#[path = "slurm_submission.rs"]
mod submission;
pub use submission::{
    SlurmRecordedSubmission, SlurmSubmissionIntent, SlurmSubmissionObservation,
    SlurmSubmissionRecorder,
};

// ── ssh plumbing ─────────────────────────────────────────────────────────

/// Where and how to reach the cluster login node over ssh.
#[derive(Debug, Clone)]
pub struct SshTarget {
    endpoint: Option<vfs::SshEndpoint>,
    shared_endpoints: BTreeMap<String, vfs::SshEndpoint>,
    program: String,
    host: String,
    connect_timeout: Duration,
    command_timeout: Duration,
    transfer_timeout: Duration,
    /// Keepalive cadence for connections that outlive a short command. Absent
    /// means every connection is short-lived and needs no liveness probing.
    keepalive: Option<(Duration, u32)>,
    pinned_configuration: Option<(FileRef, Vec<FileRef>)>,
}

impl SshTarget {
    pub fn new(program: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            endpoint: None,
            shared_endpoints: BTreeMap::new(),
            program: program.into(),
            host: host.into(),
            connect_timeout: Duration::from_secs(10),
            command_timeout: Duration::from_secs(120),
            transfer_timeout: Duration::from_secs(3600),
            keepalive: None,
            pinned_configuration: None,
        }
    }

    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn with_endpoint(mut self, endpoint: vfs::SshEndpoint) -> Self {
        self.endpoint = Some(endpoint);
        self
    }

    /// Operator-declared data endpoints for shared-home identity checks.
    pub fn with_shared_endpoints(mut self, endpoints: BTreeMap<String, vfs::SshEndpoint>) -> Self {
        self.shared_endpoints = endpoints;
        self
    }

    fn verify_endpoints(&self) -> Result<(), DagError> {
        for endpoint in self.endpoint.iter().chain(self.shared_endpoints.values()) {
            endpoint
                .validate()
                .map_err(|_| DagError::Schedule("invalid frozen Slurm endpoint identity".into()))?;
        }
        Ok(())
    }

    pub fn with_command_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = timeout;
        self
    }

    pub fn with_transfer_timeout(mut self, timeout: Duration) -> Self {
        self.transfer_timeout = timeout;
        self
    }

    /// Probe liveness on connections that may stay open for a long task.
    ///
    /// A short command needs no keepalive, and every transport-managed
    /// connection is short. A transport that waits on a remote task for hours
    /// needs one: without it a silent network drop leaves the local side
    /// waiting on a socket that will never answer again.
    pub fn with_keepalive(mut self, interval: Duration, count: u32) -> Self {
        self.keepalive = Some((interval, count.max(1)));
        self
    }

    /// An operator-owned private configuration and its public dependencies.
    /// Credential bytes are never copied here. Recheck the same closed files
    /// before every command, retry and artifact transfer; never fall back to
    /// the process user's ambient SSH configuration.
    pub fn with_pinned_configuration(
        mut self,
        configuration: FileRef,
        dependencies: Vec<FileRef>,
    ) -> Result<Self, DagError> {
        if dependencies.len() > 8 {
            return Err(DagError::Schedule(
                "too many SSH profile dependencies".into(),
            ));
        }
        self.pinned_configuration = Some((configuration, dependencies));
        self.verify_pinned_configuration()?;
        Ok(self)
    }

    #[cfg(unix)]
    fn verify_pinned_configuration(&self) -> Result<(), DagError> {
        let Some((configuration, dependencies)) = &self.pinned_configuration else {
            return Ok(());
        };
        use std::io::Read;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        for file in std::iter::once(configuration).chain(dependencies) {
            let bad = || {
                DagError::Schedule("pinned private SSH profile changed or is unavailable".into())
            };
            let p = Path::new(&file.path);
            let fp = file.fingerprint.as_ref().ok_or_else(bad)?;
            let sha = fp.content_hash.as_ref().ok_or_else(bad)?;
            if !p.is_absolute()
                || p.canonicalize().map_err(|_| bad())? != p
                || fp.size == 0
                || fp.size > 65536
                || sha.len() != 64
                || !sha
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err(bad());
            }
            let parent = p.parent().ok_or_else(bad)?.metadata().map_err(|_| bad())?;
            let mut fd = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(p)
                .map_err(|_| bad())?;
            let before = fd.metadata().map_err(|_| bad())?;
            let uid = unsafe { libc::geteuid() };
            if !before.is_file()
                || before.uid() != uid
                || before.mode() & 0o077 != 0
                || parent.uid() != uid
                || parent.mode() & 0o077 != 0
                || before.len() != fp.size
            {
                return Err(bad());
            }
            let mut raw = Vec::new();
            fd.by_ref()
                .take(65537)
                .read_to_end(&mut raw)
                .map_err(|_| bad())?;
            let after = fd.metadata().map_err(|_| bad())?;
            if raw.len() as u64 != fp.size
                || hex_encode(&Sha256::digest(&raw)) != *sha
                || before.mtime_nsec() != after.mtime_nsec()
                || before.mtime() != after.mtime()
                || before.ctime_nsec() != after.ctime_nsec()
                || before.ctime() != after.ctime()
                || before.len() != after.len()
            {
                return Err(bad());
            }
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn verify_pinned_configuration(&self) -> Result<(), DagError> {
        if self.pinned_configuration.is_some() {
            Err(DagError::Schedule(
                "private pinned SSH profiles require Unix".into(),
            ))
        } else {
            Ok(())
        }
    }

    // Every command and artifact transfer must share the same SSH policy.
    // Keeping this construction in one place prevents a transfer path from
    // inheriting an independently managed persistent master from ~/.ssh/config.
    fn command(&self) -> Result<tokio::process::Command, DagError> {
        self.verify_pinned_configuration()?;
        self.verify_endpoints()?;
        let connect = format!("ConnectTimeout={}", self.connect_timeout.as_secs().max(1));
        let mut command = tokio::process::Command::new(&self.program);
        if let Some((configuration, _)) = &self.pinned_configuration {
            command.args(["-F", &configuration.path]);
        }
        if let Some(endpoint) = &self.endpoint {
            command.args(endpoint.options());
        }
        let mut options: Vec<String> = vec![
            "BatchMode=yes".into(),
            "LogLevel=ERROR".into(),
            "StrictHostKeyChecking=yes".into(),
            "ControlMaster=no".into(),
            "ControlPath=none".into(),
            "ControlPersist=no".into(),
            connect,
        ];
        if let Some((interval, count)) = &self.keepalive {
            options.push(format!("ServerAliveInterval={}", interval.as_secs().max(1)));
            options.push(format!("ServerAliveCountMax={count}"));
        }
        for option in &options {
            command.args(["-o", option.as_str()]);
        }
        command.arg(&self.host);
        Ok(command)
    }

    /// Run one remote command, capturing stdout/stderr/exit status. Carries
    /// the one-shot flaky-connection retry; only call this for idempotent,
    /// read-only remote commands.
    pub async fn run(&self, remote_command: &str) -> Result<Output, DagError> {
        self.execute(
            &[remote_command],
            StdinSource::Null,
            self.command_timeout,
            true,
            None,
        )
        .await
    }

    /// Run `bash -s` with `script` piped to the remote shell's stdin.
    /// Read-only scripts only — see [`Self::run_script_once`].
    pub async fn run_script(&self, script: &str) -> Result<Output, DagError> {
        self.execute(
            &["bash", "-s"],
            StdinSource::Bytes(script.as_bytes()),
            self.command_timeout,
            true,
            None,
        )
        .await
    }

    /// Run one remote command under an explicit deadline.
    ///
    /// The default command timeout bounds short probes. A transport that
    /// blocks on a remote task needs the task's own horizon instead, so the
    /// caller states it rather than inheriting a fixed default.
    pub async fn run_with_timeout(
        &self,
        remote_command: &str,
        timeout: Duration,
    ) -> Result<Output, DagError> {
        self.execute(&[remote_command], StdinSource::Null, timeout, true, None)
            .await
    }

    /// Run `bash -s` under an explicit deadline. Read-only scripts only.
    pub async fn run_script_with_timeout(
        &self,
        script: &str,
        timeout: Duration,
    ) -> Result<Output, DagError> {
        self.execute(
            &["bash", "-s"],
            StdinSource::Bytes(script.as_bytes()),
            timeout,
            true,
            None,
        )
        .await
    }

    /// Run one remote command with a local file piped to its stdin —
    /// `cat > <remote path>` uploads. The file is streamed, never buffered
    /// in memory.
    pub async fn run_with_file_stdin(
        &self,
        remote_command: &str,
        local_path: &Path,
    ) -> Result<Output, DagError> {
        let file = admit_upload_source(local_path)?;
        self.run_with_admitted_file_stdin(remote_command, file)
            .await
    }

    // Preserve the same admitted inode across optional remote path resolution
    // and mkdir operations performed by SshArtifactStore::upload.
    async fn run_with_admitted_file_stdin(
        &self,
        remote_command: &str,
        file: std::fs::File,
    ) -> Result<Output, DagError> {
        self.execute(
            &[remote_command],
            StdinSource::File(file),
            self.transfer_timeout,
            false,
            None,
        )
        .await
    }

    async fn run_with_admitted_file_stdin_observed(
        &self,
        remote_command: &str,
        file: std::fs::File,
        reporter: &crate::dag::node_event::NodeReporter,
        phase: &str,
    ) -> Result<Output, DagError> {
        self.execute(
            &[remote_command],
            StdinSource::File(file),
            self.transfer_timeout,
            false,
            Some((reporter, phase)),
        )
        .await
    }

    /// Run `bash -s` with `script` piped to stdin, WITHOUT the
    /// flaky-connection retry. The submit path must use this: sbatch is the
    /// only non-idempotent command in the channel, and a blind retry can
    /// submit a second job when the first attempt committed but its stdout
    /// was lost. Any caller with side effects must use a no-retry path too.
    pub async fn run_script_once(&self, script: &str) -> Result<Output, DagError> {
        self.execute(
            &["bash", "-s"],
            StdinSource::Bytes(script.as_bytes()),
            self.command_timeout,
            false,
            None,
        )
        .await
    }

    /// Authenticate the actual submission connection before recording an intent.
    /// Only a harmless printf reaches the remote shell until `before_send`
    /// succeeds. The bootstrap then uses this same stream, without reconnecting
    /// or retrying. A missing handshake therefore cannot have submitted a job.
    pub async fn run_submission_once<F>(
        &self,
        script: &str,
        before_send: F,
    ) -> Result<Output, DagError>
    where
        F: FnOnce() -> Result<(), DagError> + Send,
    {
        let marker = format!("AUTONOMICS_SUBMIT_READY_{}\n", nonce_hex(16));
        let prelude = format!("printf '%s\\n' {}\n", sh_single_quote(marker.trim_end()));
        let mut command = self
            .command()
            .map_err(|error| DagError::SubmissionNotSent {
                message: error.to_string(),
                local_ssh_stopped: true,
            })?;
        command
            .args(["bash", "-s"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| DagError::SubmissionNotSent {
                message: format!("SSH submission channel unavailable before payload: {error}"),
                local_ssh_stopped: true,
            })?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        // Drain independently, retaining bounded private diagnostics. A banner
        // must not fill stderr and deadlock authentication against stdout.
        let mut stderr_task = tokio::spawn(async move {
            let mut retained = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let n = stderr.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                let keep = n.min(65536usize.saturating_sub(retained.len()));
                retained.extend_from_slice(&buffer[..keep]);
            }
            Ok::<_, std::io::Error>(retained)
        });
        let mut payload_admitted = false;
        let operation = async {
            stdin.write_all(prelude.as_bytes()).await.map_err(|e| {
                DagError::Schedule(format!("SSH authentication handshake write failed: {e}"))
            })?;
            stdin
                .flush()
                .await
                .map_err(|e| DagError::Schedule(e.to_string()))?;
            let mut observed = vec![0u8; marker.len()];
            stdout.read_exact(&mut observed).await.map_err(|e| {
                DagError::Schedule(format!("SSH authentication handshake absent: {e}"))
            })?;
            if observed != marker.as_bytes() {
                return Err(DagError::Schedule(
                    "SSH authentication handshake mismatch".into(),
                ));
            }
            before_send()?;
            // From this point even a partial write is uncertain. The caller's
            // durable intent already exists and must be reconciled, not replayed.
            payload_admitted = true;
            stdin.write_all(script.as_bytes()).await.map_err(|e| {
                DagError::Schedule(format!("cannot pipe SSH submission payload: {e}"))
            })?;
            stdin
                .shutdown()
                .await
                .map_err(|e| DagError::Schedule(e.to_string()))?;
            drop(stdin);
            let mut bytes = Vec::new();
            (&mut stdout)
                .take(65537)
                .read_to_end(&mut bytes)
                .await
                .map_err(|e| DagError::Schedule(e.to_string()))?;
            if bytes.len() > 65536 {
                return Err(DagError::Schedule(
                    "oversized SSH submission response".into(),
                ));
            }
            let status = child
                .wait()
                .await
                .map_err(|e| DagError::Schedule(e.to_string()))?;
            Ok((status, bytes))
        };
        let result = tokio::time::timeout(self.command_timeout, operation).await;
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(DagError::Schedule(
                "SSH submission channel timed out".into(),
            )),
        };
        drop(stdout);
        let mut stop_unknown = false;
        if result.is_err() {
            let _ = child.start_kill();
            stop_unknown = !matches!(
                tokio::time::timeout(Duration::from_secs(5), child.wait()).await,
                Ok(Ok(_))
            );
        }
        let diagnostics = match tokio::time::timeout(Duration::from_secs(5), &mut stderr_task).await
        {
            Ok(Ok(Ok(bytes))) => bytes,
            _ => {
                stderr_task.abort();
                Vec::new()
            }
        };
        match result {
            Ok((status, stdout)) => Ok(Output {
                status,
                stdout,
                stderr: diagnostics,
            }),
            Err(error) => {
                let message = format!("{error}; {}", String::from_utf8_lossy(&diagnostics).trim());
                if payload_admitted {
                    Err(DagError::Schedule(format!(
                        "SSH submission outcome UNKNOWN: {message}; retain original intent; do not replay{}",
                        if stop_unknown {
                            "; local SSH child stop UNKNOWN"
                        } else {
                            ""
                        }
                    )))
                } else {
                    Err(DagError::SubmissionNotSent {
                        message,
                        local_ssh_stopped: !stop_unknown,
                    })
                }
            }
        }
    }

    /// Stream one remote command's stdout into `staging` under a size
    /// declaration protocol: the remote side prints ONE header line with the
    /// decimal byte count, then raw content. The declared count catches both
    /// a truncated transfer and stdout pollution — anything other than
    /// exactly `declared` content bytes fails. The payload is never buffered
    /// whole; only a connection failure with zero content bytes is retried.
    pub async fn stream_remote_to_file(
        &self,
        remote_command: &str,
        staging: &Path,
    ) -> Result<RemoteStream, DagError> {
        let mut attempt = 0usize;
        loop {
            match self.stream_remote_once(remote_command, staging).await {
                Ok(stream) => return Ok(stream),
                Err(failure) => {
                    let _ = std::fs::remove_file(staging);
                    if attempt == 0 && failure.ssh_exit_255 && failure.received_bytes == 0 {
                        attempt += 1;
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                    return Err(failure.error);
                }
            }
        }
    }

    async fn stream_remote_once(
        &self,
        remote_command: &str,
        staging: &Path,
    ) -> Result<RemoteStream, StreamFailure> {
        let mut command = self.command().map_err(|error| StreamFailure {
            error,
            ssh_exit_255: false,
            received_bytes: 0,
        })?;
        command
            .arg(remote_command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| StreamFailure {
            error: DagError::Schedule(format!(
                "cannot start ssh program `{}`: {error}",
                self.program
            )),
            ssh_exit_255: false,
            received_bytes: 0,
        })?;
        let mut stdout = child.stdout.take().expect("ssh stdout is piped");
        // Keep draining stderr on its own task: a chatty remote side must
        // not deadlock a full pipe against our read loop.
        let stderr_handle = child.stderr.take().expect("ssh stderr is piped");
        let mut stderr_task = Some(tokio::spawn(async move {
            let mut text = String::new();
            let mut handle = stderr_handle;
            // Read errors here surface through the exit status instead.
            let _ = handle.read_to_string(&mut text).await;
            text
        }));

        let failure = |message: String, ssh_exit_255: bool, received_bytes: u64| StreamFailure {
            error: DagError::Schedule(message),
            ssh_exit_255,
            received_bytes,
        };
        // Keep the child owned across the deadline so handled failures do
        // not return before the local SSH process is actually reaped.
        let result = tokio::time::timeout(self.transfer_timeout, async {
        let mut file = tokio::fs::File::create(staging).await.map_err(|error| {
            failure(
                format!(
                    "cannot create staging file `{}`: {error}",
                    staging.display()
                ),
                false,
                0,
            )
        })?;
        let mut hasher = Sha256::new();
        let mut declared: Option<u64> = None;
        let mut header = Vec::new();
        let mut content_bytes = 0u64;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = stdout.read(&mut buffer).await.map_err(|error| {
                failure(
                    format!("ssh download `{remote_command}` stream error: {error}"),
                    false,
                    content_bytes,
                )
            })?;
            if read == 0 {
                break;
            }
            let mut chunk = &buffer[..read];
            if declared.is_none() {
                match chunk.iter().position(|&byte| byte == b'\n') {
                    Some(newline) => {
                        header.extend_from_slice(&chunk[..newline]);
                        let text = String::from_utf8_lossy(&header);
                        let value = text.trim().parse::<u64>().map_err(|_| {
                            failure(
                                format!(
                                    "ssh download `{remote_command}` sent unusable size header {text:?}"
                                ),
                                false,
                                0,
                            )
                        })?;
                        declared = Some(value);
                        chunk = &chunk[newline + 1..];
                    }
                    None => {
                        if header.len() + chunk.len() > 128 {
                            return Err(failure(
                                format!(
                                    "ssh download `{remote_command}` sent no size header within the first 128 bytes"
                                ),
                                false,
                                0,
                            ));
                        }
                        header.extend_from_slice(chunk);
                        continue;
                    }
                }
            }
            if !chunk.is_empty() {
                hasher.update(chunk);
                file.write_all(chunk).await.map_err(|error| {
                    failure(
                        format!("cannot write staging file `{}`: {error}", staging.display()),
                        false,
                        content_bytes,
                    )
                })?;
                content_bytes += chunk.len() as u64;
            }
        }
        file.flush().await.map_err(|error| {
            failure(
                format!("cannot flush staging file `{}`: {error}", staging.display()),
                false,
                content_bytes,
            )
        })?;
        file.sync_all().await.map_err(|error| {
            failure(
                format!("cannot sync staging file `{}`: {error}", staging.display()),
                false,
                content_bytes,
            )
        })?;
        let status = child.wait().await.map_err(|error| {
            failure(
                format!("ssh download `{remote_command}`: {error}"),
                false,
                content_bytes,
            )
        })?;
        let stderr_text = stderr_task.take().expect("stderr task").await.unwrap_or_default();
        if !status.success() {
            return Err(failure(
                format!(
                    "ssh download `{remote_command}` failed with exit status {:?}: {}",
                    status.code(),
                    stderr_text.trim()
                ),
                status.code() == Some(255),
                content_bytes,
            ));
        }
        let Some(declared) = declared else {
            return Err(failure(
                format!("ssh download `{remote_command}` sent no size header"),
                false,
                0,
            ));
        };
        if declared != content_bytes {
            return Err(failure(
                format!(
                    "ssh download `{remote_command}` declared {declared} bytes but {content_bytes} arrived — refusing the payload"
                ),
                false,
                content_bytes,
            ));
        }
        Ok(RemoteStream {
            declared_size: declared,
            content_bytes,
            content_sha256: hex_encode(&hasher.finalize()),
        })
        }).await.unwrap_or_else(|_| Err(failure(format!("ssh transfer `{remote_command}` timed out"), false, 0)));
        if let Err(mut error) = result {
            drop(stdout);
            let _ = child.start_kill();
            let stopped = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            if let Some(task) = stderr_task.take() {
                task.abort();
                let _ = task.await;
            }
            if !matches!(stopped, Ok(Ok(_))) {
                error.error =
                    DagError::Schedule(format!("{}; local SSH child stop UNKNOWN", error.error));
            }
            return Err(error);
        }
        result
    }

    async fn execute(
        &self,
        args: &[&str],
        stdin: StdinSource<'_>,
        timeout: Duration,
        allow_connection_retry: bool,
        progress: Option<(&crate::dag::node_event::NodeReporter, &str)>,
    ) -> Result<Output, DagError> {
        let mut attempt = 0;
        loop {
            let mut command = self.command()?;
            command
                .args(args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            match &stdin {
                StdinSource::File(file) => {
                    let file = file.try_clone().map_err(|error| {
                        DagError::Schedule(format!("cannot duplicate admitted ssh stdin: {error}"))
                    })?;
                    command.stdin(Stdio::from(file));
                }
                StdinSource::Bytes(_) => {
                    command.stdin(Stdio::piped());
                }
                StdinSource::Null => {
                    command.stdin(Stdio::null());
                }
            }
            let mut child = command.spawn().map_err(|error| {
                DagError::Schedule(format!(
                    "cannot start ssh program `{}`: {error}",
                    self.program
                ))
            })?;
            // The same deadline covers stdin and output. Only explicitly
            // idempotent callers may retry one empty connection failure.
            let operation = async {
                if let StdinSource::Bytes(bytes) = &stdin {
                    if let Some(mut handle) = child.stdin.take() {
                        handle.write_all(bytes).await.map_err(|error| {
                            DagError::Schedule(format!("cannot pipe ssh stdin: {error}"))
                        })?;
                        handle.shutdown().await.map_err(|error| {
                            DagError::Schedule(format!("cannot close ssh stdin: {error}"))
                        })?;
                    }
                }
                let mut observed_file = match &stdin {
                    StdinSource::File(file) => file.try_clone().ok(),
                    _ => None,
                };
                let total = observed_file
                    .as_ref()
                    .and_then(|file| file.metadata().ok())
                    .map(|meta| meta.len());
                let output = child.wait_with_output();
                tokio::pin!(output);
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                let result = loop {
                    tokio::select! {
                        result = &mut output => break result,
                        _ = tick.tick(), if progress.is_some() => {
                            if let (Some((reporter, phase)), Some(file)) = (progress, observed_file.as_mut()) {
                                if let Ok(current) = std::io::Seek::stream_position(file) {
                                    // Bytes consumed by SSH, not proof of remote publication.
                                    reporter.phase_progress(phase, current, total, "bytes");
                                }
                            }
                        }
                    }
                };
                result.map_err(|error| {
                    DagError::Schedule(format!("ssh command `{}` failed: {error}", args.join(" ")))
                })
            };
            let output = tokio::time::timeout(timeout, operation)
                .await
                .map_err(|_| {
                    DagError::Schedule(format!(
                        "ssh command `{}` timed out after {timeout:?}; remote outcome UNKNOWN",
                        args.join(" ")
                    ))
                })??;
            if attempt == 0
                && allow_connection_retry
                && !matches!(&stdin, StdinSource::File(_))
                && output.status.code() == Some(255)
                && output.stdout.is_empty()
            {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            return Ok(output);
        }
    }
}

fn admit_upload_source(path: &Path) -> Result<std::fs::File, DagError> {
    open_upload_source(path).map_err(|error| {
        DagError::Schedule(format!(
            "cannot admit regular ssh stdin `{}`: {error}",
            path.display()
        ))
    })
}

/// Admit an upload before SSH starts. Never follow a final symlink or block
/// waiting for a FIFO writer. Stdio consumes this exact held descriptor; the
/// source pathname is never reopened after admission.
#[cfg(target_os = "linux")]
fn open_upload_source(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    // Linux O_NOFOLLOW | O_NONBLOCK. Do not replace this with File::open or
    // retry without flags: that would reintroduce pre-deadline FIFO blocking.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(0x20000 | 0x800)
        .open(path)?;
    let held = file.metadata()?;
    if !held.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "upload source must be a regular file",
        ));
    }
    let named = std::fs::symlink_metadata(path)?;
    if !named.is_file() || held.dev() != named.dev() || held.ino() != named.ino() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "upload source changed during descriptor admission",
        ));
    }
    Ok(file)
}

#[cfg(not(target_os = "linux"))]
fn open_upload_source(_path: &Path) -> std::io::Result<std::fs::File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "nonblocking no-follow Slurm upload admission requires Linux",
    ))
}

enum StdinSource<'a> {
    Null,
    Bytes(&'a [u8]),
    File(std::fs::File),
}

/// What [`SshTarget::stream_remote_to_file`] received from the remote side.
pub struct RemoteStream {
    /// Byte count the remote side declared in its header line.
    pub declared_size: u64,
    /// Content bytes actually streamed into the staging file.
    pub content_bytes: u64,
    /// Lowercase SHA-256 of the streamed content.
    pub content_sha256: String,
}

/// A failed stream attempt, carrying what the retry policy needs: only a
/// connection-level failure with zero content bytes is safe to retry.
struct StreamFailure {
    error: DagError,
    ssh_exit_255: bool,
    received_bytes: u64,
}

/// Fail loud on a nonzero ssh exit; otherwise hand back stdout as text.
pub(super) fn ensure_ssh_success(output: Output, context: &str) -> Result<Output, DagError> {
    if output.status.success() {
        return Ok(output);
    }
    Err(DagError::Schedule(format!(
        "{context} failed with exit status {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// Whether one exact-job `squeue` response proves queue absence. This says
/// nothing about completion: exact terminal accounting is still mandatory.
/// Unknown errors and mixed output never count as absence.
pub fn slurm_queue_absent(output: &Output) -> bool {
    output.stdout.iter().all(u8::is_ascii_whitespace)
        && (output.status.success()
            || (output.status.code() == Some(1)
                && output.stderr.as_slice().trim_ascii()
                    == b"slurm_load_jobs error: Invalid job id specified"))
}

fn queue_stdout_text(output: Output, context: &str) -> Result<String, DagError> {
    if slurm_queue_absent(&output) {
        Ok(String::new())
    } else {
        ssh_stdout_text(output, context)
    }
}

pub(super) fn ssh_stdout_text(output: Output, context: &str) -> Result<String, DagError> {
    ensure_ssh_success(output, context)
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Quote an arbitrary string as one shell word. Used for every env value
/// and remote path embedded in generated scripts, so neither the
/// coordinator nor a task spec can inject shell syntax.
pub(super) fn sh_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Lowercase hex without pulling in a hex crate (house pattern).
pub(super) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A per-attempt unique tag (`bytes` bytes = twice as many lowercase hex
/// chars) hashed from clock/pid/counter — `rand` is not a dependency and
/// this only needs collision-unlikelihood within one daemon's lifetime.
fn nonce_hex(bytes: usize) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(counter.to_le_bytes());
    let digest = hasher.finalize();
    hex_encode(&digest.as_slice()[..bytes])
}

/// `auto-<sanitized task id>-<nonce>`, clamped to the 63-character
/// job-name ceiling every Slurm version tolerates (the id loses its tail
/// first; the nonce stays intact so adoption can never confuse attempts).
fn slurm_job_name(task_id: &str, nonce: &str) -> String {
    let mut name = format!("auto-{}", sanitize_path_component(task_id));
    let budget = 63usize.saturating_sub(nonce.len() + 1);
    if name.len() > budget {
        let mut boundary = budget;
        while boundary > 0 && !name.is_char_boundary(boundary) {
            boundary -= 1;
        }
        name.truncate(boundary);
    }
    format!("{name}-{nonce}")
}

// ── settings ─────────────────────────────────────────────────────────────

/// Operator-owned settings for the Slurm backend. Built from
/// `RuntimeConfig::slurm` at daemon start; never agent-settable.
#[derive(Debug, Clone)]
pub struct SlurmSettings {
    /// Operator-owned digest-to-SIF map; absent means container tasks fail closed.
    pub apptainer: Option<super::apptainer::ApptainerSettings>,
    /// ssh destination for the cluster login node.
    pub ssh: SshTarget,
    /// Remote scratch root for task workspaces; a leading `~/` expands to
    /// the remote home directory. Must not contain whitespace once expanded
    /// (sbatch directive values cannot be quoted).
    pub scratch_dir: String,
    /// Local directory downloaded artifacts land in.
    pub local_artifact_root: PathBuf,
    /// Default sbatch partition (e.g. `64c512g`).
    pub partition: String,
    /// Explicit allocation account; no implicit scheduler default when configured.
    pub account: Option<String>,
    /// Scheduler-owned GPU allocation. None preserves CPU-only submission.
    pub gpus: Option<u32>,
    /// Optional operator QOS (e.g. debug for a debug GPU partition).
    pub qos: Option<String>,
    /// Memory per allocated CPU when a task requests no explicit total.
    pub mem_per_cpu: String,
    /// Wall-clock limit when a task requests no explicit duration.
    pub default_time: String,
    /// Terminal-state polling cadence.
    pub poll_interval: Duration,
}

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
    settings: SlurmSettings,
    pending: Mutex<HashMap<String, PendingSlurmJob>>,
    cancelled: Mutex<HashSet<String>>,
    scratch_home: Mutex<Option<String>>,
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
            sh_single_quote(&format!("{}/{}", job.task_dir, super::heartbeat::FILE_NAME)),
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
/// the same contract [`super::remote::RemoteTaskExecutor`] uses when it
/// uploads (`input-{port}-{index}` names, one artifact per staged path).
pub(super) fn match_uploaded_inputs(
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
pub(super) fn attempt_key(dispatch: &TaskDispatch) -> Result<String, DagError> {
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

pub(super) fn remote_task_dir(scratch_dir: &str, task_id: &str) -> String {
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
pub(super) async fn resolve_remote_prefix(
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
fn render_sbatch_script(
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
        script.push_str(&super::apptainer::render(
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
    script.push_str(&super::process::render_process_payload(
        process,
        environment,
        inputs_by_name,
    ));
    Ok(script)
}

pub(super) fn heredoc_delimiter(base: &str, contents: &str) -> String {
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
fn render_bootstrap_script(
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
pub(super) fn render_collect_script(process: &ProcessTaskSpec, task_dir: &str) -> String {
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

// ── artifact store ───────────────────────────────────────────────────────

/// A retained cluster filesystem exposed through a read-only VFS mount.
/// Aliases are explicit operator assertions that data/login endpoints share
/// this filesystem; the login endpoint reopens and measures every reference.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshDataHome {
    pub ssh_aliases: Vec<String>,
    pub remote_root: String,
    pub vfs_prefix: String,
    /// Catalog identity of this domain, flowing into every placement the
    /// store publishes so downstream consumers can compare domains by id.
    /// Legacy profiles (frozen before domains existed) carry none: their
    /// placements stay unmarked (`local`), preserving old behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_id: Option<String>,
    /// Optional dedicated output namespace. Both fields must be supplied;
    /// inputs remain confined to `remote_root` and its admitted SSH aliases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_vfs_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_remote_root: Option<String>,
}

fn clean_absolute(path: &str) -> bool {
    path.starts_with('/')
        && path != "/"
        && !path.ends_with('/')
        && !path
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        && !path.chars().any(char::is_control)
}

impl SshDataHome {
    fn retained_mapping(&self) -> (&str, &str) {
        match (&self.retained_vfs_prefix, &self.retained_remote_root) {
            (Some(prefix), Some(root)) => (prefix, root),
            _ => (&self.vfs_prefix, &self.remote_root),
        }
    }

    fn validate(&self) -> Result<(), DagError> {
        if !clean_absolute(&self.remote_root)
            || !clean_absolute(&self.vfs_prefix)
            || self.ssh_aliases.is_empty()
            || self.ssh_aliases.iter().any(|alias| {
                alias.is_empty()
                    || alias.starts_with('-')
                    || !alias
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_.@:-".contains(&c))
            })
        {
            return Err(DagError::Schedule("invalid retained SSH data home".into()));
        }
        match (&self.retained_vfs_prefix, &self.retained_remote_root) {
            (None, None) => {}
            (Some(prefix), Some(root)) if clean_absolute(prefix)
                && clean_absolute(root)
                && prefix != &self.vfs_prefix
                && root != &self.remote_root
                && Path::new(root).starts_with(&self.remote_root) => {}
            _ => return Err(DagError::Schedule(
                "retained output prefix/root must be a paired, distinct namespace below the data home".into()
            )),
        }
        Ok(())
    }

    /// Bind the operator's retained-output namespace to the actual VFS.
    /// Check nested overrides too: a deeper local or different SSH mount must
    /// never silently reinterpret a generated retained artifact reference.
    pub fn validate_vfs(&self, storage: &vfs::OpendalFileStorage) -> Result<(), DagError> {
        self.validate()?;
        self.validate_vfs_mapping(storage, &self.vfs_prefix, &self.remote_root)?;
        if let (Some(prefix), Some(root)) = (&self.retained_vfs_prefix, &self.retained_remote_root)
        {
            self.validate_vfs_mapping(storage, prefix, root)?;
        }
        Ok(())
    }

    fn validate_vfs_mapping(
        &self,
        storage: &vfs::OpendalFileStorage,
        vfs_prefix: &str,
        remote_root: &str,
    ) -> Result<(), DagError> {
        let mut prefixes = vec![vfs_prefix.to_string()];
        if let Some(mounts) = &storage.mounts {
            prefixes.extend(
                mounts
                    .mount_paths()
                    .into_iter()
                    .filter(|path| path.starts_with(&format!("{vfs_prefix}/"))),
            );
        }
        for prefix in prefixes {
            let virtual_path = format!("{prefix}/__autonomics_retained_mapping_probe");
            let expected = format!(
                "{}{}{}",
                remote_root,
                prefix.strip_prefix(vfs_prefix).unwrap_or_default(),
                "/__autonomics_retained_mapping_probe"
            );
            let location = storage
                .ssh_location(&virtual_path)
                .map_err(|_| {
                    DagError::Schedule(
                        "retained VFS mapping is not readable by the operator".into(),
                    )
                })?
                .ok_or_else(|| {
                    DagError::Schedule("retained VFS namespace must resolve to SSH storage".into())
                })?;
            if !self.ssh_aliases.contains(&location.alias) || location.path != expected {
                return Err(DagError::Schedule(
                    "retained VFS namespace does not match its SSH data home".into(),
                ));
            }
        }
        Ok(())
    }
}

// Data bytes stay on the shared filesystem. Measure through a held no-follow
// descriptor and refuse changed files; only the bounded identity JSON returns.
const MEASURE_DATA_HOME_FILE: &str = r#"
import os,sys,json,stat,hashlib
r=json.loads(sys.argv[1])
root=r['root']; path=r['path']
if not path.startswith(root+'/'): raise ValueError('outside data root')
parts=path.split('/')[1:]
if any(p in ('','.','..') for p in parts): raise ValueError('invalid path')
fd=os.open('/',os.O_PATH|os.O_DIRECTORY|os.O_NOFOLLOW)
try:
 for p in parts[:-1]:
  nxt=os.open(p,os.O_PATH|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=fd)
  os.close(fd); fd=nxt
 data=os.open(parts[-1],os.O_RDONLY|os.O_NONBLOCK|os.O_NOFOLLOW,dir_fd=fd)
 try:
  before=os.fstat(data)
  if not stat.S_ISREG(before.st_mode): raise ValueError('regular file required')
  h=hashlib.sha256()
  while True:
   chunk=os.read(data,1024*1024)
   if not chunk: break
   h.update(chunk)
  key=lambda s:(s.st_dev,s.st_ino,s.st_mode,s.st_size,s.st_mtime_ns,s.st_ctime_ns)
  if key(before)!=key(os.fstat(data)) or key(before)!=key(os.stat(parts[-1],dir_fd=fd,follow_symlinks=False)): raise ValueError('changed file')
  print(json.dumps({'size':before.st_size,'sha256':h.hexdigest()}))
 finally: os.close(data)
finally: os.close(fd)
"#;

/// A [`TaskArtifactStore`] that moves single files over the same ssh
/// channel as the transport (`cat > remote` to upload, `cat remote` to
/// download), keeping the ssh surface at exactly one program.
#[derive(Debug)]
pub struct SshArtifactStore {
    ssh: SshTarget,
    scratch_dir: String,
    local_root: PathBuf,
    scratch_home: Mutex<Option<String>>,
    data_home: Option<SshDataHome>,
}

impl SshArtifactStore {
    /// The catalog identity of the domain this store's in-place references
    /// and retained publications belong to. Legacy profiles without a
    /// declared domain keep placements unmarked (`local`).
    fn placement_domain(&self) -> DomainId {
        self.data_home
            .as_ref()
            .and_then(|home| home.domain_id.as_deref())
            .map(DomainId::new)
            .unwrap_or_default()
    }

    pub fn new(
        ssh: SshTarget,
        scratch_dir: impl Into<String>,
        local_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            ssh,
            scratch_dir: scratch_dir.into(),
            local_root: local_root.into(),
            scratch_home: Mutex::new(None),
            data_home: None,
        }
    }

    pub fn with_data_home(mut self, data_home: SshDataHome) -> Result<Self, DagError> {
        data_home.validate()?;
        if !clean_absolute(&self.scratch_dir)
            || !Path::new(&self.scratch_dir).starts_with(&data_home.remote_root)
            || self.scratch_dir == data_home.remote_root
        {
            return Err(DagError::Schedule(
                "remote task work must be below its retained data home".into(),
            ));
        }
        let (_, retained_root) = data_home.retained_mapping();
        if !Path::new(&self.scratch_dir).starts_with(retained_root) {
            return Err(DagError::Schedule(
                "task work must be inside the retained output root".into(),
            ));
        }
        self.data_home = Some(data_home);
        Ok(self)
    }

    async fn measure_data_file(&self, path: &str, original: &FileRef) -> Result<FileRef, DagError> {
        let home = self
            .data_home
            .as_ref()
            .ok_or_else(|| DagError::Schedule("SSH data home is disabled".into()))?;
        if !clean_absolute(path)
            || !Path::new(path).starts_with(&home.remote_root)
            || path == home.remote_root
        {
            return Err(DagError::Schedule(
                "remote file is outside its retained data home".into(),
            ));
        }
        let request = serde_json::json!({"root":home.remote_root,"path":path}).to_string();
        let command = format!(
            "python3 -c {} {}",
            sh_single_quote(MEASURE_DATA_HOME_FILE),
            sh_single_quote(&request)
        );
        let raw = ssh_stdout_text(
            self.ssh.run_script_once(&command).await?,
            "remote file identity",
        )?;
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Identity {
            size: u64,
            sha256: String,
        }
        let identity: Identity = serde_json::from_str(&raw)
            .map_err(|_| DagError::Schedule("invalid remote file identity".into()))?;
        if identity.sha256.len() != 64 || !identity.sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(DagError::Schedule("invalid remote file SHA256".into()));
        }
        if let Some(expected) = &original.fingerprint {
            if expected.size != identity.size
                || expected.content_hash.as_ref().is_some_and(|hash| {
                    hash.strip_prefix("sha256:").unwrap_or(hash) != identity.sha256
                })
            {
                return Err(DagError::Schedule(
                    "retained remote file fingerprint changed".into(),
                ));
            }
        }
        let mut result = FileRef::remote(
            path,
            original.format.clone(),
            identity.size,
            Some(identity.sha256),
        )
        .in_domain(self.placement_domain());
        // A POSIX file is not an immutable object store, even after hashing.
        if let Some(fingerprint) = &mut result.fingerprint {
            fingerprint.immutable_remote = false;
        }
        Ok(result)
    }

    /// Absolute remote path for one artifact inside the task's scratch dir.
    async fn remote_path(&self, task_id: &str, name: &str) -> Result<String, DagError> {
        let scratch =
            resolve_remote_prefix(&self.ssh, &self.scratch_dir, &self.scratch_home).await?;
        Ok(format!(
            "{}/{}/{}",
            scratch.trim_end_matches('/'),
            sanitize_path_component(task_id),
            name
        ))
    }
}

#[async_trait::async_trait]
impl TaskArtifactStore for SshArtifactStore {
    async fn remote_input(
        &self,
        artifact: FileRef,
        storage: Option<&vfs::OpendalFileStorage>,
    ) -> Result<Option<FileRef>, DagError> {
        let Some(home) = &self.data_home else {
            return Ok(None);
        };
        let Some(storage) = storage else {
            return Ok(None);
        };
        let Some(location) = storage
            .ssh_location(&artifact.path)
            .map_err(|error| DagError::Schedule(format!("remote input authorization: {error}")))?
        else {
            return Ok(None);
        };
        let expected_endpoint = self
            .ssh
            .shared_endpoints
            .get(&location.alias)
            .cloned()
            .or_else(|| {
                (location.alias == self.ssh.host)
                    .then(|| self.ssh.endpoint.clone())
                    .flatten()
            });
        let actual_endpoint = if location.endpoint.is_none() && expected_endpoint.is_some() {
            // An existing vfs.toml mount can still use an enrolled shared data
            // endpoint. Resolve its current local SSH configuration, never
            // assume that an unpinned alias still names the original server.
            vfs::SshEndpoint::capture("ssh", &location.alias).ok()
        } else {
            location.endpoint.clone()
        };
        if !home.ssh_aliases.contains(&location.alias)
            || actual_endpoint != expected_endpoint
            || !Path::new(&location.path).starts_with(&home.remote_root)
        {
            // Another authorized data source needs ordinary VFS staging. Its
            // physical path is never interpreted on this compute target.
            return Ok(None);
        }
        self.measure_data_file(&location.path, &artifact)
            .await
            .map(Some)
    }

    async fn retained_reference(&self, artifact: FileRef) -> Result<Option<FileRef>, DagError> {
        let Some(home) = &self.data_home else {
            return Ok(None);
        };
        let (prefix, root) = home.retained_mapping();
        let relative = Path::new(&artifact.path)
            .strip_prefix(root)
            .map_err(|_| DagError::Schedule("remote output is outside its retained root".into()))?;
        if relative.as_os_str().is_empty() {
            return Err(DagError::Schedule(
                "retained output must be a file below its root".into(),
            ));
        }
        let mut reference = self.measure_data_file(&artifact.path, &artifact).await?;
        reference.path = vfs::uri::uri(format!("{prefix}/{}", relative.to_string_lossy()));
        // measure_data_file already stamps the domain; assert it stays put
        // when the path is rewritten into the VFS projection.
        debug_assert_eq!(reference.domain, self.placement_domain());
        Ok(Some(reference))
    }

    async fn upload(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
    ) -> Result<FileRef, DagError> {
        self.upload_observed(
            artifact,
            task_id,
            name,
            &crate::dag::node_event::NodeReporter::noop(),
        )
        .await
    }

    async fn upload_observed(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<FileRef, DagError> {
        let phase = format!("hashing_upload:{name}");
        reporter.phase(&phase, "verifying upload source");
        // Reject FIFO/socket/directory/link/missing inputs before even the
        // first SSH home lookup or mkdir, then keep this exact file descriptor.
        let mut source = admit_upload_source(Path::new(&artifact.path))?;
        let before = source
            .metadata()
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut size = 0u64;
        let mut last = std::time::Instant::now();
        reporter.phase_progress(&phase, 0, Some(before.len()), "bytes");
        loop {
            let count = std::io::Read::read(&mut source, &mut buffer)
                .map_err(|error| DagError::Schedule(error.to_string()))?;
            if count == 0 {
                break;
            }
            size += count as u64;
            hasher.update(&buffer[..count]);
            if last.elapsed() >= Duration::from_secs(1) {
                reporter.phase_progress(&phase, size, Some(before.len()), "bytes");
                last = std::time::Instant::now();
                tokio::task::yield_now().await;
            }
        }
        reporter.phase_progress(&phase, size, Some(before.len()), "bytes");
        let after = source
            .metadata()
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        if before.len() != size
            || before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
        {
            return Err(DagError::Schedule(
                "upload input changed during hashing".into(),
            ));
        }
        let digest = hex_encode(&hasher.finalize());
        if let Some(expected) = &artifact.fingerprint {
            if expected.size != size
                || expected
                    .content_hash
                    .as_ref()
                    .is_some_and(|hash| hash.strip_prefix("sha256:").unwrap_or(hash) != digest)
            {
                return Err(DagError::Schedule(
                    "upload input fingerprint changed".into(),
                ));
            }
        }
        std::io::Seek::seek(&mut source, std::io::SeekFrom::Start(0))
            .map_err(|error| DagError::Schedule(error.to_string()))?;
        let remote = self.remote_path(task_id, name).await?;
        let parent = match remote.rfind('/') {
            Some(index) => remote[..index].to_string(),
            None => ".".into(),
        };
        ssh_stdout_text(
            self.ssh
                .run(&format!("umask 077; mkdir -p {}", sh_single_quote(&parent)))
                .await?,
            "slurm artifact staging",
        )?;
        for attempt in 0..2 {
            std::io::Seek::seek(&mut source, std::io::SeekFrom::Start(0))
                .map_err(|error| DagError::Schedule(error.to_string()))?;
            let output = self
                .ssh
                .run_with_admitted_file_stdin_observed(
                    &format!("umask 077; set -C; cat > {}", sh_single_quote(&remote)),
                    source
                        .try_clone()
                        .map_err(|error| DagError::Schedule(error.to_string()))?,
                    reporter,
                    &format!("uploading:{name}"),
                )
                .await?;
            if output.status.success() {
                reporter.phase_progress(&format!("uploading:{name}"), size, Some(size), "bytes");
                break;
            }
            if output.status.code() != Some(255) || attempt != 0 {
                ensure_ssh_success(output, "slurm artifact upload")?;
                unreachable!();
            }
            // A lost SSH reply does not tell us whether cat ran. Inspect the
            // exact target before deciding: complete bytes are usable input,
            // absence permits one retry, and partial/foreign bytes stay held.
            // This never retries sbatch or invents a remote command exit.
            let target = sh_single_quote(&remote);
            let observed = ssh_stdout_text(self.ssh.run(&format!(
                "set -e; if [ -L {target} ] || {{ [ -e {target} ] && [ ! -f {target} ]; }}; then printf 'FOREIGN\\n'; elif [ -f {target} ]; then printf 'FILE\\n'; stat -c %s -- {target}; sha256sum -- {target}; else printf 'ABSENT\\n'; fi"
            )).await?, "slurm interrupted upload readback")?;
            if observed == "ABSENT\n" {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            let lines = observed.lines().collect::<Vec<_>>();
            if lines.len() == 3
                && lines[0] == "FILE"
                && lines[1].parse::<u64>().ok() == Some(size)
                && lines[2].split_whitespace().next() == Some(digest.as_str())
            {
                break;
            }
            return Err(DagError::Schedule(
                "interrupted upload left partial or foreign bytes; target retained without retry or overwrite".into(),
            ));
        }
        Ok(FileRef {
            path: remote,
            format: artifact.format.clone(),
            fingerprint: Some(FileFingerprint {
                size,
                mtime_ns: 0,
                content_hash: Some(digest),
                immutable_remote: false,
            }),
            // The uploaded copy physically lives in the target's scratch
            // domain; the coordinator-side original keeps its own domain.
            domain: self.placement_domain(),
        })
    }

    async fn download(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
    ) -> Result<FileRef, DagError> {
        self.download_observed(
            artifact,
            task_id,
            name,
            &crate::dag::node_event::NodeReporter::noop(),
        )
        .await
    }

    async fn download_observed(
        &self,
        artifact: FileRef,
        task_id: &str,
        name: &str,
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<FileRef, DagError> {
        reporter.phase(
            &format!("downloading:{name}"),
            "verifying remote artifact identity",
        );

        let destination_dir = self.local_root.join(sanitize_path_component(task_id));
        std::fs::create_dir_all(&destination_dir).map_err(|error| {
            DagError::Schedule(format!(
                "cannot create slurm artifact directory `{}`: {error}",
                destination_dir.display()
            ))
        })?;
        if Path::new(name).components().count() != 1 || name == "." || name == ".." {
            return Err(DagError::Schedule(
                "invalid artifact destination name".into(),
            ));
        }
        let destination = destination_dir.join(name);
        // Artifact paths normally come from our own receipts (absolute), but
        // resolve defensively so a tilde never reaches a quoted ssh command.
        let remote_source =
            resolve_remote_prefix(&self.ssh, &artifact.path, &self.scratch_home).await?;
        let metadata = ssh_stdout_text(
            self.ssh
                .run(&format!(
                    "stat -c %s -- {source} && sha256sum -- {source}",
                    source = sh_single_quote(&remote_source)
                ))
                .await?,
            "slurm artifact metadata",
        )?;
        let mut lines = metadata.lines();
        let expected_size = lines
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| DagError::Schedule("invalid artifact size".into()))?;
        let expected_hash = lines
            .next()
            .and_then(|s| s.split_whitespace().next())
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| DagError::Schedule("invalid artifact SHA256".into()))?;
        if lines.next().is_some() {
            return Err(DagError::Schedule("ambiguous artifact metadata".into()));
        }
        let fingerprint = match &artifact.fingerprint {
            Some(expected) => {
                if expected.size != expected_size
                    || expected
                        .content_hash
                        .as_deref()
                        .is_some_and(|hash| !hash.eq_ignore_ascii_case(expected_hash))
                {
                    return Err(DagError::Schedule(
                        "artifact fingerprint mismatch for slurm download".into(),
                    ));
                }
                let mut verified = expected.clone();
                if verified.content_hash.is_none() {
                    verified.content_hash = Some(expected_hash.to_ascii_lowercase());
                }
                Some(verified)
            }
            None => Some(FileFingerprint::remote(
                expected_size,
                Some(expected_hash.to_ascii_lowercase()),
            )),
        };
        stream_download_observed(
            &self.ssh,
            &remote_source,
            &destination,
            expected_size,
            expected_hash,
            reporter,
            &format!("downloading:{name}"),
        )
        .await?;
        Ok(FileRef {
            domain: DomainId::local(),
            path: destination.to_string_lossy().into_owned(),
            format: artifact.format.clone(),
            fingerprint,
        })
    }
}

// Anonymous Linux staging has no pathname to replace or accidentally delete.
// Closing its held FD on failure/cancellation is the complete cleanup operation.
struct StagedDownload(std::fs::File);

impl StagedDownload {
    #[cfg(target_os = "linux")]
    fn create(destination: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let parent = destination.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "missing destination directory",
            )
        })?;
        // Linux O_TMPFILE | O_NOFOLLOW. No named-file fallback: unsupported
        // filesystems must fail without touching an existing destination.
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(0x410000 | 0x20000)
            .open(parent)
            .map(Self)
    }

    #[cfg(not(target_os = "linux"))]
    fn create(_destination: &Path) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "safe anonymous Slurm downloads require Linux O_TMPFILE",
        ))
    }

    #[cfg(target_os = "linux")]
    fn publish(&self, destination: &Path) -> std::io::Result<()> {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        unsafe extern "C" {
            fn linkat(
                oldfd: i32,
                oldpath: *const std::ffi::c_char,
                newfd: i32,
                newpath: *const std::ffi::c_char,
                flags: i32,
            ) -> i32;
        }
        let source = CString::new(format!("/proc/self/fd/{}", self.0.as_raw_fd()))?;
        let destination = CString::new(destination.as_os_str().as_bytes())?;
        // AT_FDCWD / AT_SYMLINK_FOLLOW: follow only our held proc FD, never a
        // mutable staging pathname. linkat is atomic and never replaces EEXIST.
        // SAFETY: both C strings live across the call and the FD remains owned.
        let result = unsafe { linkat(-100, source.as_ptr(), -100, destination.as_ptr(), 0x400) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn publish(&self, _destination: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "safe anonymous Slurm downloads require Linux",
        ))
    }
}

async fn stream_download_observed(
    ssh: &SshTarget,
    source: &str,
    destination: &Path,
    expected_size: u64,
    expected_hash: &str,
    reporter: &crate::dag::node_event::NodeReporter,
    phase: &str,
) -> Result<(), DagError> {
    let started = std::time::Instant::now();
    for attempt in 0..2 {
        let remaining = ssh.transfer_timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(DagError::Schedule(
                "artifact transfer timed out; no retry budget remains".into(),
            ));
        }
        let connection = ssh.clone().with_transfer_timeout(remaining);
        match stream_download_once_observed(
            &connection,
            source,
            destination,
            expected_size,
            expected_hash,
            reporter,
            phase,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(DagError::Schedule(message))
                if attempt == 0 && message == "artifact ssh transfer interrupted (exit 255)" =>
            {
                // Reading an authenticated artifact is idempotent. The failed
                // local child has been reaped and its anonymous staging FD is
                // closed; start a fresh full stream and verify all bytes again.
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

async fn stream_download_once_observed(
    ssh: &SshTarget,
    source: &str,
    destination: &Path,
    expected_size: u64,
    expected_hash: &str,
    reporter: &crate::dag::node_event::NodeReporter,
    phase: &str,
) -> Result<(), DagError> {
    let failure = |error| DagError::Schedule(format!("slurm artifact transfer failed: {error}"));
    let staging = StagedDownload::create(destination).map_err(failure)?;
    let mut file = tokio::fs::File::from_std(staging.0.try_clone().map_err(failure)?);
    let mut child = ssh
        .command()?
        .arg(format!("cat -- {}", sh_single_quote(source)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(failure)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| DagError::Schedule("missing ssh stdout".into()))?;
    let transfer = async {
        let mut hash = Sha256::new();
        let mut size = 0u64;
        let mut last = std::time::Instant::now();
        reporter.phase_progress(phase, 0, Some(expected_size), "bytes");
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = stdout.read(&mut buffer).await.map_err(failure)?;
            if count == 0 {
                break;
            }
            size = size
                .checked_add(count as u64)
                .ok_or_else(|| DagError::Schedule("artifact size overflow".into()))?;
            if size > expected_size {
                return Err(DagError::Schedule("artifact size mismatch".into()));
            }
            hash.update(&buffer[..count]);
            file.write_all(&buffer[..count]).await.map_err(failure)?;
            if last.elapsed() >= Duration::from_secs(1) {
                reporter.phase_progress(phase, size, Some(expected_size), "bytes");
                last = std::time::Instant::now();
            }
        }
        let status = child.wait().await.map_err(failure)?;
        if !status.success() {
            return Err(DagError::Schedule(format!(
                "artifact ssh transfer interrupted (exit {})",
                status
                    .code()
                    .map_or_else(|| "signal".into(), |code| code.to_string())
            )));
        }
        if size != expected_size
            || !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected_hash)
        {
            return Err(DagError::Schedule(
                "artifact size or SHA256 mismatch".into(),
            ));
        }
        file.sync_all().await.map_err(failure)?;
        staging.publish(destination).map_err(failure)?;
        reporter.phase_progress(phase, size, Some(expected_size), "bytes");
        Ok(())
    };
    let result = tokio::time::timeout(ssh.transfer_timeout, transfer)
        .await
        .unwrap_or_else(|_| {
            Err(DagError::Schedule(
                "artifact transfer timed out; destination retained".into(),
            ))
        });
    if let Err(error) = result {
        // An early size/read/write error or deadline can occur before wait().
        // kill_on_drop only requests termination; do not return a handled
        // failure while relying on a runtime which may immediately shut down
        // to reap the local SSH process later. No remote job stop is inferred.
        drop(stdout);
        let _ = child.start_kill();
        match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
            Ok(Ok(_)) => return Err(error),
            Ok(Err(wait_error)) => {
                return Err(DagError::Schedule(format!(
                    "{error}; local SSH child wait failed: {wait_error}; local stop UNKNOWN"
                )));
            }
            Err(_) => {
                return Err(DagError::Schedule(format!(
                    "{error}; local SSH child wait timed out; local stop UNKNOWN"
                )));
            }
        }
    }
    Ok(())
}

// ── routing executor ─────────────────────────────────────────────────────

/// Coordinator executor that keeps every task on the local executor except
/// `kind == "process"` tasks, which route to the remote (Slurm) stack when
/// one is configured. Process tasks have no in-process node payload to run
/// — remote dispatch is their only execution path; typed nodes must stay
/// local because the remote path ignores the [`crate::dag::DagNode`]
/// payload entirely.
pub struct ProcessRoutingTaskExecutor {
    local: std::sync::Arc<dyn TaskExecutor>,
    remote: Option<std::sync::Arc<dyn TaskExecutor>>,
}

impl ProcessRoutingTaskExecutor {
    pub fn new(
        local: std::sync::Arc<dyn TaskExecutor>,
        remote: Option<std::sync::Arc<dyn TaskExecutor>>,
    ) -> Self {
        Self { local, remote }
    }
}

#[async_trait::async_trait]
impl TaskExecutor for ProcessRoutingTaskExecutor {
    fn requires_cancel_join(&self) -> bool {
        self.local.requires_cancel_join()
            || self
                .remote
                .as_ref()
                .is_some_and(|executor| executor.requires_cancel_join())
    }
    fn name(&self) -> &'static str {
        "process-routing"
    }

    async fn run(&self, mut execution: TaskExecution) -> JobResult {
        // The graph records a node's registered address as its kind
        // (`<plugin>/process`), while the remote contract speaks the bare
        // `process` kind. The node payload itself declares which contract it
        // implements, so route on that and normalize before dispatch — the
        // same rewrite the typed-worker lowering performs.
        if execution.submission.spec.kind != "process" && execution.node.kind() == "process" {
            execution.submission.spec.kind = "process".into();
        }
        if self.remote.is_none()
            && execution.submission.spec.kind == "process"
            && execution
                .submission
                .spec
                .spec
                .get("container")
                .is_some_and(|v| !v.is_null())
        {
            return JobResult::Failed {
                id: execution.submission.spec.id.clone(),
                error: DagError::Schedule(
                    "Apptainer tasks require an enabled Slurm backend; local fallback refused"
                        .into(),
                ),
                duration: Duration::ZERO,
                details: None,
            };
        }
        match &self.remote {
            Some(remote) if execution.submission.spec.kind == "process" => {
                remote.run(execution).await
            }
            _ => self.local.run(execution).await,
        }
    }
}

/// Assemble the full Slurm stack behind a routing executor: `process` tasks
/// go through [`RemoteTaskExecutor`] over [`SlurmTaskTransport`] +
/// [`SshArtifactStore`]; everything else stays on `local`.
pub fn slurm_backed_executor(
    settings: SlurmSettings,
    coordinator_workspace_root: impl Into<PathBuf>,
    local: std::sync::Arc<dyn TaskExecutor>,
) -> std::sync::Arc<dyn TaskExecutor> {
    let store = SshArtifactStore::new(
        settings.ssh.clone(),
        settings.scratch_dir.clone(),
        settings.local_artifact_root.clone(),
    );
    let transport = SlurmTaskTransport::new(settings);
    let remote = RemoteTaskExecutor::new(
        std::sync::Arc::new(transport),
        std::sync::Arc::new(store),
        coordinator_workspace_root,
    );
    std::sync::Arc::new(ProcessRoutingTaskExecutor::new(
        local,
        Some(std::sync::Arc::new(remote)),
    ))
}

#[cfg(test)]
#[path = "data_home_tests.rs"]
mod data_home_tests;
