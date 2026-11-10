//! Daemon lifecycle management for frontends: probe, stop.
//!
//! Frontends never spawn the daemon — the operator starts it
//! (`autonomics serve`) after provisioning (`autonomics prepare`).

use crate::client::GatewayClient;
use crate::daemon::{env_addr, env_token, token_path};
use crate::proto::GatewayStatus;

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

/// Stop the running daemon: SIGTERM, then a synchronous wait for exit.
///
/// The daemon's SIGTERM handler cancels its shutdown token and runs the
/// same graceful path the removed HTTP endpoint triggered (HTTP server
/// drain, 30s agent grace, pid-file removal). This function returns only
/// once the process is gone, so `serve stop && serve start` cannot race
/// the instance lock. Identity is confirmed before signaling: the
/// authenticated status endpoint's pid must agree with the `gateway.pid`
/// file when both exist; if HTTP is unreachable but the pid file does,
/// the hung or half-started daemon is still signaled — a process too
/// wedged to run its handler still dies via SIGTERM's default
/// disposition.
pub async fn stop() -> Result<(), String> {
    let pid = resolve_daemon_pid().await?;
    signal_daemon(pid)?;
    wait_for_exit(pid).await
}

/// How long `stop` waits for the daemon to exit after SIGTERM: the
/// daemon's own agent grace is 30s, this leaves margin for the server
/// drain and runtime teardown.
const STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

fn pid_file_path() -> std::path::PathBuf {
    runtime::RuntimeConfig::default()
        .state_dir
        .join(crate::daemon::PID_FILE)
}

/// Decide which pid to signal, cross-checking the live status response
/// against the recorded pid file.
async fn resolve_daemon_pid() -> Result<i32, String> {
    let pid_file = pid_file_path();
    let recorded = std::fs::read_to_string(&pid_file)
        .ok()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty());
    match probe().await {
        Ok(Some(status)) => {
            let live = status.pid as i32;
            if let Some(recorded) = recorded.filter(|recorded| recorded != &live.to_string()) {
                return Err(format!(
                    "pid mismatch: status reports {live} but {} records {recorded} \
                     — refusing to signal; inspect `autonomics serve status`",
                    pid_file.display()
                ));
            }
            Ok(live)
        }
        // No listener and no pid file: nothing to stop.
        Ok(None) => Err("gateway not running".to_owned()),
        // HTTP unreachable: fall back to the pid file so a daemon that
        // can't serve status (hung, half-started) is still stoppable.
        Err(probe_error) => recorded
            .and_then(|recorded| recorded.parse::<i32>().ok())
            .ok_or_else(|| {
                format!(
                    "gateway status unreachable ({probe_error}) and no readable pid file at {}",
                    pid_file.display()
                )
            }),
    }
}

fn signal_daemon(pid: i32) -> Result<(), String> {
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc != 0 {
        let error = std::io::Error::last_os_error();
        return Err(format!("failed to signal gateway pid {pid}: {error}"));
    }
    Ok(())
}

/// Poll until the daemon process is gone. Process death — not pid-file
/// removal — is the criterion: the file is deleted slightly before the
/// instance lock releases, and returning on it could let an immediate
/// `serve start` lose the lock race.
async fn wait_for_exit(pid: i32) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
    loop {
        // `kill(pid, 0)` probes existence without delivering a signal.
        // The daemon is not our child (the launcher exited long ago), so
        // it cannot linger as our zombie; ESRCH means it is gone.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        if !alive {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "gateway pid {pid} did not exit within {}s — it may be stuck mid-shutdown; \
                 inspect logs and kill it manually if needed",
                STOP_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}
