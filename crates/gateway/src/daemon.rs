//! The daemon entry point — `autonomics serve`.
//!
//! Startup order (each step before the next):
//!
//! 1. resolve config + bind address + token policy;
//! 2. open the model store (app DB — single writer via the instance
//!    lock taken inside `RuntimeHost::open`);
//! 3. open the RuntimeHost, seed/load profiles, install the default
//!    model slot;
//! 4. build the EventHub + session cache + shared gateway state;
//! 5. bind the HTTP server, and only then write the token/pid files
//!    (a frontend probing `/gateway/status` never sees a half-started
//!    daemon);
//! 6. run the driver loop until shutdown; then graceful-stop the server
//!    and all agents.

use std::sync::Arc;

use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use runtime::{RuntimeConfig, RuntimeHost};
use tokio_util::sync::CancellationToken;

use crate::driver::{self, SessionCache};
use crate::hub::EventHub;
use crate::model_store::ModelStore;
use crate::server::DockerHubClient;
use crate::server::{GatewayState, router_with_bib};

pub const GATEWAY_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where `ensure_running` looks for the daemon's bearer token.
pub const TOKEN_FILE: &str = "gateway.token";
/// Where the daemon's pid is recorded for diagnostics.
pub const PID_FILE: &str = "gateway.pid";

/// Errors from running the daemon.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error(
        "another instance holds the single-writer lock: {path} — is a gateway already running? (`autonomics serve stop` to stop it)"
    )]
    InstanceLockHeld { path: std::path::PathBuf },
    #[error("failed to open runtime host: {0}")]
    HostOpen(#[from] runtime::Error),
    #[error("model store error: {0}")]
    ModelStore(#[from] crate::model_store::ModelStoreError),
    #[error("failed to bind HTTP server on {addr}: {source}")]
    Bind {
        addr: String,
        source: std::io::Error,
    },
    #[error("{0}")]
    Message(String),
}

/// Options for [`run_daemon`].
pub struct DaemonOptions {
    pub config: RuntimeConfig,
    /// Bind address (default `127.0.0.1:8765`, `AUTONOMICS_HTTP_API_ADDR`).
    pub addr: String,
    /// Explicit bearer token (from `AUTONOMICS_HTTP_API_TOKEN`); when
    /// absent a fresh token is generated per start and written to
    /// `<state_dir>/gateway.token`.
    pub env_token: Option<String>,
}

/// Resolve the bind address and token policy from the environment
/// (shared by the daemon and the manager so they always agree).
pub fn env_addr() -> String {
    std::env::var("AUTONOMICS_HTTP_API_ADDR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| api_server::server::DEFAULT_HTTP_API_ADDR.to_owned())
}

pub fn env_token() -> Option<String> {
    std::env::var("AUTONOMICS_HTTP_API_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub fn token_path(config: &RuntimeConfig) -> std::path::PathBuf {
    config.state_dir.join(TOKEN_FILE)
}

/// Run the daemon until the shutdown token fires (POST `/gateway/shutdown`
/// or, for foreground mode, the caller cancels it).
pub async fn run_daemon(
    opts: DaemonOptions,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    let DaemonOptions {
        config,
        addr,
        env_token,
    } = opts;

    // ── Model store (app DB) ─────────────────────────────────────────
    let models = Arc::new(ModelStore::open(&config.app_db_path)?);
    let hub = EventHub::new();

    // ── RuntimeHost (takes the instance lock) ────────────────────────
    let mut host = match RuntimeHost::open(&config).await {
        Ok(host) => host,
        Err(runtime::Error::InstanceLockHeld { path }) => {
            return Err(DaemonError::InstanceLockHeld { path });
        }
        Err(e) => return Err(e.into()),
    };

    // ── Profiles + default model ─────────────────────────────────────
    let profile_storage = host.infra().profile_storage.clone();
    let _ = profile_storage.seed_defaults_if_empty().await;
    let profiles = profile_storage.list_profiles().await.unwrap_or_default();
    host.set_profiles(profiles.clone());

    let model_slot: Arc<ArcSwapOption<Model>> =
        Arc::new(ArcSwapOption::from_pointee(models.active_model(&hub)));
    host.set_model(model_slot.clone());

    // Rehydrate the daemon-owned multi-agent layout before HTTP state is
    // built, so the first frontend snapshot already sees the restored agents.
    // A startup-storage failure is fatal; malformed individual rows are
    // logged and skipped by the restore loop itself.
    let restored_agents = host
        .restore_persisted_agents(&profiles, model_slot.clone(), |spec| {
            models.resolve_with_refresh_callback(spec, &hub)
        })
        .await
        .map_err(|e| DaemonError::Message(format!("restore agent layout: {e}")))?;
    tracing::info!(
        count = restored_agents,
        "persisted multi-agent layout restored"
    );

    // Startup proactive ChatGPT token refresh (8-day/24h rule).
    models.spawn_ensure_fresh(hub.clone());

    // ── Gateway state + server ───────────────────────────────────────
    let sessions = SessionCache::new();
    // The bound address is only known after `start()` — bind resolves
    // after the router is built — so handlers read it through this cell,
    // installed once the server owns its socket (before the token file
    // is published, so no authenticated client can observe it empty).
    let addr_slot: Arc<ArcSwapOption<String>> = Arc::new(ArcSwapOption::default());
    let state = GatewayState {
        hub: hub.clone(),
        sessions: sessions.clone(),
        control: host.control(),
        infra: host.infra(),
        dockerhub: Arc::new(DockerHubClient::default()),
        models: models.clone(),
        model_slot: model_slot.clone(),
        profiles: Arc::new(profiles),
        addr: addr_slot.clone(),
        started: std::time::Instant::now(),
        shutdown: shutdown.clone(),
        version: GATEWAY_VERSION,
    };

    let gateway_token = match env_token.as_deref() {
        Some(token) => token.to_string(),
        None => generate_token(),
    };
    let bib_shared = state.infra.bib.as_ref().clone();
    // Clone for the extraction sweep before the move into the router.
    let sweep_shared = bib_shared.clone();
    let router = router_with_bib(state, gateway_token.clone(), bib_shared, env_token.clone());

    let server = api_server::server::start(router, &addr)
        .await
        .map_err(|source| DaemonError::Bind {
            addr: addr.clone(),
            source,
        })?;

    // Server is up — publish the bound address to the handlers, then the
    // token + pid files. Permissions are owner-only: the token is a
    // filesystem permission gate for local processes.
    addr_slot.store(Some(Arc::new(server.addr().to_string())));
    if env_token.is_none() {
        let token_path = token_path(&config);
        write_private_file(&token_path, &gateway_token);
    }
    write_private_file(
        &config.state_dir.join(PID_FILE),
        &std::process::id().to_string(),
    );

    tracing::info!(
        addr = %server.addr(),
        "gateway daemon listening (agents keep running after frontends disconnect)"
    );

    // Resume unfinished full-text extractions (rows left pending/running
    // by a previous session) — MinerU sweep, previously kicked off by the
    // TUI's HTTP server startup. Fire-and-forget: the semaphore inside
    // BibShared caps the concurrency.
    agentik_core::supervise::spawn_safe_drop("bib-extraction-sweep", async move {
        bib_base::sweep_pending(&sweep_shared).await;
    });

    // ── Driver loop (sole consumer of the host) ──────────────────────
    let mut host = driver::run(host, hub, sessions, shutdown.clone()).await;

    // ── Graceful shutdown ────────────────────────────────────────────
    if let Err(error) = server.shutdown().await {
        tracing::warn!(error = %error, "failed to shut down HTTP server");
    }
    if tokio::time::timeout(
        std::time::Duration::from_secs(30),
        host.shutdown_all_agents_and_wait(),
    )
    .await
    .is_err()
    {
        tracing::warn!("agents did not shut down within grace; dropping host");
    }
    let _ = std::fs::remove_file(config.state_dir.join(PID_FILE));
    tracing::info!("gateway daemon stopped");
    Ok(())
}

fn generate_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Write a file with owner-only permissions (best-effort on non-unix).
fn write_private_file(path: &std::path::Path, contents: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let result = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(contents.as_bytes()));
    if let Err(error) = result {
        tracing::warn!(path = %path.display(), error = %error, "failed to write gateway file");
    }
}
