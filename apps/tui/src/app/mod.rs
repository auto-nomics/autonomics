use std::sync::Arc;
use std::time::{Duration, Instant};

use agentik_sdk::AuthMethod;
use agentik_sdk::model::{Model, ModelInfo, ProviderConfig, ProviderType};
use agentik_sdk::types::AgentEvent;
use agentik_sdk::types::messages::{ContentBlock, Message, Role};
use arc_swap::ArcSwapOption;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind,
};
use ratatui::{
    Frame,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    prelude::{Terminal, Widget},
};
use rusqlite::Connection;
use std::io::{Stdout, Write, stdout};
use uuid::Uuid;

use crate::state::{self, AgentSession, AgentStatus, AppState, ChatLine, InputMode};
use crate::widgets::agent_workspace::AgentWorkspace;
use agentik_core::AgentProfile;
use runtime::{AgentHandle, RuntimeHost};

/// Lines scrolled by a half-page motion (PageDown / PageUp in browse mode).
const HALF_PAGE: usize = 12;

/// If the user presses Ctrl+C again within this window after a cooperative
/// cancel, the app force-quits regardless of agent status.
const FORCE_QUIT_WINDOW: Duration = Duration::from_secs(3);

mod agents;
mod chat;
mod commands;
mod event_loop;
mod history;
mod keyboard;
mod model_config;
mod render;
mod runtime_events;
mod sessions;
mod terminal;

pub struct App {
    state: AppState,
    /// Multi-agent host owning shared infrastructure.
    host: Option<RuntimeHost>,
    /// Per-agent handles, parallel to `state.sessions`.
    handles: Vec<AgentHandle>,
    /// Kept alive to drive the agent's background event loop task.
    _runtime: Option<tokio::runtime::Runtime>,
    /// Handle for spawning background tasks from within the sync event loop.
    runtime_handle: tokio::runtime::Handle,
    /// Local HTTP API backend, shut down with the TUI process.
    http_server: Option<tui_http::HttpServerHandle>,
    conn: Connection,
    /// Holds clipboard ownership on platforms where dropping it clears the clipboard.
    clipboard_lease: Option<crate::clipboard_copy::ClipboardLease>,
    /// Internal event channel for decoupled communication.
    app_event_rx: tokio::sync::mpsc::UnboundedReceiver<crate::app_event::AppEvent>,
    /// Sender half exposed for subsystems (file search, plugins, etc.)
    #[allow(dead_code)]
    pub(crate) app_event_tx: crate::app_event_sender::AppEventSender,
    should_quit: bool,
    cancel_requested_at: Option<Instant>,
    dirty: bool,
}

impl App {
    pub fn new() -> Self {
        let config = runtime::RuntimeConfig::default();

        // Ensure state_dir exists before opening DBs inside it.
        if let Some(parent) = config.app_db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let conn = app_config::open(&config.app_db_path).unwrap_or_else(|e| {
            panic!("failed to open {}: {e}", config.app_db_path.display())
        });

        let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let model = Arc::new(ArcSwapOption::from_pointee(app_config::build_model(&conn)));

        // ── Open RuntimeHost + load profiles ──────────────────────
        let (mut host, profiles) = runtime.block_on(async {
            tracing::info!("startup: opening RuntimeHost");
            let host = match RuntimeHost::open(&config).await {
                Ok(h) => {
                    tracing::info!("runtime host opened successfully");
                    Some(h)
                }
                Err(runtime::Error::InstanceLockHeld { path }) => {
                    // Another Autonomics process (desktop shell, or a second
                    // TUI) owns the state-dir single-writer lock. The
                    // terminal is not in raw mode yet — App::start sets that
                    // up later — so plain stderr + exit is safe. Design doc
                    // §9: the second instance errors out instead of
                    // double-writing agent.db / bib.db.
                    eprintln!(
                        "Autonomics 已在另一个实例中运行（{} 被占用）。\n\
                         请先关闭正在运行的 TUI 或桌面版，再重新启动。",
                        path.display()
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "failed to open runtime host — agent spawn/resume will not work"
                    );
                    None
                }
            };

            tracing::info!("startup: seeding default profiles + loading");
            let profiles = if let Some(ref h) = host {
                use agentik_core::AgentProfileRegistry;
                let storage = h.infra().profile_storage.clone();
                let _ = storage.seed_defaults_if_empty().await;
                storage.list_profiles().await.unwrap_or_default()
            } else {
                Vec::new()
            };
            tracing::info!("startup: loaded {} profile(s)", profiles.len());
            (host, profiles)
        });

        let mut state = AppState {
            active_model: model,
            profiles: profiles.clone(),
            ..Default::default()
        };

        // Share profiles + model with RuntimeHost for tool-driven agent spawning.
        if let Some(ref mut host) = host {
            host.set_profiles(profiles);
            host.set_model(state.active_model.clone());
        }

        Self::load_model_config(&conn, &mut state.model_config_state);

        // Sync profiles to the command palette and picker.
        state.command_palette.set_profiles(&state.profiles);
        state.profile_picker.set_profiles(state.profiles.clone());

        // ── Load display settings from the settings table ──
        for (key, field) in [
            (
                "collapse_thinking",
                &mut state.display_settings.collapse_thinking,
            ),
            (
                "collapse_tool_calls",
                &mut state.display_settings.collapse_tool_calls,
            ),
            (
                "collapse_tool_results",
                &mut state.display_settings.collapse_tool_results,
            ),
        ] {
            if let Ok(value) = conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get::<_, String>(0),
            ) {
                *field = value == "1";
            }
        }

        let (app_event_tx, app_event_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime_handle = runtime.handle().clone();
        let http_server = Self::start_http_server(&runtime, host.as_ref(), state.active_model.clone());
        if let Some(ref server) = http_server {
            state.toasts.info("HTTP API started", Some(server.url()));
        } else {
            state.toasts.error(
                "HTTP API unavailable",
                Some("See logs for the bind or runtime error".to_owned()),
            );
        }

        Self {
            state,
            host,
            handles: Vec::new(),
            _runtime: Some(runtime),
            runtime_handle: runtime_handle.clone(),
            http_server,
            conn,
            clipboard_lease: None,
            app_event_rx,
            app_event_tx: crate::app_event_sender::AppEventSender::new(app_event_tx),
            should_quit: false,
            cancel_requested_at: None,
            dirty: true,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
