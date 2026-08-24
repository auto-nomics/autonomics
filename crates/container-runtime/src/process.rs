use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::{io::AsyncReadExt, process::Command};

use crate::error::ContainerRuntimeError;

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

#[derive(Debug)]
pub(crate) struct PodmanCommandOutput {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Invoke one Podman command and cap its captured output.
///
/// Non-zero statuses are returned as `Ok` because some Podman commands use a
/// non-zero result as semantic information (for example, `image exists`).
pub(crate) async fn execute_podman(
    binary: &Path,
    args: &[String],
    timeout_secs: u64,
    max_output_bytes: usize,
) -> Result<PodmanCommandOutput, ContainerRuntimeError> {
    if timeout_secs == 0 {
        return Err(ContainerRuntimeError::Invalid(
            "timeout must be greater than zero".into(),
        ));
    }

    let mut command = Command::new(binary);
    command.args(args);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stdout_task =
        tokio::spawn(async move { capture_with_limit(&mut stdout, max_output_bytes).await });
    let stderr_task =
        tokio::spawn(async move { capture_with_limit(&mut stderr, max_output_bytes).await });

    let wait = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait()).await;
    let status = match wait {
        Ok(status) => status?,
        Err(_) => {
            if let Some(pid) = child.id() {
                #[cfg(unix)]
                kill_process_group(pid);

                #[cfg(not(unix))]
                let _ = pid;
            }
            child.wait().await?;
            return Err(ContainerRuntimeError::Timeout { timeout_secs });
        }
    };

    let stdout_bytes = stdout_task
        .await
        .unwrap_or_else(|e| Err(std::io::Error::other(e)))
        .unwrap_or_default();
    let stderr_bytes = stderr_task
        .await
        .unwrap_or_else(|e| Err(std::io::Error::other(e)))
        .unwrap_or_default();

    Ok(PodmanCommandOutput {
        success: status.success(),
        exit_code: status.code().unwrap_or(-1),
        stdout: lossy_prefix(&stdout_bytes, max_output_bytes),
        stderr: lossy_prefix(&stderr_bytes, max_output_bytes),
    })
}

pub(crate) async fn capture_with_limit<R>(
    mut reader: R,
    max_output_bytes: usize,
) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        if bytes.len() < max_output_bytes {
            let remaining = max_output_bytes.saturating_sub(bytes.len());
            bytes.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    }
    Ok(bytes)
}

pub(crate) fn lossy_prefix(bytes: &[u8], max_output_bytes: usize) -> String {
    let text = String::from_utf8_lossy(bytes).into_owned();
    if bytes.len() >= max_output_bytes {
        format!("{text}\n[output truncated at {max_output_bytes} bytes]")
    } else {
        text
    }
}

#[cfg(unix)]
pub(crate) fn kill_process_group(pid: u32) {
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
}

#[cfg(unix)]
pub(crate) fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(unix)]
pub(crate) fn current_gid() -> u32 {
    unsafe { libc::getgid() }
}
