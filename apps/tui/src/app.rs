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

use crate::state::{self, AgentSession, AgentStatus, AppState, InputMode};
use crate::widgets::agent_workspace::AgentWorkspace;
use agentik_core::{AgentProfile, TursoAgentStorage};
use runtime::{AgentHandle, RuntimeHost};

/// Lines scrolled by a half-page motion (PageDown / PageUp in browse mode).
const HALF_PAGE: usize = 12;

/// If the user presses Ctrl+C again within this window after a cooperative
/// cancel, the app force-quits regardless of agent status.
const FORCE_QUIT_WINDOW: Duration = Duration::from_secs(3);

/// Restore the terminal to its normal state: disable mouse capture, leave
/// alternate screen, and disable raw mode. Called on both normal exit and
/// panic (via the panic hook).
fn restore_terminal() -> std::io::Result<()> {
    crossterm::execute!(
        stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        crossterm::terminal::LeaveAlternateScreen,
    )?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

/// Install a panic hook that restores the terminal before running the
/// original hook. This ensures the user's shell is usable even if the TUI
/// panics. Modeled after codex's `tui.rs:set_panic_hook`.
fn set_panic_hook() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        hook(info);
    }));
}

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
    conn: Connection,
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
        let conn = Connection::open("phloem.db").expect("failed to open phloem.db");

        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("failed to enable foreign_keys");

        Self::init_database(&conn).expect("failed to initialize database schema");

        let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let model = Arc::new(ArcSwapOption::from_pointee(Self::build_model(&conn)));

        // ── Open RuntimeHost + load profiles ──────────────────────
        let config = runtime::RuntimeConfig::default();
        let (host, profiles) = runtime.block_on(async {
            // Open storage directly for profile seeding/loading (the host
            // also opens it, but we need AgentProfileRegistry trait methods
            // which aren't on the AgentStorage trait object).
            let storage = TursoAgentStorage::open(&config.agent_db).await.ok();
            let profiles = if let Some(ref s) = storage {
                use agentik_core::storage::AgentProfileRegistry;
                let _ = s.seed_defaults_if_empty().await;
                s.list_profiles().await.unwrap_or_default()
            } else {
                Vec::new()
            };

            // Now open the host (it will open the same DB again — Turso WAL
            // mode supports concurrent connections from the same process).
            let host = RuntimeHost::open(&config).await.ok();
            (host, profiles)
        });

        let mut state = AppState {
            active_model: model,
            profiles,
            ..Default::default()
        };

        Self::load_model_config(&conn, &mut state.model_config_state);

        // Sync profiles to the command palette and picker.
        state.command_palette.set_profiles(&state.profiles);
        state.profile_picker.set_profiles(state.profiles.clone());

        let (app_event_tx, app_event_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime_handle = runtime.handle().clone();

        Self {
            state,
            host,
            handles: Vec::new(),
            _runtime: Some(runtime),
            runtime_handle: runtime_handle.clone(),
            conn,
            app_event_rx,
            app_event_tx: crate::app_event_sender::AppEventSender::new(app_event_tx),
            should_quit: false,
            cancel_requested_at: None,
            dirty: true,
        }
    }

    /// Build a `Model` from the DB settings + built-in catalog.
    ///
    /// Reads `active_model` from the `settings` table (format:
    /// `"provider_name:model_name"`), looks up the model in the SDK registry,
    /// and joins it with provider credentials from the `providers` table.
    /// Returns `None` if no model is configured, credentials are missing,
    /// or any lookup fails — the caller treats that as "start without agent".
    fn build_model(conn: &Connection) -> Option<Model> {
        // Read the active model setting.
        let active: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'active_model'",
                [],
                |row| row.get(0),
            )
            .ok()?;

        Self::build_model_from_spec(conn, &active)
    }

    /// Build a `Model` from a `"provider_name:model_name"` spec, using
    /// provider credentials from the DB.
    ///
    /// Returns `None` if the spec is malformed, the provider is not
    /// configured, or the model is not in the built-in catalog.
    fn build_model_from_spec(conn: &Connection, spec: &str) -> Option<Model> {
        use agentik_sdk::provider::registry;

        // Parse "provider_name:model_name"
        let (provider_name, model_name) = spec.split_once(':')?;

        // Look up provider credentials + selected base_url from the DB.
        let (api_key, db_base_url): (String, String) = conn
            .query_row(
                "SELECT api_key, base_url FROM providers WHERE name = ?1",
                [provider_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok()?;

        if api_key.is_empty() {
            return None;
        }

        // Look up model info from the built-in catalog.
        let provider_type = ProviderType::from(provider_name);
        let base_url = if db_base_url.is_empty() {
            registry::default_base_url(&provider_type)
                .unwrap_or("")
                .to_string()
        } else {
            db_base_url
        };
        let auth_method = registry::default_auth_method(&provider_type);
        let preset_models = registry::preset_models(&provider_type)?;
        let mut model_info = preset_models
            .into_iter()
            .find(|m| m.model_name == model_name)?;

        let provider_config = ProviderConfig {
            id: Uuid::nil(),
            name: provider_name.to_string(),
            provider_type,
            base_url,
            api_key,
            auth_method,
        };
        model_info.provider_id = provider_config.id;

        Model::new(model_info, &provider_config).ok()
    }

    /// Load the built-in provider catalogue, augmented with DB credentials,
    /// into [`ModelConfigState`] for the model config widget to render.
    fn load_model_config(
        conn: &Connection,
        state: &mut crate::widgets::model_config_widget::ModelConfigState,
    ) {
        use crate::config_db::ProviderRow;

        // Read configured providers from DB.
        let providers = ProviderRow::all(conn).unwrap_or_default();
        let db_tuples: Vec<(String, String, String)> = providers
            .into_iter()
            .map(|p| (p.provider_type, p.api_key, p.base_url))
            .collect();

        // Build catalogue from SDK registry + DB credentials.
        *state = crate::widgets::model_config_widget::build_catalog(&db_tuples);

        // Load active model name from settings.
        if let Ok(value) = conn.query_row(
            "SELECT value FROM settings WHERE key = 'active_model'",
            [],
            |row| row.get::<_, String>(0),
        ) {
            // value format: "provider_name:model_name"
            if let Some((_, model_name)) = value.split_once(':') {
                state.active_model_name = Some(model_name.to_string());
            }
        }
    }

    fn init_database(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS providers (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                name            TEXT    NOT NULL UNIQUE,
                provider_type   TEXT    NOT NULL,
                base_url        TEXT    NOT NULL,
                api_key         TEXT    NOT NULL,
                auth_method     TEXT    NOT NULL DEFAULT 'Anthropic'
            )",
            (),
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS models (
                id                          INTEGER PRIMARY KEY AUTOINCREMENT,
                model_name                  TEXT    NOT NULL UNIQUE,
                provider_id                 INTEGER NOT NULL,
                context_length              INTEGER NOT NULL DEFAULT 0,
                max_output_tokens           INTEGER NOT NULL DEFAULT 0,
                vision_ability              INTEGER NOT NULL DEFAULT 0,
                supports_function_calling   INTEGER NOT NULL DEFAULT 1,
                supports_streaming          INTEGER NOT NULL DEFAULT 1,
                supports_thinking           INTEGER NOT NULL DEFAULT 0,
                thinking_enabled            INTEGER NOT NULL DEFAULT 0,
                input_token_price           REAL    NOT NULL DEFAULT 0,
                output_token_price          REAL    NOT NULL DEFAULT 0,
                FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
            )",
            (),
        )?;

        // Migration: add `thinking_enabled` to pre-existing databases where
        // the column doesn't exist yet. SQLite doesn't support ADD COLUMN IF
        // NOT EXISTS, so we probe PRAGMA table_info and ignore "duplicate
        // column" errors.
        let has_thinking_enabled: bool = {
            let mut stmt = conn.prepare("PRAGMA table_info(models)")?;
            let cols: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .collect();
            cols.iter().any(|c| c == "thinking_enabled")
        };
        if !has_thinking_enabled {
            let _ = conn.execute(
                "ALTER TABLE models ADD COLUMN thinking_enabled INTEGER NOT NULL DEFAULT 0",
                (),
            );
        }

        conn.execute(
            "CREATE TABLE IF NOT EXISTS settings (
                key             TEXT PRIMARY KEY,
                value           TEXT NOT NULL
            )",
            (),
        )?;

        Ok(())
    }

    pub fn start(&mut self) -> color_eyre::Result<()> {
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(
            stdout(),
            crossterm::terminal::EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
        )?;
        stdout().flush()?;

        set_panic_hook();

        let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

        // The main loop is async (tokio::select! driven); run it on the
        // existing tokio runtime that also hosts the agent task.
        let runtime = self._runtime.take().expect("runtime already consumed");
        let result = runtime.block_on(self.run_loop(&mut terminal));

        // Ensure the agent and engine tasks are torn down even if the main
        // loop exited without a cooperative shutdown (e.g. force-quit).
        for h in &mut self.handles {
            h.shutdown();
        }

        // Restore terminal on exit (whether normal or error).
        let _ = restore_terminal();

        result?;
        Ok(())
    }

    /// Async main loop driven by `tokio::select!` with a fixed-rate render tick.
    ///
    /// **Design**: state mutation and rendering are strictly separated.
    ///
    /// - **Event branches** (terminal, agent, app) only mutate state and set
    ///   the `dirty` flag. They never render.
    /// - **Render tick** fires at a fixed interval (~60 fps) but only redraws
    ///   when `dirty` is true *or* the agent is active (animation frames).
    ///   When idle and no events arrive, the loop parks on `select!` and
    ///   consumes zero CPU.
    ///
    /// Ratatui's internal buffer-diff ensures only changed cells are written
    /// to the terminal. The first interval tick completes immediately, so the
    /// initial frame is drawn right away (the constructor sets `dirty = true`).
    async fn run_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> std::io::Result<()> {
        use crossterm::event::EventStream;
        use tokio::time::{self, Duration, MissedTickBehavior};
        use tokio_stream::StreamExt;

        let mut event_stream = EventStream::new();

        // Fixed-rate render clock. The first tick completes immediately
        // (guaranteeing the initial frame), then fires every ~16 ms.
        // `Skip` discards ticks that arrive while we were busy handling
        // events, preventing burst-renders after a long event handler.
        let mut render_tick = time::interval(Duration::from_millis(16));
        render_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            if self.should_quit {
                break Ok(());
            }

            tokio::select! {
                biased;

                // ── Terminal input (keys, mouse, paste, resize) ──
                maybe_event = event_stream.next() => {
                    match maybe_event {
                        Some(Ok(event)) => {
                            let delta = self.handle_event(&event);
                            if delta != 0 {
                                self.apply_scroll_delta(delta);
                            }
                            // Most terminal events mutate state or at least require
                            // a redraw (e.g. resize). Setting dirty unconditionally
                            // is simpler and the worst case is one extra frame.
                            self.dirty = true;
                        }
                        Some(Err(e)) => {
                            // crossterm read error — log so it's not silently
                            // swallowed (the old code dropped `Err` entirely).
                            tracing::warn!(error = %e, "crossterm event read error");
                        }
                        None => {
                            // Stream exhausted (background thread died).
                            // Recreate immediately.
                            tracing::warn!("crossterm EventStream ended; recreating");
                            event_stream = EventStream::new();
                        }
                    }
                }

                // ── Agent streaming events (poll active handle) ──
                maybe_agent = async {
                    if let Some(handle) = self.handles.get_mut(self.state.active_agent_idx) {
                        handle.recv_event().await
                    } else {
                        // No active session — park forever.
                        std::future::pending::<Option<AgentEvent>>().await
                    }
                } => {
                    if let Some(event) = maybe_agent {
                        let _idx = self.state.active_agent_idx;
                        if matches!(
                            event,
                            AgentEvent::SessionActivated { .. }
                                | AgentEvent::SessionPaused { .. }
                                | AgentEvent::SessionClosed { .. }
                                | AgentEvent::SessionList { .. }
                        ) {
                            state::apply_session_event(&mut self.state, event);
                        } else {
                            state::apply_event(
                                self.state.active_tab_state_mut(),
                                event,
                            );
                        }
                        self.dirty = true;
                    } else {
                        // Active agent channel closed — don't quit, just mark.
                        tracing::warn!("active agent event channel closed");
                    }
                }

                // ── App internal events ──
                maybe_app = self.app_event_rx.recv() => {
                    match maybe_app {
                        Some(event) => self.handle_app_event(event),
                        None => self.should_quit = true,
                    }
                    self.dirty = true;
                }

                // ── Fixed-rate render tick ──
                _ = render_tick.tick() => {
                    let active_status = self.state.active_status();
                    if matches!(active_status, AgentStatus::Idle) {
                        self.clear_cancel_pending();
                    } else {
                        let ts = self.state.active_tab_state_mut();
                        ts.frame = ts.frame.wrapping_add(1);
                    }

                    let agent_active = !matches!(active_status, AgentStatus::Idle);
                    if self.dirty || agent_active {
                        terminal.draw(|f| self.render(f))?;
                        self.dirty = false;
                    }
                }
            }
        }
    }

    /// Apply an internal [`AppEvent`] to state.
    fn handle_app_event(&mut self, event: crate::app_event::AppEvent) {
        match event {
            crate::app_event::AppEvent::Agent(e) => {
                let ts = self.state.active_tab_state_mut();
                state::apply_event(ts, *e);
            }
            crate::app_event::AppEvent::Quit => {
                self.should_quit = true;
            }
            crate::app_event::AppEvent::ConfigReload => {
                Self::load_model_config(&self.conn, &mut self.state.model_config_state);
            }
            crate::app_event::AppEvent::AgentRecordsLoaded(records) => {
                self.state.agent_picker.set_records(&records);
                self.state.agent_picker.open();
                tracing::info!(count = records.len(), "agent records loaded for picker");
            }
            crate::app_event::AppEvent::AgentDeleted(agent_id) => {
                self.state.agent_picker.remove_by_id(agent_id);
                tracing::info!(%agent_id, "agent removed from picker after deletion");
            }
            crate::app_event::AppEvent::HistoryLoaded { agent_id, messages } => {
                self.replay_history(agent_id, &messages);
            }
            crate::app_event::AppEvent::AgentSpawned {
                profile_name,
                result,
            } => match result {
                Ok(handle) => {
                    let agent_id = handle.agent_id;
                    let name = handle.name.clone();
                    self.handles.push(handle);
                    // Request the session list so we can sync the session tab bar.
                    self.handles.last().map(|h| h.list_sessions());
                    self.state.sessions.push(AgentSession {
                        name,
                        agent_id,
                        sub_sessions: Vec::new(),
                        active_sub_session_idx: 0,
                    });
                    self.state.active_agent_idx = self.state.sessions.len() - 1;

                    // ── Load conversation history from storage ──
                    // On resume, the agent's Memory is restored from the WAL
                    // but the TUI's `tab_state.messages` is empty. Load the
                    // rendered history asynchronously and replay it as
                    // ChatLines.
                    if let Some(host) = &self.host {
                        let storage = host.storage().clone();
                        let tx = self.app_event_tx.clone();
                        let aid = agent_id;
                        self.runtime_handle.spawn(async move {
                            match agentik_core::storage::restore_memory(
                                storage.as_ref(),
                                aid,
                            )
                            .await
                            {
                                Ok(memory) => {
                                    let messages = memory
                                        .render_context()
                                        .unwrap_or_default();
                                    if !messages.is_empty() {
                                        tx.send(crate::app_event::AppEvent::HistoryLoaded {
                                            agent_id: aid,
                                            messages,
                                        });
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "failed to load history for agent {aid}");
                                }
                            }
                        });
                    }

                    tracing::info!(profile = %profile_name, "agent spawned successfully");
                }
                Err(e) => {
                    tracing::error!(profile = %profile_name, error = %e, "failed to spawn agent");
                }
            },
        }
    }

    /// Replay conversation history loaded from storage into the TUI's
    /// `tab_state.messages` as `ChatLine` entries.
    ///
    /// This is called when a resumed agent's history arrives via
    /// `AppEvent::HistoryLoaded`. Each `Message` in the rendered context is
    /// converted to one or more `ChatLine`s (user text → `ChatLine::User`,
    /// assistant text → `ChatLine::Assistant`, tool_use → `ChatLine::ToolCall`,
    /// tool_result → `ChatLine::ToolResult`, etc.).
    fn replay_history(&mut self, agent_id: uuid::Uuid, messages: &[Message]) {
        // Find the agent session index.
        let session_idx = self.state.sessions.iter().position(|s| s.agent_id == agent_id);
        let Some(session_idx) = session_idx else {
            tracing::warn!(%agent_id, "history loaded for unknown agent");
            return;
        };

        // Build ChatLines from the message list.
        let lines = messages_to_chatlines(messages);
        if lines.is_empty() {
            return;
        }

        // If the agent already has sub_sessions, write into the active one.
        // Otherwise, create a default sub_session to hold the history.
        let session = &mut self.state.sessions[session_idx];
        if session.sub_sessions.is_empty() {
            session.sub_sessions.push(state::SubSession::new(
                uuid::Uuid::new_v4(),
                Some("Restored".into()),
            ));
        }
        let sub = &mut session.sub_sessions[session.active_sub_session_idx];
        if !sub.tab_state.messages.is_empty() {
            // Already has messages — don't clobber (e.g. user sent something
            // while history was loading).
            tracing::debug!(%agent_id, "tab_state already populated, skipping history replay");
            return;
        }
        sub.tab_state.messages = lines;
        sub.tab_state.content_line_count = sub.tab_state.messages.len();
        sub.tab_state.scroll_to_bottom();
        self.dirty = true;
        tracing::info!(%agent_id, count = sub.tab_state.messages.len(), "history replayed into tab_state");
    }
    fn handle_event(&mut self, event: &Event) -> i32 {
        match event {
            Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
                self.handle_key(key);
                0
            }
            Event::Resize(_, _) | Event::FocusGained | Event::FocusLost => 0,
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::Paste(s) => {
                // Insert paste into the agent chat input area when in input mode and agent is idle.
                if true {
                    let ts = self.state.active_tab_state_mut();
                    if ts.input_mode == InputMode::Input && ts.status == state::AgentStatus::Idle {
                        ts.input.insert_str(s);
                    }
                }
                // Insert paste into the focused textarea when in Config mode.
                if false {
                    use crate::widgets::model_config_widget::{ConfigField, ProviderPanelState};
                    if let ProviderPanelState::Config {
                        api_key,
                        base_url,
                        focused_field,
                        ..
                    } = &mut self.state.model_config_state.provider_panel_state
                    {
                        match *focused_field {
                            ConfigField::ApiKey => api_key.insert_str(s),
                            ConfigField::BaseUrl => base_url.insert_str(s),
                        }
                    }
                }
                0
            }
            _ => 0,
        }
    }

    /// Reset the cancel-request timestamp (called when the agent returns to Idle).
    fn clear_cancel_pending(&mut self) {
        self.cancel_requested_at = None;
    }

    /// Handle mouse events: scroll wheel scrolls the chat in Agent tab.
    /// Returns the scroll delta to be batched with other scroll events.
    fn handle_mouse(&mut self, mouse: &MouseEvent) -> i32 {
        if !true {
            return 0;
        }

        let lines_per_tick: i32 = 3;

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                let ts = self.state.active_tab_state_mut();
                ts.auto_scroll = false;
                lines_per_tick
            }
            MouseEventKind::ScrollUp => {
                let ts = self.state.active_tab_state_mut();
                ts.auto_scroll = false;
                -lines_per_tick
            }
            _ => 0,
        }
    }

    /// Apply a batched scroll delta to the agent tab.
    fn apply_scroll_delta(&mut self, delta: i32) {
        if !true {
            return;
        }
        let ts = self.state.active_tab_state_mut();
        if delta > 0 {
            ts.scroll_offset = ts.scroll_offset.saturating_add(delta as usize);
        } else {
            ts.scroll_offset = ts.scroll_offset.saturating_sub((-delta) as usize);
        }
    }

    fn handle_key(&mut self, key: &KeyEvent) {
        // Ctrl+P: toggle the command palette. Handled globally so it works
        // from any tab / input mode. When opening, takes precedence over all
        // other handlers; when closing, behaves identically to Esc.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
            self.state.command_palette.toggle();
            return;
        }

        // While the command palette is open it captures all remaining keys
        // (navigation, filtering, execution, dismissal) — tab handlers never
        // see them.
        if self.state.command_palette.visible {
            self.handle_command_palette_key(key);
            return;
        }

        // Profile picker popup captures keys when visible.
        if self.state.profile_picker.visible {
            self.handle_profile_picker_key(key);
            return;
        }

        // Agent resume picker popup captures keys when visible.
        if self.state.agent_picker.visible {
            self.handle_agent_picker_key(key);
            return;
        }

        // Name input popup captures keys when visible.
        if self.state.name_input.visible {
            self.handle_name_input_key(key);
            return;
        }

        // Session picker popup captures keys when visible.
        if self.state.session_picker.visible {
            self.handle_session_picker_key(key);
            return;
        }

        // Model config popup captures keys when visible.
        if self.state.model_config_visible {
            self.handle_model_config_key(key);
            return;
        }

        // Ctrl+C: cancel running agent first, then quit on second press.
        // If the agent is blocked and doesn't transition to Idle after the
        // first cancel, a second Ctrl+C within FORCE_QUIT_WINDOW force-quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.should_quit {
                // Already quitting — no-op.
                return;
            }
            if matches!(self.state.active_tab_state_mut().status, AgentStatus::Idle) {
                self.should_quit = true;
                return;
            }
            // Agent is running — check for force-quit (double Ctrl+C).
            if let Some(ts) = self.cancel_requested_at {
                if ts.elapsed() < FORCE_QUIT_WINDOW {
                    tracing::info!("force-quit: second Ctrl+C within {:?}", FORCE_QUIT_WINDOW);
                    if let Some(h) = self.handles.get_mut(self.state.active_agent_idx) {
                        h.shutdown();
                    }
                    self.should_quit = true;
                    return;
                }
            }
            // First Ctrl+C: cooperative cancel.
            if let Some(h) = self.handles.get_mut(self.state.active_agent_idx) {
                h.cancel();
            }
            self.cancel_requested_at = Some(Instant::now());
            return;
        }

        // Ctrl+G: toggle the auto-scroll-to-bottom lock on the Agent tab.
        // When locked (following the tail), the first press releases the lock
        // so the user can scroll freely; a second press re-pins to the bottom.
        // Scroll-producing keys/mouse also release the lock (see handle_mouse
        // and handle_browse_key); Ctrl+G is the dedicated toggle.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g') && true {
            let ts = self.state.active_tab_state_mut();
            if ts.auto_scroll {
                ts.auto_scroll = false;
            } else {
                ts.scroll_to_bottom();
            }
            return;
        }

        // All remaining keys go to the agent workspace.
        self.handle_agent_key(key);
    }

    fn handle_agent_key(&mut self, key: &KeyEvent) {
        // Alt+1..9: switch to leaf by index.
        if key.modifiers.contains(KeyModifiers::ALT) {
            if let KeyCode::Char(c) = key.code {
                if let Some(digit) = c.to_digit(10) {
                    let idx = (digit as usize).saturating_sub(1);
                    if idx < self.state.sessions.len() {
                        self.state.active_agent_idx = idx;
                    }
                    return;
                }
            }
        }

        let input_mode = self.state.active_tab_state().input_mode;
        match input_mode {
            InputMode::Browse => self.handle_browse_key(key),
            InputMode::Input => self.handle_input_key(key),
        }
    }

    /// Spawn a new agent from a profile asynchronously.
    ///
    /// Because the event loop is already inside `runtime.block_on(...)`, we
    /// can't call `block_on` again. Instead, we spawn the creation as a
    /// background task and send the result back via the app event channel.
    /// The `AgentSpawned` event is handled in `handle_app_event`.
    fn spawn_agent_from_profile(&mut self, profile: &AgentProfile, agent_name: &str) {
        tracing::info!(
            profile = %profile.name,
            agent = %agent_name,
            "spawn_agent_from_profile called"
        );

        let Some(host) = self.host.clone() else {
            tracing::warn!("no runtime host available");
            return;
        };
        if self.state.active_model.load_full().is_none() {
            tracing::warn!("no model configured — configure one in Config tab first");
            return;
        }

        let global_model = self.state.active_model.clone();
        let model_override = profile
            .preferred_model
            .as_deref()
            .and_then(|spec| Self::build_model_from_spec(&self.conn, spec));
        tracing::debug!(
            has_override = model_override.is_some(),
            "model resolution complete"
        );

        let profile_clone = profile.clone();
        let agent_name_owned = agent_name.to_string();
        let profile_name_owned = profile.name.clone();
        let tx = self.app_event_tx.clone();

        let join_handle = self.runtime_handle.spawn(async move {
            tracing::debug!(
                profile = %profile_clone.name,
                agent = %agent_name_owned,
                "async spawn task started"
            );
            host.spawn_agent(
                &agent_name_owned,
                &profile_clone,
                global_model,
                model_override,
            )
            .await
            .map_err(|e| e.to_string())
        });

        self.runtime_handle.spawn(async move {
            let result = join_handle.await;
            let event = match result {
                Ok(Ok(handle)) => {
                    tracing::info!(profile = %profile_name_owned, "agent spawned successfully");
                    crate::app_event::AppEvent::AgentSpawned {
                        profile_name: profile_name_owned,
                        result: Ok(handle),
                    }
                }
                Ok(Err(e)) => {
                    tracing::error!(profile = %profile_name_owned, error = %e, "agent spawn failed");
                    crate::app_event::AppEvent::AgentSpawned {
                        profile_name: profile_name_owned,
                        result: Err(e),
                    }
                }
                Err(join_err) => {
                    let msg = if join_err.is_panic() {
                        let panic_msg = join_err.into_panic();
                        let msg = panic_msg
                            .downcast_ref::<&str>()
                            .copied()
                            .or_else(|| panic_msg.downcast_ref::<String>().map(|s| s.as_str()))
                            .unwrap_or("(non-string panic)");
                        format!("agent spawn panicked: {msg}")
                    } else {
                        "agent spawn cancelled".to_string()
                    };
                    tracing::error!(profile = %profile_name_owned, "{msg}");
                    crate::app_event::AppEvent::AgentSpawned {
                        profile_name: profile_name_owned,
                        result: Err(msg),
                    }
                }
            };
            let _ = tx.send(event);
        });

        tracing::info!(profile = %profile.name, "spawning agent...");
    }

    /// Key handling in browse mode: Up/Down scroll line-by-line,
    /// PageDown/PageUp half-page, Home/End jump to top/bottom,
    /// Enter enters the composer (input mode).
    /// Query the agents table and open the resume picker.
    fn open_agent_picker(&mut self) {
        let Some(host) = self.host.clone() else {
            tracing::warn!("no host available for agent listing");
            return;
        };
        let tx = self.app_event_tx.clone();
        self.runtime_handle.spawn(async move {
            let storage = host.storage();
            match storage.list_agents().await {
                Ok(records) => {
                    let _ = tx.send(crate::app_event::AppEvent::AgentRecordsLoaded(records));
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to list agents");
                }
            }
        });
    }

    /// Key handling while the agent resume picker popup is open.
    fn handle_agent_picker_key(&mut self, key: &KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // If delete confirmation is active, route keys differently.
        if self.state.agent_picker.delete_confirm_id.is_some() {
            match key.code {
                KeyCode::Esc => self.state.agent_picker.cancel_delete(),
                KeyCode::Backspace => {
                    self.state.agent_picker.delete_confirm_input.pop();
                }
                KeyCode::Char(c) if !ctrl => {
                    self.state.agent_picker.delete_confirm_input.push(c);
                }
                KeyCode::Enter => {
                    if let Some(agent_id) = self.state.agent_picker.check_delete_confirm() {
                        self.delete_agent_record(agent_id);
                    } else {
                        // Wrong input — cancel confirmation.
                        self.state.agent_picker.cancel_delete();
                    }
                }
                _ => {}
            }
            return;
        }

        // Normal mode.
        match key.code {
            KeyCode::Esc => {
                self.state.agent_picker.close();
            }
            KeyCode::Up => self.state.agent_picker.move_up(),
            KeyCode::Down => self.state.agent_picker.move_down(),
            KeyCode::Backspace => self.state.agent_picker.pop_char(),
            KeyCode::Char('d') if !ctrl => {
                self.state.agent_picker.start_delete_confirm();
            }
            KeyCode::Char(c) if !ctrl => self.state.agent_picker.push_char(c),
            KeyCode::Enter => {
                if let Some(item) = self.state.agent_picker.selected_item() {
                    let config_json = item.config_json.clone();
                    let agent_name = item.name.clone();
                    self.state.agent_picker.close();
                    // Try to reconstruct the profile from the stored config_json.
                    // Fall back to looking up by name in the current profiles.
                    let profile = serde_json::from_value::<AgentProfile>(config_json)
                        .ok()
                        .or_else(|| {
                            self.state
                                .profiles
                                .iter()
                                .find(|p| p.name == agent_name)
                                .cloned()
                        });
                    match profile {
                        Some(p) => self.spawn_agent_from_profile(&p, &agent_name),
                        None => tracing::warn!(
                            agent = %agent_name,
                            "could not reconstruct profile for agent record",
                        ),
                    }
                }
            }
            _ => {}
        }
    }

    /// Delete an agent record from storage and update the picker list.
    fn delete_agent_record(&mut self, agent_id: uuid::Uuid) {
        tracing::info!(%agent_id, "deleting agent record");
        let Some(host) = self.host.clone() else {
            tracing::warn!("no host available for deletion");
            return;
        };
        let tx = self.app_event_tx.clone();
        self.runtime_handle.spawn(async move {
            let storage = host.storage();
            match storage.delete_agent(agent_id).await {
                Ok(()) => {
                    tracing::info!(%agent_id, "agent deleted from storage");
                    let _ = tx.send(crate::app_event::AppEvent::AgentDeleted(agent_id));
                }
                Err(e) => {
                    tracing::error!(%agent_id, error = %e, "failed to delete agent");
                }
            }
        });
    }

    /// Key handling while the name input popup is open.
    fn handle_name_input_key(&mut self, key: &KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.state.name_input.close();
                self.state.pending_profile = None;
            }
            KeyCode::Backspace => self.state.name_input.pop_char(),
            KeyCode::Char(c) if !ctrl => self.state.name_input.push_char(c),
            KeyCode::Enter => {
                let name = self.state.name_input.value().to_string();
                let profile = self.state.pending_profile.take();
                self.state.name_input.close();
                if let Some(p) = profile {
                    if name.is_empty() {
                        tracing::warn!("agent name cannot be empty");
                        return;
                    }
                    self.spawn_agent_from_profile(&p, &name);
                }
            }
            _ => {}
        }
    }

    /// Key handling while the session picker popup is open.
    fn handle_session_picker_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.state.session_picker.close();
            }
            KeyCode::Up => self.state.session_picker.move_up(),
            KeyCode::Down => self.state.session_picker.move_down(),
            KeyCode::Char('n') => {
                // Create a new (empty) session in the current agent.
                if let Some(handle) =
                    self.handles.get(self.state.active_agent_idx)
                {
                    handle.create_session(Some("New session".into()), None);
                }
                self.state.session_picker.close();
            }
            KeyCode::Char('d') => {
                // Close the currently selected session.
                if let Some(id) = self.state.session_picker.selected_id() {
                    if let Some(handle) =
                        self.handles.get(self.state.active_agent_idx)
                    {
                        handle.close_session(id);
                    }
                }
                self.state.session_picker.close();
            }
            KeyCode::Enter => {
                // Switch to the selected session.
                if let Some(id) = self.state.session_picker.selected_id() {
                    if let Some(handle) =
                        self.handles.get(self.state.active_agent_idx)
                    {
                        handle.switch_session(id);
                    }
                }
                self.state.session_picker.close();
            }
            _ => {}
        }
    }

    fn handle_browse_key(&mut self, key: &KeyEvent) {
        // Shift+H/L: switch to previous/next agent leaf tab.
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Char('H') | KeyCode::Char('h') => {
                    if self.state.active_agent_idx > 0 {
                        self.state.active_agent_idx -= 1;
                    }
                    return;
                }
                KeyCode::Char('L') | KeyCode::Char('l') => {
                    let max = self.state.sessions.len().saturating_sub(1);
                    if self.state.active_agent_idx < max {
                        self.state.active_agent_idx += 1;
                    }
                    return;
                }
                _ => {}
            }
        }

        let ts = self.state.active_tab_state_mut();

        match key.code {
            // Down / PageDown: scroll down (show later content)
            KeyCode::Down => {
                ts.scroll_offset = ts.scroll_offset.saturating_add(1);
                ts.auto_scroll = false;
            }
            // Up: scroll up (show earlier content)
            KeyCode::Up => {
                ts.scroll_offset = ts.scroll_offset.saturating_sub(1);
                ts.auto_scroll = false;
            }
            // End: jump to bottom, re-enable auto-scroll
            KeyCode::End => {
                ts.auto_scroll = true;
            }
            // Home: jump to top
            KeyCode::Home => {
                ts.scroll_offset = 0;
                ts.auto_scroll = false;
            }
            // PageDown: half-page down
            KeyCode::PageDown => {
                ts.scroll_offset = ts.scroll_offset.saturating_add(HALF_PAGE);
                ts.auto_scroll = false;
            }
            // PageUp: half-page up
            KeyCode::PageUp => {
                ts.scroll_offset = ts.scroll_offset.saturating_sub(HALF_PAGE);
                ts.auto_scroll = false;
            }
            // Enter: enter the composer in input mode.
            KeyCode::Enter => {
                ts.input_mode = InputMode::Input;
            }
            _ => {}
        }
    }

    /// Key handling in input mode: typing goes to input, Enter sends, Esc exits.
    fn handle_input_key(&mut self, key: &KeyEvent) {
        use crate::widgets::input_area::{history_clear_recall, history_down, history_up};

        // While an incremental Ctrl+R search is active, every keystroke
        // drives the search instead of editing the buffer.
        if self.state.active_tab_state_mut().in_history_search {
            self.handle_history_search_key(key);
            return;
        }

        let active_idx = self.state.active_agent_idx;
        let ts = self.state.active_tab_state_mut();
        let idle = ts.status == state::AgentStatus::Idle;

        // Ctrl+R: enter incremental history search (codex-style).
        if idle
            && !ts.input_history.is_empty()
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('r')
        {
            ts.in_history_search = true;
            ts.history_search_query.clear();
            ts.history_search_draft = Some(ts.input.value());
            ts.history_search_matches = compute_search_matches(&ts.input_history, "");
            ts.history_search_selected = 0;
            load_selected_history_match(ts);
            return;
        }

        // Pre-extract data needed after the `ts` borrow ends.
        let mut send_text: Option<String> = None;

        match key.code {
            // Esc: leave input mode, return to browse. Any in-progress
            // Up/Down history recall is collapsed first.
            KeyCode::Esc => {
                history_clear_recall(&mut ts.input_draft, &mut ts.input_recall);
                ts.input_mode = InputMode::Browse;
            }
            // Enter: Shift/Alt+Enter inserts a newline (multiline compose);
            // a plain Enter sends the message and returns to browse mode.
            KeyCode::Enter => {
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT)
                {
                    // Alt is a fallback for terminals that don't report Shift on Enter.
                    if idle {
                        ts.input.insert_newline();
                    }
                    return;
                }
                if ts.can_send() {
                    let text = ts.take_input();
                    // Push to in-memory history before clearing the
                    // recall state — `take_input()` already cleared the
                    // textbox, but recall metadata is independent.
                    crate::widgets::input_area::history_push(
                        &mut ts.input_history,
                        text.clone(),
                        ts.input_history_capacity,
                    );
                    history_clear_recall(&mut ts.input_draft, &mut ts.input_recall);
                    ts.push_user_message(text.clone());
                    send_text = Some(text);
                    ts.scroll_to_bottom();
                }
                ts.input_mode = InputMode::Browse;
            }
            // Up/Down: recall history (Up) / advance towards draft (Down)
            KeyCode::Up => {
                if idle {
                    let _ = history_up(
                        &mut ts.input,
                        &ts.input_history,
                        &mut ts.input_draft,
                        &mut ts.input_recall,
                    );
                }
            }
            KeyCode::Down => {
                if idle {
                    let _ = history_down(
                        &mut ts.input,
                        &ts.input_history,
                        &mut ts.input_draft,
                        &mut ts.input_recall,
                    );
                }
            }
            // Any other key: collapse in-progress recall so subsequent
            // edits are treated as user-driven (not as a recalled entry
            // we'd accidentally re-push when sent).
            _ => {
                if idle {
                    if ts.input_recall.is_some() {
                        history_clear_recall(&mut ts.input_draft, &mut ts.input_recall);
                    }
                    ts.input.handle_key(*key);
                }
            }
        }

        // Dispatch send_message outside the `ts` borrow.
        if let Some(text) = send_text {
            if let Some(h) = self.handles.get(active_idx) {
                h.send_message(text);
            }
        }
    }

    /// Key handling while a Ctrl+R incremental history search is active.
    fn handle_history_search_key(&mut self, key: &KeyEvent) {
        let ts = self.state.active_tab_state_mut();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // Esc: cancel the search, restore the original draft buffer.
            KeyCode::Esc => {
                let draft = ts.history_search_draft.take().unwrap_or_default();
                ts.input.clear();
                ts.input.insert_str(&draft);
                end_history_search(ts);
            }
            // Enter: accept the currently-previewed match into the buffer
            // and resume normal input editing.
            KeyCode::Enter => {
                end_history_search(ts);
            }
            // Up: move to the next-older match.
            KeyCode::Up => {
                if !ts.history_search_matches.is_empty() {
                    ts.history_search_selected =
                        (ts.history_search_selected + 1).min(ts.history_search_matches.len() - 1);
                    load_selected_history_match(ts);
                }
            }
            // Down: move toward the newest match.
            KeyCode::Down => {
                if !ts.history_search_matches.is_empty() {
                    ts.history_search_selected = ts.history_search_selected.saturating_sub(1);
                    load_selected_history_match(ts);
                }
            }
            // Backspace: drop the last query character and refilter.
            KeyCode::Backspace => {
                ts.history_search_query.pop();
                recompute_history_search(ts);
            }
            // Type into the query (plain chars only).
            KeyCode::Char(c) if !ctrl => {
                ts.history_search_query.push(c);
                recompute_history_search(ts);
            }
            _ => {}
        }
    }

    // ── Command palette ─────────────────────────────────

    /// Key handling while the command palette is open. Returns control to
    /// `handle_key`'s caller; the caller is responsible for closing on Esc
    /// (via [`CommandPaletteState::close`]).
    fn handle_command_palette_key(&mut self, key: &KeyEvent) {
        use crate::widgets::command_palette::CommandAction;

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // Esc or Ctrl+P (handled above) — close without running anything.
            KeyCode::Esc => {
                self.state.command_palette.close();
            }
            // Enter: run the selected action, then dismiss.
            KeyCode::Enter => {
                let action = self.state.command_palette.selected_action();
                self.state.command_palette.close();
                if let Some(action) = action {
                    self.run_command_action(action);
                }
            }
            KeyCode::Up => self.state.command_palette.move_up(),
            KeyCode::Down => self.state.command_palette.move_down(),
            KeyCode::Backspace => self.state.command_palette.pop_char(),
            KeyCode::Char(c) if !ctrl => self.state.command_palette.push_char(c),
            _ => {}
        }
    }

    /// Key handling while the profile picker popup is open.
    fn handle_profile_picker_key(&mut self, key: &KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.state.profile_picker.close();
            }
            KeyCode::Up => self.state.profile_picker.move_up(),
            KeyCode::Down => self.state.profile_picker.move_down(),
            KeyCode::Backspace => self.state.profile_picker.pop_char(),
            KeyCode::Char(c) if !ctrl => self.state.profile_picker.push_char(c),
            KeyCode::Enter => {
                if let Some(item) = self.state.profile_picker.selected_item() {
                    let profile = item.profile;
                    self.state.profile_picker.close();
                    // Stash the selected profile; the name input popup
                    // will open next and prompt the user for an agent
                    // name (pre-filled with the profile name).
                    self.state.pending_profile = Some(profile.clone());
                    self.state
                        .name_input
                        .open(format!(" New Agent ({}) ", profile.name), profile.name);
                }
            }
            _ => {}
        }
    }

    /// Execute a command selected from the palette. Each action mirrors an
    /// existing key binding — the palette is just a discoverable shortcut to
    /// the same operations.
    fn run_command_action(&mut self, action: crate::widgets::command_palette::CommandAction) {
        use crate::widgets::command_palette::CommandAction;

        match action {
            CommandAction::Quit => {
                self.should_quit = true;
            }
            CommandAction::CancelAgent => {
                if !matches!(self.state.active_status(), AgentStatus::Idle) {
                    if let Some(h) = self.handles.get_mut(self.state.active_agent_idx) {
                        h.cancel();
                    }
                    self.cancel_requested_at = Some(Instant::now());
                }
            }
            CommandAction::EnterInput => {
                if true {
                    self.state.active_tab_state_mut().input_mode = InputMode::Input;
                }
            }
            CommandAction::ToggleAutoScroll => {
                if true {
                    let ts = self.state.active_tab_state_mut();
                    if ts.auto_scroll {
                        ts.auto_scroll = false;
                    } else {
                        ts.scroll_to_bottom();
                    }
                }
            }
            CommandAction::ScrollToBottom => {
                if true {
                    self.state.active_tab_state_mut().scroll_to_bottom();
                }
            }
            CommandAction::ScrollToTop => {
                if true {
                    let ts = self.state.active_tab_state_mut();
                    ts.scroll_offset = 0;
                    ts.auto_scroll = false;
                }
            }
            CommandAction::HistorySearch => {
                // Trigger the same flow as Ctrl+R: only meaningful in the
                // Agent tab when there is history and the agent is idle.
                let is_agent_tab = true;
                let ts = self.state.active_tab_state_mut();
                let can = is_agent_tab
                    && ts.status == state::AgentStatus::Idle
                    && !ts.input_history.is_empty();
                if can {
                    ts.input_mode = InputMode::Input;
                    ts.in_history_search = true;
                    ts.history_search_query.clear();
                    ts.history_search_draft = Some(ts.input.value());
                    ts.history_search_matches = compute_search_matches(&ts.input_history, "");
                    ts.history_search_selected = 0;
                    load_selected_history_match(ts);
                }
            }
            CommandAction::ClearTranscript => {
                let ts = self.state.active_tab_state_mut();
                ts.messages.clear();
                ts.msg_versions.clear();
                ts.cached_msg_lines.clear();
                ts.cached_msg_versions.clear();
                ts.scroll_offset = 0;
                ts.scroll_to_bottom();
            }
            CommandAction::ReloadConfig => {
                Self::load_model_config(&self.conn, &mut self.state.model_config_state);
            }
            CommandAction::SpawnAgent(profile_name) => {
                if let Some(profile) = self
                    .state
                    .profiles
                    .iter()
                    .find(|p| p.name == profile_name)
                    .cloned()
                {
                    // Stash the profile; name input prompts for the agent name.
                    self.state.pending_profile = Some(profile.clone());
                    self.state
                        .name_input
                        .open(format!(" New Agent ({}) ", profile.name), profile.name);
                }
            }
            CommandAction::NewAgent => {
                self.state.profile_picker.open();
            }
            CommandAction::ResumeAgent => {
                self.open_agent_picker();
            }
            CommandAction::ModelConfig => {
                self.state.model_config_visible = true;
            }
            CommandAction::OpenSessions => {
                self.open_session_picker();
            }
        }
    }

    /// Open the session picker for the currently active agent.
    fn open_session_picker(&mut self) {
        let Some(agent_session) = self.state.sessions.get(self.state.active_agent_idx) else {
            tracing::warn!("no active agent to open session picker");
            return;
        };
        let agent_id = agent_session.agent_id;
        let agent_name = agent_session.name.clone();
        let Some(handle) = self.handles.get(self.state.active_agent_idx) else {
            tracing::warn!("no handle for active agent");
            return;
        };
        // Seed picker with whatever sub_sessions are already known; the
        // SessionList event will refresh the list shortly.
        let initial_items: Vec<crate::widgets::session_picker::PickerSession> =
            agent_session
                .sub_sessions
                .iter()
                .map(|s| crate::widgets::session_picker::PickerSession {
                    id: s.id,
                    title: s.title.clone(),
                })
                .collect();
        let active_id = agent_session
            .sub_sessions
            .get(agent_session.active_sub_session_idx)
            .map(|s| s.id);
        self.state
            .session_picker
            .open(agent_id, agent_name.clone());
        self.state
            .session_picker
            .set_sessions(initial_items, active_id);
        // Ask the agent for a fresh list (will arrive via SessionList event).
        handle.list_sessions();
        tracing::info!(agent = %agent_name, "session picker opened");
    }

    fn render(&mut self, frame: &mut Frame) {
        // ── Workspace (full screen) ──
        // Read the model name from the active agent's own model slot (not
        // the global active_model), so per-agent model switches are reflected.
        let model_info = self
            .handles
            .get(self.state.active_agent_idx)
            .and_then(|h| h.model_handle().load_full())
            .map(|m| (m.model_info.model_name.clone(), m.model_info.context_length))
            .or_else(|| {
                // Fallback to global model when no agent is active.
                self.state
                    .active_model
                    .load_full()
                    .map(|m| (m.model_info.model_name.clone(), m.model_info.context_length))
            });
        let (model_name, context_window) = match model_info {
            Some((name, ctx)) => (Some(name), Some(ctx)),
            None => (None, None),
        };

        // Collect tab data before mutably borrowing tab state.
        let workspace_tabs: Vec<crate::widgets::agent_workspace::LeafTab> = self
            .state
            .sessions
            .iter()
            .map(|s| crate::widgets::agent_workspace::LeafTab {
                name: s.name.clone(),
                status: s
                    .sub_sessions
                    .get(s.active_sub_session_idx)
                    .map(|sub| sub.tab_state.status.clone())
                    .unwrap_or_default(),
            })
            .collect();
        let active_idx = self.state.active_agent_idx;

        let workspace = AgentWorkspace {
            active_model: model_name.as_deref(),
            context_window,
        };
        workspace.render(
            frame.area(),
            frame.buffer_mut(),
            &workspace_tabs,
            active_idx,
            self.state.active_tab_state_mut(),
        );

        // Position cursor only when there's an active agent leaf.
        if !self.state.sessions.is_empty() {
            if let Some((cx, cy)) = self.state.active_tab_state_mut().input.last_cursor_pos() {
                frame.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
            }
        }

        // ── Command palette overlay ──
        if self.state.command_palette.visible {
            crate::widgets::command_palette::render_command_palette(
                frame.area(),
                frame.buffer_mut(),
                &mut self.state.command_palette,
            );
        }

        // ── Profile picker popup ──
        if self.state.profile_picker.visible {
            use ratatui::widgets::StatefulWidget as _;
            crate::widgets::profile_picker::ProfilePicker::new()
                .popup_width((frame.area().width * 8 / 10).max(70))
                .list_width(28)
                .render(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.profile_picker,
                );
        }

        // ── Agent resume picker popup ──
        if self.state.agent_picker.visible {
            use ratatui::widgets::StatefulWidget as _;
            crate::widgets::agent_picker::AgentPicker::new()
                .popup_width((frame.area().width * 8 / 10).max(70))
                .list_width(28)
                .render(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.agent_picker,
                );
        }

        // ── Session picker popup ──
        if self.state.session_picker.visible {
            use ratatui::widgets::StatefulWidget as _;
            crate::widgets::session_picker::SessionPicker::new()
                .render(
                    frame.area(),
                    frame.buffer_mut(),
                    &mut self.state.session_picker,
                );
        }

        // ── Name input popup ──
        crate::widgets::name_input::render_name_input(
            frame.area(),
            frame.buffer_mut(),
            &self.state.name_input,
        );

        // ── Model config popup ──
        if self.state.model_config_visible {
            use ratatui::widgets::StatefulWidgetRef as _;
            let popup = crate::widgets::popup::Popup::new(" Model Config ")
                .accent(ratatui::style::Color::Magenta);
            let inner = popup.render(frame.area(), frame.buffer_mut());
            let widget = crate::widgets::model_config_widget::ModelConfigWidget;
            widget.render_ref(
                inner,
                frame.buffer_mut(),
                &mut self.state.model_config_state,
            );
        }
    }

    /// Key handling while the model config popup is open.
    /// Persist a model spec into the agent's stored record so it survives restarts.
    fn persist_agent_model(&mut self, agent_name: &str, model_spec: &str) {
        let Some(host) = self.host.clone() else {
            tracing::warn!("no host available for model persistence");
            return;
        };
        let name = agent_name.to_string();
        let spec = model_spec.to_string();
        self.runtime_handle.spawn(async move {
            let storage = host.storage();
            // Read the current record.
            let Some(mut record) = storage
                .get_agent_by_name(&name)
                .await
                .ok()
                .flatten()
            else {
                tracing::warn!(agent = %name, "agent record not found for model persistence");
                return;
            };
            // Update preferred_model inside config_json.
            if let Some(obj) = record.config_json.as_object_mut() {
                obj.insert(
                    "preferred_model".to_string(),
                    serde_json::Value::String(spec.clone()),
                );
            }
            record.last_active = chrono::Utc::now().timestamp_millis();
            // Upsert the updated record.
            if let Err(e) = storage.upsert_agent(record).await {
                tracing::error!(agent = %name, error = %e, "failed to persist model spec");
            } else {
                tracing::info!(agent = %name, model = %spec, "model spec persisted to agent record");
            }
        });
    }

    fn handle_model_config_key(&mut self, key: &KeyEvent) {
        use crate::widgets::model_config_widget::ConfigCommand;

        // Esc closes the popup.
        if key.code == KeyCode::Esc {
            self.state.model_config_visible = false;
            return;
        }

        let cmd = self.state.model_config_state.handle_key(*key);
        match cmd {
            ConfigCommand::SaveProvider {
                provider_name,
                api_key,
                base_url,
            } => {
                self.save_provider_config(&provider_name, &api_key, &base_url);
            }
            ConfigCommand::SelectModel {
                provider_name,
                model_name,
            } => {
                // Build the model and apply to the active agent's handle.
                let spec = format!("{provider_name}:{model_name}");
                if let Some(model) = Self::build_model_from_spec(&self.conn, &spec) {
                    if let Some(handle) = self.handles.get(self.state.active_agent_idx) {
                        handle.set_model(model);
                        tracing::info!(
                            agent_idx = self.state.active_agent_idx,
                            model = %spec,
                            "model hot-swapped for active agent"
                        );

                        // Persist the model spec into the agent's record so it
                        // survives restarts. We update the `preferred_model`
                        // field inside the stored `config_json`.
                        let agent_name = handle.name.clone();
                        self.persist_agent_model(&agent_name, &spec);
                    }
                }
                self.state.model_config_visible = false;
            }
            ConfigCommand::None => {}
        }
    }

    /// Insert or update a provider's api_key and base_url in the database.
    fn save_provider_config(&self, provider_name: &str, api_key: &str, base_url: &str) {
        // Look up the built-in provider to get its type and default base_url.
        let provider = self
            .state
            .model_config_state
            .providers
            .iter()
            .find(|p| p.name == provider_name);

        let Some(provider) = provider else {
            tracing::error!("provider not found in catalog: {provider_name}");
            return;
        };

        let provider_type = provider.provider_type.as_str().to_string();
        // Prefer the user-supplied base_url; fall back to the provider default
        // (the registry's first preset endpoint) when the field is empty.
        let base_url = if base_url.is_empty() {
            provider.selected_base_url.clone()
        } else {
            base_url.to_string()
        };
        // Resolve the default auth method from the registry for this provider.
        let auth_str =
            match agentik_sdk::provider::registry::default_auth_method(&provider.provider_type) {
                AuthMethod::Bearer => "Bearer",
                AuthMethod::Anthropic => "Anthropic",
            };

        // Check if a row for this provider name already exists.
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM providers WHERE name = ?1",
                [provider_name],
                |row| row.get(0),
            )
            .ok();

        let result = if let Some(id) = existing {
            self.conn.execute(
                "UPDATE providers SET api_key = ?1, base_url = ?2, auth_method = ?3 WHERE id = ?4",
                rusqlite::params![api_key, &base_url, auth_str, id],
            )
        } else {
            self.conn.execute(
                "INSERT INTO providers (name, provider_type, base_url, api_key, auth_method)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![provider_name, &provider_type, &base_url, api_key, auth_str],
            )
        };

        match &result {
            Ok(_) => tracing::info!("saved provider config: {provider_name}"),
            Err(e) => tracing::error!("failed to save provider config: {e}"),
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// ── Ctrl+R history search helpers ──────────────────────────
//
// Free functions operating on `AgentTabState`. Search state lives on the
// state struct (see `state.rs`); these compute matches and drive previews.

use std::collections::VecDeque;

/// Return indices into `history` (newest-first) whose text case-insensitively
/// contains `query`. An empty query matches everything, so the user starts at
/// the most recent entry and narrows as they type.
fn compute_search_matches(history: &VecDeque<String>, query: &str) -> Vec<usize> {
    let needle = query.to_lowercase();
    history
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, s)| needle.is_empty() || s.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

/// Load the match at `history_search_selected` into the input buffer so the
/// user sees a live preview as they navigate matches. Clears the buffer when
/// no match is selected.
fn load_selected_history_match(ts: &mut crate::state::AgentTabState) {
    if let Some(&idx) = ts.history_search_matches.get(ts.history_search_selected) {
        if let Some(entry) = ts.input_history.get(idx).cloned() {
            ts.input.clear();
            ts.input.insert_str(&entry);
        }
    } else {
        ts.input.clear();
    }
}

/// Recompute the match list for the current query, reset selection to the
/// newest match, and load its preview into the buffer.
fn recompute_history_search(ts: &mut crate::state::AgentTabState) {
    let q = ts.history_search_query.clone();
    ts.history_search_matches = compute_search_matches(&ts.input_history, &q);
    ts.history_search_selected = 0;
    load_selected_history_match(ts);
}

/// Leave history search mode, clearing transient search state. The buffer
/// retains whatever was previewed (on Enter) or the restored draft (on Esc);
/// the caller is responsible for buffer contents on the way in.
fn end_history_search(ts: &mut crate::state::AgentTabState) {
    ts.in_history_search = false;
    ts.history_search_query.clear();
    ts.history_search_matches.clear();
    ts.history_search_selected = 0;
    ts.history_search_draft = None;
}

/// Convert a flat list of SDK `Message`s (as returned by
/// `Memory::render_context()`) into TUI `ChatLine`s for display.
///
/// Each message can contain multiple `ContentBlock`s:
/// - User text → `ChatLine::User`
/// - System text → skipped (system prompt isn't shown)
/// - Assistant text → `ChatLine::Assistant`
/// - Assistant thinking → `ChatLine::Thinking`
/// - `tool_use` → `ChatLine::ToolCall`
/// - `tool_result` → `ChatLine::ToolResult`
///
/// Summary checkpoint messages (from compaction) are rendered as
/// `ChatLine::Assistant` with a prefix so the user can see what was
/// summarized.
fn messages_to_chatlines(messages: &[Message]) -> Vec<state::ChatLine> {
    let mut lines = Vec::new();
    for msg in messages {
        match msg.role {
            Role::User => {
                // A user message may contain multiple blocks (text +
                // tool_result). Separate them.
                let mut text_parts = String::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text_parts.is_empty() {
                                text_parts.push('\n');
                            }
                            text_parts.push_str(text);
                        }
                        ContentBlock::ToolResult {
                            content,
                            is_error,
                            ..
                        } => {
                            if !text_parts.is_empty() {
                                lines.push(state::ChatLine::User(
                                    std::mem::take(&mut text_parts),
                                ));
                            }
                            lines.push(state::ChatLine::ToolResult {
                                ok: !is_error.unwrap_or(false),
                                content: content.clone().unwrap_or_default(),
                            });
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    lines.push(state::ChatLine::User(text_parts));
                }
            }
            Role::Assistant => {
                let mut text_parts = String::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text_parts.is_empty() {
                                text_parts.push('\n');
                            }
                            text_parts.push_str(text);
                        }
                        ContentBlock::Thinking { thinking, .. } if !thinking.is_empty() => {
                            if !text_parts.is_empty() {
                                lines.push(state::ChatLine::Assistant {
                                    text: std::mem::take(&mut text_parts),
                                    usage: None,
                                });
                            }
                            lines.push(state::ChatLine::Thinking(thinking.clone()));
                        }
                        ContentBlock::ToolUse { name, input, .. } => {
                            if !text_parts.is_empty() {
                                lines.push(state::ChatLine::Assistant {
                                    text: std::mem::take(&mut text_parts),
                                    usage: None,
                                });
                            }
                            lines.push(state::ChatLine::ToolCall {
                                name: name.clone(),
                                input: serde_json::to_string_pretty(input)
                                    .unwrap_or_else(|_| input.to_string()),
                            });
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    lines.push(state::ChatLine::Assistant {
                        text: text_parts,
                        usage: None,
                    });
                }
            }
        }
    }
    lines
}
