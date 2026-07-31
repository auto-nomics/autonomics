use std::sync::Arc;
use std::time::{Duration, Instant};

use agentik_sdk::AuthMethod;
use agentik_sdk::model::{Model, ModelInfo, ProviderConfig, ProviderType};
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
use ratatui_comfy_tabs::{TabBarAlign, TabDirection, TabNav, TabNavState};
use rusqlite::Connection;
use std::io::{Stdout, Write, stdout};
use uuid::Uuid;

use crate::state::{self, AgentStatus, AppState, InputMode, MainTabState};
use crate::widgets::agent_tab_widget::AgentTabWidget;
use runtime::AgentRuntime;

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
    tab_state: TabNavState,
    agent_runtime: AgentRuntime,
    /// Kept alive to drive the agent's background event loop task.
    _runtime: Option<tokio::runtime::Runtime>,
    conn: Connection,
    /// Internal event channel for decoupled communication.
    app_event_rx: tokio::sync::mpsc::UnboundedReceiver<crate::app_event::AppEvent>,
    /// Sender half exposed for subsystems (file search, plugins, etc.)
    /// to push events into the main loop without direct App access.
    #[allow(dead_code)]
    pub(crate) app_event_tx: crate::app_event_sender::AppEventSender,
    /// Set to break the main event loop so `ratatui::run()` can call `restore()`.
    should_quit: bool,
    /// Timestamp of the last cooperative cancel (Ctrl+C while agent running).
    /// A second Ctrl+C within `FORCE_QUIT_WINDOW` forces an immediate quit.
    cancel_requested_at: Option<Instant>,
}

impl App {
    pub fn new() -> Self {
        let conn = Connection::open("phloem.db").expect("failed to open phloem.db");

        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("failed to enable foreign_keys");

        Self::init_database(&conn).expect("failed to initialize database schema");

        let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

        // Try to build a Model from DB; if none configured yet, start without an agent.
        let model = Arc::new(ArcSwapOption::from_pointee(Self::build_model(&conn)));

        let agent_runtime =
            AgentRuntime::new(&runtime, model.clone()).expect("failed to build agent runtime");

        let mut state = AppState {
            active_model: model,
            ..Default::default()
        };

        Self::load_model_config(&conn, &mut state.model_config_state);

        let (app_event_tx, app_event_rx) = tokio::sync::mpsc::unbounded_channel();

        Self {
            state,
            tab_state: TabNavState::new(MainTabState::default().index()),
            agent_runtime,
            _runtime: Some(runtime),
            conn,
            app_event_rx,
            app_event_tx: crate::app_event_sender::AppEventSender::new(app_event_tx),
            should_quit: false,
            cancel_requested_at: None,
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
        use agentik_sdk::provider::registry;

        // Read the active model setting.
        let active: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'active_model'",
                [],
                |row| row.get(0),
            )
            .ok()?;

        // Parse "provider_name:model_name"
        let (provider_name, model_name) = active.split_once(':')?;

        // Look up provider credentials from DB (only api_key matters —
        // base_url always comes from the registry so code updates take
        // effect without needing to re-write the DB).
        let api_key: String = conn
            .query_row(
                "SELECT api_key FROM providers WHERE name = ?1",
                [provider_name],
                |row| row.get(0),
            )
            .ok()?;

        if api_key.is_empty() {
            return None;
        }

        // Look up model info + base_url from the built-in catalog.
        let provider_type = ProviderType::from(provider_name);
        let base_url = registry::default_base_url(&provider_type)
            .unwrap_or("")
            .to_string();
        let auth_method = registry::default_auth_method(&provider_type);
        let preset_models = registry::preset_models(&provider_type)?;
        let mut model_info = preset_models
            .into_iter()
            .find(|m| m.model_name == model_name)?;

        // Build ProviderConfig: api_key from DB, base_url and auth_method
        // from registry defaults.
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
                input_token_price           REAL    NOT NULL DEFAULT 0,
                output_token_price          REAL    NOT NULL DEFAULT 0,
                FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
            )",
            (),
        )?;

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
        self.agent_runtime.shutdown();

        // Restore terminal on exit (whether normal or error).
        let _ = restore_terminal();

        result?;
        Ok(())
    }

    /// Async main loop driven by `tokio::select!` with a fixed-rate render tick.
    ///
    /// **Design**: state mutation and rendering are strictly separated.
    ///
    /// - **Event branches** (terminal, agent, app) only mutate state.
    ///   They never render.
    /// - **Render tick** fires at a fixed interval (~60 fps) and redraws
    ///   unconditionally. Ratatui's internal buffer-diff ensures only
    ///   changed cells are written to the terminal, so idle frames are
    ///   near-zero cost.
    ///
    /// This eliminates all drain/batch logic and the `dirty` flag: events
    /// between two ticks are naturally coalesced into a single render.
    /// The first interval tick completes immediately, so the initial frame
    /// is drawn right away.
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
                    if let Some(Ok(event)) = maybe_event {
                        let delta = self.handle_event(&event);
                        if delta != 0 {
                            self.apply_scroll_delta(delta);
                        }
                    }
                }

                // ── Agent streaming events ──
                maybe_agent = self.agent_runtime.recv_event() => {
                    if let Some(event) = maybe_agent {
                        state::apply_event(&mut self.state.agent_tab_state, event);
                    } else {
                        // Agent channel closed → shutdown.
                        self.should_quit = true;
                    }
                }

                // ── App internal events ──
                maybe_app = self.app_event_rx.recv() => {
                    match maybe_app {
                        Some(event) => self.handle_app_event(event),
                        None => self.should_quit = true,
                    }
                }

                // ── Fixed-rate render tick ──
                _ = render_tick.tick() => {
                    if matches!(self.state.agent_tab_state.status, AgentStatus::Idle) {
                        self.clear_cancel_pending();
                    } else {
                        // Advance animation frame counter while the agent is active.
                        self.state.agent_tab_state.frame =
                            self.state.agent_tab_state.frame.wrapping_add(1);
                    }
                    terminal.draw(|f| self.render(f))?;
                }
            }
        }
    }

    /// Apply an internal [`AppEvent`] to state.
    fn handle_app_event(&mut self, event: crate::app_event::AppEvent) {
        match event {
            crate::app_event::AppEvent::Agent(e) => {
                state::apply_event(&mut self.state.agent_tab_state, *e);
            }
            crate::app_event::AppEvent::Quit => {
                self.should_quit = true;
            }
            crate::app_event::AppEvent::ConfigReload => {
                Self::load_model_config(&self.conn, &mut self.state.model_config_state);
            }
        }
    }

    /// Handle a single event. Returns a scroll delta to be accumulated.
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
                if matches!(self.state.main_tab_state, MainTabState::AgentTab) {
                    let ts = &mut self.state.agent_tab_state;
                    if ts.input_mode == InputMode::Input && ts.status == state::AgentStatus::Idle {
                        ts.input.insert_str(s);
                    }
                }
                // Insert paste into the api_key textarea when in Config mode.
                if matches!(self.state.main_tab_state, MainTabState::ConfigTab) {
                    use crate::widgets::model_config_widget::ProviderPanelState;
                    if let ProviderPanelState::Config { textarea, .. } =
                        &mut self.state.model_config_state.provider_panel_state
                    {
                        textarea.insert_str(s);
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
        if !matches!(self.state.main_tab_state, MainTabState::AgentTab) {
            return 0;
        }

        let lines_per_tick: i32 = 3;

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                let ts = &mut self.state.agent_tab_state;
                ts.auto_scroll = false;
                lines_per_tick
            }
            MouseEventKind::ScrollUp => {
                let ts = &mut self.state.agent_tab_state;
                ts.auto_scroll = false;
                -lines_per_tick
            }
            _ => 0,
        }
    }

    /// Apply a batched scroll delta to the agent tab.
    fn apply_scroll_delta(&mut self, delta: i32) {
        if !matches!(self.state.main_tab_state, MainTabState::AgentTab) {
            return;
        }
        let ts = &mut self.state.agent_tab_state;
        if delta > 0 {
            ts.scroll_offset = ts.scroll_offset.saturating_add(delta as usize);
        } else {
            ts.scroll_offset = ts.scroll_offset.saturating_sub((-delta) as usize);
        }
    }

    fn handle_key(&mut self, key: &KeyEvent) {
        // Ctrl+C: cancel running agent first, then quit on second press.
        // If the agent is blocked and doesn't transition to Idle after the
        // first cancel, a second Ctrl+C within FORCE_QUIT_WINDOW force-quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.should_quit {
                // Already quitting — no-op.
                return;
            }
            if matches!(self.state.agent_tab_state.status, AgentStatus::Idle) {
                self.should_quit = true;
                return;
            }
            // Agent is running — check for force-quit (double Ctrl+C).
            if let Some(ts) = self.cancel_requested_at {
                if ts.elapsed() < FORCE_QUIT_WINDOW {
                    tracing::info!("force-quit: second Ctrl+C within {:?}", FORCE_QUIT_WINDOW);
                    self.agent_runtime.shutdown();
                    self.should_quit = true;
                    return;
                }
            }
            // First Ctrl+C: cooperative cancel.
            self.agent_runtime.cancel();
            self.cancel_requested_at = Some(Instant::now());
            return;
        }

        // Ctrl+G: toggle the auto-scroll-to-bottom lock on the Agent tab.
        // When locked (following the tail), the first press releases the lock
        // so the user can scroll freely; a second press re-pins to the bottom.
        // Scroll-producing keys/mouse also release the lock (see handle_mouse
        // and handle_browse_key); Ctrl+G is the dedicated toggle.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('g')
            && matches!(self.state.main_tab_state, MainTabState::AgentTab)
        {
            let ts = &mut self.state.agent_tab_state;
            if ts.auto_scroll {
                ts.auto_scroll = false;
            } else {
                ts.scroll_to_bottom();
            }
            return;
        }

        // Tab switching (global)
        match key.code {
            KeyCode::Char(']') => {
                self.tab_state
                    .select_direction_wrapping(TabDirection::Next, state::TABS.len());
                self.sync_tab_state();
                return;
            }
            KeyCode::Char('[') => {
                self.tab_state
                    .select_direction_wrapping(TabDirection::Previous, state::TABS.len());
                self.sync_tab_state();
                return;
            }
            _ => {}
        }

        // Delegate to active tab
        match self.state.main_tab_state {
            MainTabState::AgentTab => {
                self.handle_agent_key(key);
            }
            MainTabState::ConfigTab => {
                self.handle_config_key(key);
            }
        }
    }

    fn handle_agent_key(&mut self, key: &KeyEvent) {
        let ts = &mut self.state.agent_tab_state;

        match ts.input_mode {
            InputMode::Browse => self.handle_browse_key(key),
            InputMode::Input => self.handle_input_key(key),
        }
    }

    /// Key handling in browse mode: Up/Down scroll line-by-line,
    /// PageDown/PageUp half-page, Home/End jump to top/bottom,
    /// Enter enters the composer (input mode).
    fn handle_browse_key(&mut self, key: &KeyEvent) {
        let ts = &mut self.state.agent_tab_state;

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
        if self.state.agent_tab_state.in_history_search {
            self.handle_history_search_key(key);
            return;
        }

        let ts = &mut self.state.agent_tab_state;
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
                    self.agent_runtime.send_message(text);
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
    }

    /// Key handling while a Ctrl+R incremental history search is active.
    fn handle_history_search_key(&mut self, key: &KeyEvent) {
        let ts = &mut self.state.agent_tab_state;
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

    fn sync_tab_state(&mut self) {
        self.state.main_tab_state = MainTabState::from_index(self.tab_state.selected);
    }

    fn render(&mut self, frame: &mut Frame) {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // TabBar
                Constraint::Min(5),    // Content (Widget handles its own layout)
            ])
            .split(frame.area());

        // ── TabBar ──
        let tabs = TabNav::new(state::TABS, self.tab_state.selected)
            .tab_bar_align(TabBarAlign::Center)
            .highlight_style(ratatui::style::Style::default().yellow());
        frame.render_stateful_widget(tabs, areas[0], &mut self.tab_state);

        match self.state.main_tab_state {
            MainTabState::AgentTab => {
                use ratatui::widgets::StatefulWidgetRef;
                // Load the current model name from the shared ArcSwapOption.
                // `load_full()` returns an owned Option<Arc<Model>> (one refcount
                // bump), so the immutable borrow of `active_model` is released
                // before we mutably borrow `agent_tab_state` below.
                let model_name = self
                    .state
                    .active_model
                    .load_full()
                    .map(|m| m.model_info.model_name.clone());
                let widget = AgentTabWidget {
                    active_model: model_name.as_deref(),
                };
                widget.render_ref(areas[1], frame.buffer_mut(), &mut self.state.agent_tab_state);

                // Position the terminal hardware cursor over the chat input.
                // The xAI TextArea renders only text; the host must place the
                // caret itself. Calling set_cursor_position inside the draw
                // closure makes ratatui emit show_cursor + the move without a
                // hide→show cycle that resets the blink timer every frame.
                if let Some((cx, cy)) = self.state.agent_tab_state.input.last_cursor_pos() {
                    frame.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
                }
            }
            MainTabState::ConfigTab => {
                use ratatui::widgets::StatefulWidgetRef;
                let widget = crate::widgets::model_config_widget::ModelConfigWidget;
                widget.render_ref(
                    areas[1],
                    frame.buffer_mut(),
                    &mut self.state.model_config_state,
                );
            }
        }
    }

    // ── Config tab ──────────────────────────────────────

    /// Config tab key handling.
    ///
    /// Widget-internal keys are delegated to [`ModelConfigState::handle_key`],
    /// which returns a [`ConfigCommand`] for operations requiring DB access.
    /// Unrecognized keys fall through to App-level handlers (e.g. `r` reload).
    fn handle_config_key(&mut self, key: &KeyEvent) {
        use crate::widgets::model_config_widget::ConfigCommand;

        let cmd = self.state.model_config_state.handle_key(*key);
        match cmd {
            ConfigCommand::SaveProvider {
                provider_name,
                api_key,
            } => {
                self.save_provider_config(&provider_name, &api_key);
            }
            ConfigCommand::SelectModel {
                provider_name,
                model_name,
            } => {
                self.activate_model(&provider_name, &model_name);
            }
            ConfigCommand::None => {
                // Key not consumed by widget — try App-level handlers.
                match key.code {
                    KeyCode::Char('r') => {
                        Self::load_model_config(&self.conn, &mut self.state.model_config_state);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Persist the active model selection to the `settings` table and hot-swap
    /// the model in the shared [`ArcSwapOption`]. The agent picks up the new
    /// model on its next turn — no runtime restart needed.
    fn activate_model(&mut self, provider_name: &str, model_name: &str) {
        let value = format!("{provider_name}:{model_name}");

        // Persist to settings table.
        let _ = self.conn.execute(
            "INSERT INTO settings (key, value) VALUES ('active_model', ?1)
             ON CONFLICT(key) DO UPDATE SET value = ?1",
            [&value],
        );

        // Hot-swap: rebuild the Model from DB and atomically store it.
        if let Some(new_model) = Self::build_model(&self.conn) {
            self.state.active_model.store(Some(Arc::new(new_model)));
            tracing::info!("model '{value}' hot-swapped");
        } else {
            tracing::warn!("model '{value}' saved but failed to build — restart to apply");
        }
    }

    /// Insert or update a provider's api_key in the database.
    fn save_provider_config(&self, provider_name: &str, api_key: &str) {
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
        let base_url = provider.base_url.clone();
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
