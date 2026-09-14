//! Daemon lifecycle management for frontends: probe, auto-spawn, stop.

use std::time::Duration;

use crate::client::GatewayClient;
use crate::daemon::{env_addr, env_token, token_path};
use crate::proto::GatewayStatus;

/// How long `ensure_running` waits for a freshly spawned daemon to come
/// up (opening the full SharedInfra — Turso DBs, podman connection — can
/// take seconds on a cold start).
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

/// Probe the daemon without spawning anything. `Ok(None)` = not running.
pub async fn probe() -> Result<Option<GatewayStatus>, String> {
    let addr = env_addr();
    let token = read_token();
    let client = GatewayClient::new(&addr, token.as_deref()).map_err(|e| e.to_string())?;
    client.gateway_status_opt().await.map_err(|e| e.to_string())
}

/// Read the daemon token: explicit env override first, then the token
/// file written at daemon startup.
pub fn read_token() -> Option<String> {
    if let Some(token) = env_token() {
        return Some(token);
    }
    let path = token_path(&runtime::RuntimeConfig::default());
    std::fs::read_to_string(path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Ensure a daemon is running, spawning one from the current executable
/// if the probe fails. Frontends call this before connecting.
///
/// The spawned process is fully detached: null stdio (never steal the
/// spawning process's terminal — the podman.rs lesson) and its own
/// process group, so the frontend exiting never takes the daemon down.
pub async fn ensure_running() -> Result<(), String> {
    if let Some(status) = probe().await? {
        tracing::debug!(pid = status.pid, "reusing running gateway daemon");
        return Ok(());
    }

    let exe =
        std::env::current_exe().map_err(|e| format!("cannot resolve current executable: {e}"))?;
    let spawned =
        spawn_detached_daemon(&exe).map_err(|e| format!("failed to spawn gateway daemon: {e}"))?;
    tracing::info!(pid = spawned, "gateway daemon spawned");

    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if let Some(status) = probe().await? {
            tracing::info!(pid = status.pid, "gateway daemon ready");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(format!(
        "gateway daemon did not become ready within {}s (see the gateway log under the state dir)",
        STARTUP_TIMEOUT.as_secs()
    ))
}

/// Spawn `exe serve --daemon` detached from the current process group
/// with all stdio nulled. Returns the child pid.
fn spawn_detached_daemon(exe: &std::path::Path) -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt;

    let mut command = std::process::Command::new(exe);
    command
        .args(["serve", "--daemon"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0);
    command.spawn().map(|child| child.id())
}

/// Ask a running daemon to shut down gracefully.
pub async fn stop() -> Result<(), String> {
    let addr = env_addr();
    let token = read_token();
    let client = GatewayClient::new(&addr, token.as_deref()).map_err(|e| e.to_string())?;
    client.shutdown_gateway().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_timeout_is_generous() {
        // Cold-starting SharedInfra (Turso DBs, podman) takes seconds.
        assert!(STARTUP_TIMEOUT >= Duration::from_secs(10));
    }
}
