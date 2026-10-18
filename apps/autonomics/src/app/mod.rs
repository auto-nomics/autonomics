use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use uuid::Uuid;

use agentik_core::AgentKind;
use agentik_sdk::types::AgentEvent;
use agentik_sdk::types::messages::Message;
use agentik_types::SessionInfo;

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
use std::io::{Stdout, Write, stdout};

use crate::state::{self, AgentSession, AgentStatus, AppState, ChatLine, InputMode};
use crate::widgets::agent_workspace::AgentWorkspace;

/// Lines scrolled by a half-page motion (PageDown / PageUp in browse mode).
const HALF_PAGE: usize = 12;

mod agent_config;
mod agents;
mod chat;
mod commands;
mod dag_view;
mod embedding_config;
mod event_loop;
mod history;
mod keyboard;
mod model_config;
mod render;
mod runtime_events;
mod sessions;
mod skill_evolution;
mod terminal;

pub struct App {
    state: AppState,
    /// Thin-client connection to the resident gateway daemon — the only
    /// path to the runtime. Agents keep running in the daemon when the
    /// TUI exits.
    client: gateway::GatewayClient,
    /// Gateway event frames (agent / host / notice) forwarded by the SSE
    /// pump task. Drained in the main loop's `select!`.
    gateway_rx: tokio::sync::mpsc::UnboundedReceiver<gateway::client::GatewayFrame>,
    /// Kept alive to drive background tasks (client calls, history loads).
    runtime: Option<tokio::runtime::Runtime>,
    /// Handle for spawning background tasks from within the sync event loop.
    runtime_handle: tokio::runtime::Handle,
    /// Holds clipboard ownership on platforms where dropping it clears the clipboard.
    clipboard_lease: Option<crate::clipboard_copy::ClipboardLease>,
    /// Internal event channel for decoupled communication.
    app_event_rx: tokio::sync::mpsc::UnboundedReceiver<crate::app_event::AppEvent>,
    /// Sender half exposed for subsystems (file search, plugins, etc.)
    pub(crate) app_event_tx: crate::app_event_sender::AppEventSender,
    /// Cached per-agent model info `(name, context_length)` for the render
    /// path. Render must never issue HTTP — entries are prefetched on
    /// demand here and filled asynchronously via
    /// [`AppEvent::ModelInfoLoaded`].
    agent_model_cache: HashMap<String, crate::state::ModelDisplay>,
    /// Agents whose model info fetch is in flight (prevents per-frame
    /// request storms while a fetch is pending).
    model_info_pending: HashSet<String>,
    should_quit: bool,
    cancel_requested_at: Option<Instant>,
    dirty: bool,
}

impl App {
    pub fn new() -> color_eyre::Result<Self> {
        let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

        // ── Connect to the gateway daemon, then hydrate ────────────────
        // The daemon must already be running — the TUI never spawns
        // one. Snapshot first, pump after: the pump starts from
        // `snapshot.last_seq` so frames published between the two calls
        // are neither missed nor double-applied (ring replay covers the
        // gap; a lag frame triggers re-hydration).
        let (client, gateway_rx, snapshot) = runtime
            .block_on(async {
                match gateway::manager::probe().await {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        return Err(color_eyre::eyre::eyre!(
                            "the gateway daemon is not running — start it with \
                             `autonomics serve` (or run `autonomics` for guidance)"
                        ));
                    }
                    Err(e) => {
                        return Err(color_eyre::eyre::eyre!(
                            "cannot reach the gateway daemon: {e} \
                             (check `autonomics serve status`)"
                        ));
                    }
                }
                let token = gateway::manager::read_token();
                let addr = gateway::daemon::env_addr();
                let client = gateway::GatewayClient::new(&addr, token.as_deref())
                    .map_err(|e| color_eyre::eyre::eyre!("gateway client: {e}"))?;
                let snapshot = client
                    .state()
                    .await
                    .map_err(|e| color_eyre::eyre::eyre!("gateway hydration: {e}"))?;
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                gateway::client::EventPump::spawn(client.clone(), snapshot.last_seq, tx);
                color_eyre::eyre::Ok((client, rx, snapshot))
            })
            .map_err(|e| color_eyre::eyre::eyre!("failed to connect to the gateway daemon: {e}"))?;

        let mut state = AppState {
            profiles: snapshot.profiles.clone(),
            ..Default::default()
        };

        // Active default model: spec + display tuple (provider:model spec,
        // context) from the catalog rows.
        state.active_model_spec = snapshot.active_model_spec.clone();
        state.active_model_display = snapshot.active_model_spec.as_deref().and_then(|spec| {
            let (provider, name) = spec.split_once(':')?;
            snapshot
                .model_catalog
                .models
                .iter()
                .find(|m| m.provider_name == provider && m.model_name == name)
                .map(|m| crate::state::ModelDisplay {
                    spec: spec.to_string(),
                    context_length: m.context_length,
                    // The catalogue rows don't carry effort; the agent
                    // cache fills it once an agent is active.
                    reasoning_effort: None,
                })
        });

        // Display settings from the daemon-owned settings table.
        state.display_settings.collapse_thinking = snapshot.display_settings.collapse_thinking;
        state.display_settings.collapse_tool_calls = snapshot.display_settings.collapse_tool_calls;
        state.display_settings.collapse_tool_results =
            snapshot.display_settings.collapse_tool_results;

        // Model catalog for the config widget.
        Self::load_model_config(&snapshot.model_catalog, &mut state.model_config_state);

        // Live agents + their session lists.
        for info in &snapshot.agents {
            state.sessions.push(state::AgentSession {
                name: info.path.clone(),
                agent_id: info.agent_id.unwrap_or_default(),
                sub_sessions: Vec::new(),
                active_sub_session_idx: 0,
                pending_tab_state: Default::default(),
            });
        }

        // Sync profiles to the command palette and picker.
        state.command_palette.set_profiles(&state.profiles);
        state.profile_picker.set_profiles(state.profiles.clone());

        let (app_event_tx, app_event_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime_handle = runtime.handle().clone();

        let mut app = Self {
            state,
            client,
            gateway_rx,
            runtime: Some(runtime),
            runtime_handle: runtime_handle.clone(),
            clipboard_lease: None,
            app_event_rx,
            app_event_tx: crate::app_event_sender::AppEventSender::new(app_event_tx),
            agent_model_cache: HashMap::new(),
            model_info_pending: HashSet::new(),
            should_quit: false,
            cancel_requested_at: None,
            dirty: true,
        };

        // Fold known session lists into the UI and load transcripts for
        // empty tabs (agents already running in the daemon).
        let known_sessions = snapshot.sessions.clone();
        for (agent_path, sessions) in known_sessions {
            app.apply_session_list(&agent_path, sessions);
        }
        app.spawn_all_history_loads();
        Ok(app)
    }
}
