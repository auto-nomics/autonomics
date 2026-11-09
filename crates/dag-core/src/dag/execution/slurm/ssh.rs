//! Ssh plumbing shared by every remote transport: the connection target
//! (pinned endpoints, per-invocation options), streamed upload sources,
//! and small shell/quoting helpers.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::contract::{TaskAttemptReceipt, TaskDispatch, TaskExecution};
use super::super::process::{
    ProcessTaskSpec, parse_process_task, process_task_environment, render_process_script,
    sanitize_path_component,
};
use super::super::traits::{TaskArtifactStore, TaskExecutor, TaskLease, TaskTransport};
use crate::dag::DagError;
use crate::dag::node_event::JobResult;
use crate::value::{DomainId, FileFingerprint, FileRef};

// ── ssh plumbing ─────────────────────────────────────────────────────────
/// Where and how to reach the cluster login node over ssh.
#[derive(Debug, Clone)]
pub struct SshTarget {
    pub(crate) endpoint: Option<vfs::SshEndpoint>,
    pub(crate) shared_endpoints: BTreeMap<String, vfs::SshEndpoint>,
    pub(crate) program: String,
    pub(crate) host: String,
    pub(crate) connect_timeout: Duration,
    pub(crate) command_timeout: Duration,
    pub(crate) transfer_timeout: Duration,
    /// Keepalive cadence for connections that outlive a short command. Absent
    /// means every connection is short-lived and needs no liveness probing.
    pub(crate) keepalive: Option<(Duration, u32)>,
    pub(crate) pinned_configuration: Option<(FileRef, Vec<FileRef>)>,
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
    pub(crate) fn verify_endpoints(&self) -> Result<(), DagError> {
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
    pub(crate) fn command(&self) -> Result<tokio::process::Command, DagError> {
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
            vfs::ssh_channel::batch_mode_option().into(),
            "LogLevel=ERROR".into(),
            "StrictHostKeyChecking=yes".into(),
            connect,
        ];
        options.extend(vfs::ssh_channel::options());
        if let Some((interval, count)) = &self.keepalive {
            options.push(format!("ServerAliveInterval={}", interval.as_secs().max(1)));
            options.push(format!("ServerAliveCountMax={count}"));
        }
        for option in &options {
            command.args(["-o", option.as_str()]);
        }
        command.arg(&self.host);
        // Vault-backed password/OTP answering, when a vault agent is
        // configured; a no-op otherwise.
        vfs::ssh_channel::attach_credentials(&mut command, &self.host);
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
    pub(crate) async fn run_with_admitted_file_stdin_observed(
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

    /// One-shot connection command with live, bounded stdout/stderr observations.
    pub async fn run_script_once_with_output(
        &self,
        script: &str,
        observe: &(dyn Fn(&str, &str) + Send + Sync),
    ) -> Result<Output, DagError> {
        async fn collect(
            mut pipe: impl tokio::io::AsyncRead + Unpin,
            stream: &str,
            observe: &(dyn Fn(&str, &str) + Send + Sync),
        ) -> std::io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let count = pipe.read(&mut chunk).await?;
                if count == 0 { return Ok(bytes); }
                if bytes.len() + count > 1024 * 1024 {
                    return Err(std::io::Error::other("connection output exceeds 1 MiB limit"));
                }
                bytes.extend_from_slice(&chunk[..count]);
                observe(stream, &String::from_utf8_lossy(&chunk[..count]));
            }
        }
        let mut command = self.command()?;
        command.args(["bash", "-s"]).stdin(Stdio::piped())
            .stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = command.spawn().map_err(|e| DagError::Schedule(format!("cannot start SSH: {e}")))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let operation = async {
            let writer = async {
                let result = match stdin.write_all(script.as_bytes()).await {
                    Ok(()) => stdin.shutdown().await,
                    Err(error) => Err(error),
                };
                drop(stdin);
                // Preserve authentication diagnostics even if SSH closes stdin early.
                Ok::<_, std::io::Error>(result.err())
            };
            let (stdin_error, stdout, stderr, status) = tokio::try_join!(
                writer, collect(stdout, "stdout", observe), collect(stderr, "stderr", observe), child.wait()
            )?;
            if let Some(error) = stdin_error && status.success() {
                return Err(std::io::Error::other(format!("cannot pipe ssh stdin: {error}; complete payload was not confirmed; stdout: {}; stderr: {}", String::from_utf8_lossy(&stdout), String::from_utf8_lossy(&stderr))));
            }
            Ok::<_, std::io::Error>(Output { status, stdout, stderr })
        };
        tokio::time::timeout(self.command_timeout, operation).await
            .map_err(|_| DagError::Schedule("SSH connection command timed out; remote outcome UNKNOWN".into()))?
            .map_err(|e| DagError::Schedule(format!("SSH connection command failed: {e}")))
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
                // An authentication rejection may close stdin before the payload
                // is written. Still reap SSH and retain its stdout/stderr.
                let stdin_error = if let StdinSource::Bytes(bytes) = &stdin {
                    if let Some(mut handle) = child.stdin.take() {
                        let result = match handle.write_all(bytes).await {
                            Ok(()) => handle.shutdown().await,
                            Err(error) => Err(error),
                        };
                        result.err()
                    } else {
                        None
                    }
                } else {
                    None
                };
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
                let output = result.map_err(|error| {
                    DagError::Schedule(format!("ssh command `{}` failed: {error}", args.join(" ")))
                })?;
                if let Some(error) = stdin_error
                    && output.status.success()
                {
                    return Err(DagError::Schedule(format!(
                        "cannot pipe ssh stdin: {error}; remote exited successfully before the complete payload was confirmed; stdout: {}; stderr: {}",
                        String::from_utf8_lossy(&output.stdout).trim(),
                        String::from_utf8_lossy(&output.stderr).trim()
                    )));
                }
                Ok(output)
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
pub(crate) fn admit_upload_source(path: &Path) -> Result<std::fs::File, DagError> {
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
pub(crate) fn ensure_ssh_success(output: Output, context: &str) -> Result<Output, DagError> {
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
pub(crate) fn queue_stdout_text(output: Output, context: &str) -> Result<String, DagError> {
    if slurm_queue_absent(&output) {
        Ok(String::new())
    } else {
        ssh_stdout_text(output, context)
    }
}
pub(crate) fn ssh_stdout_text(output: Output, context: &str) -> Result<String, DagError> {
    ensure_ssh_success(output, context)
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
}
/// Quote an arbitrary string as one shell word. Used for every env value
/// and remote path embedded in generated scripts, so neither the
/// coordinator nor a task spec can inject shell syntax.
pub(crate) fn sh_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
/// Lowercase hex without pulling in a hex crate (house pattern).
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);
/// A per-attempt unique tag (`bytes` bytes = twice as many lowercase hex
/// chars) hashed from clock/pid/counter — `rand` is not a dependency and
/// this only needs collision-unlikelihood within one daemon's lifetime.
pub(crate) fn nonce_hex(bytes: usize) -> String {
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
pub(crate) fn slurm_job_name(task_id: &str, nonce: &str) -> String {
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
