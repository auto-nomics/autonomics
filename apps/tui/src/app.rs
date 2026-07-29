use std::time::{Duration, Instant};

use agentik_sdk::AuthMethod;
use agentik_sdk::model::{Model, ModelInfo, ProviderConfig, ProviderType};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind,
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

use crate::state::{self, AgentStatus, AppState, ConfigMode, InputMode, MainTabState};
use crate::widgets::agent_tab_widget::AgentTabWidget;
use runtime::AgentRuntime;

const POLL_TIMEOUT_ACTIVE: Duration = Duration::from_millis(16);
const POLL_TIMEOUT_IDLE: Duration = Duration::from_millis(100);

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
    _runtime: tokio::runtime::Runtime,
    conn: Connection,
    /// Internal event channel for decoupled communication.
    app_event_rx: std::sync::mpsc::Receiver<crate::app_event::AppEvent>,
    /// Sender half exposed for subsystems (file search, plugins, etc.)
    /// to push events into the main loop without direct App access.
    #[allow(dead_code)]
    pub(crate) app_event_tx: crate::app_event_sender::AppEventSender,
    /// True when state has changed and a re-render is needed.
    dirty: bool,
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

        let model = Self::build_model(&conn).expect("failed to build model");

        let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let agent_runtime =
            AgentRuntime::new(&runtime, model).expect("failed to create agent runtime");

        let mut state = AppState::default();
        crate::config_db::reload_config(&mut state.config_tab_state, &conn);

        let (app_event_tx, app_event_rx) = std::sync::mpsc::channel();

        Self {
            state,
            tab_state: TabNavState::new(MainTabState::default().index()),
            agent_runtime,
            _runtime: runtime,
            conn,
            app_event_rx,
            app_event_tx: crate::app_event_sender::AppEventSender::new(app_event_tx),
            dirty: true, // render the initial frame
            should_quit: false,
            cancel_requested_at: None,
        }
    }

    /// Build a single `Model` from the database: pick the first model row,
    /// join it with its referenced provider, and construct the `Model`.
    fn build_model(conn: &Connection) -> Result<Model, Box<dyn std::error::Error>> {
        // Load all providers into a lookup map keyed by integer id.
        let mut stmt = conn.prepare(
            "SELECT id, name, base_url, api_key, auth_method, provider_type FROM providers",
        )?;
        let providers: Vec<ProviderConfig> = stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let auth_method: String = row.get(4)?;
                let auth: AuthMethod = auth_method.try_into().map_err(
                    |e: agentik_sdk::types::errors::AnthropicError| {
                        rusqlite::Error::ToSqlConversionFailure(e.into())
                    },
                )?;
                let provider_type: String = row.get(5)?;
                Ok(ProviderConfig {
                    id: Uuid::from_u128(id as u128),
                    name: row.get(1)?,
                    base_url: row.get(2)?,
                    api_key: row.get(3)?,
                    provider_type: ProviderType::from(provider_type),
                    auth_method: auth,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Pick the first model row.
        let mut stmt = conn.prepare(
            "SELECT model_name, provider_id, context_length, max_output_tokens,
                    vision_ability, supports_function_calling, supports_streaming,
                    supports_thinking, input_token_price, output_token_price
             FROM models ORDER BY id LIMIT 1",
        )?;
        let model_info = stmt
            .query_row([], |row| {
                let provider_id: i64 = row.get(1)?;
                Ok(ModelInfo {
                    model_name: row.get(0)?,
                    provider_id: Uuid::from_u128(provider_id as u128),
                    context_length: row.get::<_, i64>(2)? as u64,
                    max_output_tokens: row.get::<_, i64>(3)? as u64,
                    vision_ability: row.get::<_, i32>(4)? != 0,
                    supports_function_calling: row.get::<_, i32>(5)? != 0,
                    supports_streaming: row.get::<_, i32>(6)? != 0,
                    supports_thinking: row.get::<_, i32>(7)? != 0,
                    input_token_price: row.get(8)?,
                    output_token_price: row.get(9)?,
                })
            })
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

        // Find the matching provider.
        let provider = providers
            .iter()
            .find(|p| p.id == model_info.provider_id)
            .ok_or_else(|| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "model '{}' references unknown provider_id {}",
                        model_info.model_name, model_info.provider_id
                    ),
                )) as Box<dyn std::error::Error>
            })?;

        Model::new(model_info, provider).map_err(Into::into)
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

        let result = self.app(&mut terminal);

        // Ensure the agent and engine tasks are torn down even if the main
        // loop exited without a cooperative shutdown (e.g. force-quit).
        self.agent_runtime.shutdown();

        // Restore terminal on exit (whether normal or error).
        let _ = restore_terminal();

        result?;
        Ok(())
    }

    fn app(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> std::io::Result<()> {
        loop {
            if self.should_quit {
                break Ok(());
            }

            // ── Drain internal app events (non-blocking) ──
            while let Ok(event) = self.app_event_rx.try_recv() {
                match event {
                    crate::app_event::AppEvent::Agent(e) => {
                        state::apply_event(&mut self.state.agent_tab_state, *e);
                    }
                    crate::app_event::AppEvent::Quit => {
                        self.should_quit = true;
                    }
                    crate::app_event::AppEvent::ConfigReload => {
                        crate::config_db::reload_config(
                            &mut self.state.config_tab_state,
                            &self.conn,
                        );
                    }
                }
                self.dirty = true;
            }

            // ── Event handling phase ──
            // Drain all queued input events before rendering.
            if event::poll(self.poll_timeout())? {
                let mut scroll_delta: i32 = 0;
                loop {
                    let event = event::read()?;
                    let scroll = self.handle_event(&event);
                    scroll_delta += scroll;
                    // Check for more events without blocking.
                    if !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
                // Apply batched scroll delta once.
                if scroll_delta != 0 {
                    self.apply_scroll_delta(scroll_delta);
                }
                self.dirty = true;
            } else {
                // Timeout: drain agent streaming events.
                let had_events = self.drain_agent_events();
                if had_events {
                    self.dirty = true;
                }
            }

            // ── Render phase (only when state changed) ──
            if self.dirty {
                terminal.draw(|f| self.render(f))?;
                self.dirty = false;
            }
        }
    }

    /// Return a poll timeout appropriate for the current agent status.
    /// When idle we poll less frequently to save CPU; during streaming we
    /// poll at ~60 fps for smooth text rendering.
    fn poll_timeout(&self) -> Duration {
        match self.state.agent_tab_state.status {
            AgentStatus::Idle => POLL_TIMEOUT_IDLE,
            _ => POLL_TIMEOUT_ACTIVE,
        }
    }

    fn drain_agent_events(&mut self) -> bool {
        let mut had_events = false;
        while let Some(event) = self.agent_runtime.poll_event() {
            state::apply_event(&mut self.state.agent_tab_state, event);
            had_events = true;
        }
        // Clear the cancel-pending flag once the agent is idle so a single
        // Ctrl+C will quit normally next time.
        if had_events && matches!(self.state.agent_tab_state.status, AgentStatus::Idle) {
            self.clear_cancel_pending();
        }
        had_events
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
                // Insert paste into the focused field of config tab forms (Provider / Model).
                if matches!(self.state.main_tab_state, MainTabState::ConfigTab) {
                    let cs = &mut self.state.config_tab_state;
                    match &mut cs.mode {
                        ConfigMode::EditProvider(form) => {
                            let f = form.focus;
                            form.fields_mut()[f].insert_str(s);
                        }
                        ConfigMode::EditModel(form) => {
                            let f = form.focus;
                            if f < state::ModelForm::TEXT_FIELD_COUNT {
                                form.text_fields_mut()[f].insert_str(s);
                            }
                        }
                        ConfigMode::Browsing => {}
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
                let widget = AgentTabWidget {
                    state: &mut self.state.agent_tab_state,
                };
                frame.render_widget(widget, areas[1]);

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
                crate::widgets::config_widget::render_config(
                    &self.state.config_tab_state,
                    areas[1],
                    frame.buffer_mut(),
                );
            }
        }
    }

    // ── Config tab ──────────────────────────────────────

    /// Config tab key handling. Esc/Enter-in-form/'d'-delete need database
    /// access, so they are handled here; everything else is delegated to the
    /// widget's key handler.
    fn handle_config_key(&mut self, key: &KeyEvent) {
        let code = key.code;
        let cs = &mut self.state.config_tab_state;

        // Esc closes any open form.
        if code == KeyCode::Esc && !matches!(cs.mode, state::ConfigMode::Browsing) {
            cs.mode = state::ConfigMode::Browsing;
            cs.message.clear();
            return;
        }

        // Enter inside a form validates and saves.
        if code == KeyCode::Enter {
            if matches!(cs.mode, state::ConfigMode::EditProvider(_)) {
                cs.message = crate::config_db::save_provider(cs, &self.conn).unwrap_or_else(|e| e);
                return;
            }
            if matches!(cs.mode, state::ConfigMode::EditModel(_)) {
                cs.message = crate::config_db::save_model(cs, &self.conn).unwrap_or_else(|e| e);
                return;
            }
        }

        // 'd' in browsing deletes the selected row.
        if code == KeyCode::Char('d') && matches!(cs.mode, state::ConfigMode::Browsing) {
            cs.message = crate::config_db::delete_selected(cs, &self.conn);
            return;
        }

        // Everything else (navigation, opening forms, typing) goes to the widget.
        crate::widgets::config_widget::handle_config_key(&mut self.state.config_tab_state, *key);
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
