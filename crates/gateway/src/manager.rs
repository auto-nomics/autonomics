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

/// Ask a running daemon to shut down gracefully.
pub async fn stop() -> Result<(), String> {
    let addr = env_addr();
    let token = read_token();
    let client = GatewayClient::new(&addr, token.as_deref()).map_err(|e| e.to_string())?;
    client.shutdown_gateway().await.map_err(|e| e.to_string())
}
