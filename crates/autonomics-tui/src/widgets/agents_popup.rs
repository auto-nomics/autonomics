//! Unified agents popup (`Ctrl+T`): the single browser over every agent
//! identity — live instances, dormant (stored-but-passivated) identities,
//! and the new-agent section — built on [`PickerState`] like the other
//! popups. This replaces both the former switch-only agents popup and the
//! resume-only profile picker.
//!
//! Rows carry the status glyph (hollow `◌` on dormant rows), the
//! workspace-root-stripped name (bold on the active leaf), and trailing
//! model / effort columns (live rows only). A status-tab strip (glyph +
//! count, including a `Dormant` pseudo-status) filters by lifecycle
//! state; typing filters the loaded page and searches stored identities;
//! the `Alt+h`/`Alt+l` column
//! cursor picks what Enter does on the selected row — open/switch the
//! leaf (or resume a dormant identity), open the model quick-picker, or
//! cycle the effort override. `Alt+m`/`Alt+M`/`Alt+e`/`Alt+E` are the direct shortcuts.
//! `Ctrl+R` renames and `Ctrl+D` deletes the selected identity (inline
//! confirmation), `Ctrl+W` closes its leaf.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, StatefulWidgetRef, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::state::AgentStatus;
use crate::widgets::agent_workspace::{display_name, status_color, status_icon};
use crate::widgets::popup::{Popup, PopupControls};
use crate::widgets::searchable_picker::{PickerItem, PickerState};
use crate::xai_textarea::{TextArea, TextAreaState};

/// Prompt gutter width for the search input: `> ` = 2 cols.
const PROMPT_GUTTER: u16 = 2;

/// Fixed widths of the trailing columns. The effort column fits the longest
/// common effort id (`medium`).
const MODEL_COL: u16 = 12;
const EFFORT_COL: u16 = 6;

/// Glyph for dormant rows: a hollow circle — the identity exists in
/// storage, but no live instance holds it.
const DORMANT_GLYPH: &str = "◌";

// ═══════════════════════════════════════════════════════════════════════
// Data
// ═══════════════════════════════════════════════════════════════════════

/// One agent identity row in the popup, supplied by the app when the
/// popup opens or a refresh lands (a snapshot — statuses are live only
/// for that moment).
#[derive(Clone, Debug)]
pub struct AgentRow {
    /// Full hierarchical path (e.g. `/root/researcher/worker`).
    pub name: String,
    pub agent_id: uuid::Uuid,
    pub status: AgentStatus,
    /// Whether the daemon currently holds a live instance for this
    /// identity. `false` = dormant (passivated, resumable from storage).
    pub live: bool,
    /// The focused leaf is this identity.
    pub is_active: bool,
    /// Any open leaf covers this identity.
    pub is_open: bool,
    /// The agent's live model spec (`provider:model`) from the render
    /// cache; `None` while the per-agent fetch is still in flight. On
    /// dormant rows the stored `preferred_model`, when present.
    pub model: Option<String>,
    /// The reasoning effort this agent's next request would send; `None`
    /// on non-thinking models (column `–`) and alongside a `None` model.
    pub effort: Option<String>,
    /// Serialized `AgentProfileConfig` from the stored record; the
    /// resume path re-spawns from it. Empty on live rows whose record
    /// fell outside the loaded page.
    pub config_json: serde_json::Value,
}

impl AgentRow {
    /// The text shown for this row: the workspace-root-stripped path.
    pub fn shown_name(&self) -> &str {
        display_name(&self.name)
    }
}

/// Order rows shallow-path-first, then lexically — a stable order across
/// opens. Live rows sort among dormant ones by path depth only; the
/// status glyphs distinguish them.
pub fn sort_agent_rows(rows: &mut [AgentRow]) {
    rows.sort_by(|left, right| {
        let depth = |name: &str| name.split('/').filter(|part| !part.is_empty()).count();
        depth(&left.name)
            .cmp(&depth(&right.name))
            .then_with(|| left.name.cmp(&right.name))
    });
}

/// Picker item wrapping one popup row (identity or new-agent entry) with
/// a pre-built search blob, so `PickerState`'s query filter matches
/// everything the user would reasonably type.
#[derive(Clone, Debug)]
pub struct AgentsPopupItem {
    pub row: PopupRow,
    search: String,
}

#[derive(Clone, Debug)]
pub enum PopupRow {
    /// Virtual "new agent" entry — one per profile kind, prepended to
    /// the list.
    NewAgent(agentik_core::profile::AgentKind),
    Agent(AgentRow),
}

impl AgentsPopupItem {
    fn new(row: PopupRow) -> Self {
        let mut search = String::new();
        match &row {
            PopupRow::NewAgent(kind) => search.push_str(&format!("new {} ", kind.name())),
            PopupRow::Agent(agent) => {
                search.push_str(&format!("{} {} ", agent.name, display_name(&agent.name)));
                if let Some(model) = &agent.model {
                    search.push_str(model);
                }
            }
        }
        Self { row, search }
    }
}

impl PickerItem for AgentsPopupItem {
    fn title(&self) -> &str {
        match &self.row {
            PopupRow::NewAgent(kind) => kind.name(),
            PopupRow::Agent(agent) => agent.shown_name(),
        }
    }
    fn keywords(&self) -> &str {
        &self.search
    }
    fn matches_query(&self, needle: &str) -> bool {
        let mut chars = self.search.chars().flat_map(char::to_lowercase);
        needle
            .chars()
            .all(|c| chars.any(|candidate| candidate == c))
    }
    fn category(&self) -> Option<&str> {
        match &self.row {
            PopupRow::NewAgent(kind) => Some(kind.name()),
            PopupRow::Agent(agent) => agent.model.as_deref(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Status tabs
// ═══════════════════════════════════════════════════════════════════════

/// A status-tab filter. `Dormant` is a popup-level pseudo-status: stored
/// identity with no live instance (it is deliberately NOT part of
/// `AgentLifecycleStatus`, which describes what a *live* run loop is
/// doing).
#[derive(Clone, Copy, PartialEq)]
enum TabFilter {
    All,
    Dormant,
    Status(AgentStatus),
}

const TABS: [(&str, TabFilter); 11] = [
    ("All", TabFilter::All),
    ("Dormant", TabFilter::Dormant),
    ("Idle", TabFilter::Status(AgentStatus::Idle)),
    ("Waiting", TabFilter::Status(AgentStatus::Waiting)),
    ("Requesting", TabFilter::Status(AgentStatus::Requesting)),
    ("Streaming", TabFilter::Status(AgentStatus::Streaming)),
    ("Tools", TabFilter::Status(AgentStatus::ToolRunning)),
    ("Retrying", TabFilter::Status(AgentStatus::Retrying)),
    ("Compacting", TabFilter::Status(AgentStatus::Compacting)),
    ("Error", TabFilter::Status(AgentStatus::Error)),
    ("Cancelled", TabFilter::Status(AgentStatus::Cancelled)),
];

fn tab_matches(tab: usize, row: &AgentRow) -> bool {
    match TABS[tab].1 {
        TabFilter::All => true,
        TabFilter::Dormant => !row.live,
        TabFilter::Status(expected) => {
            row.live
                && match expected {
                    AgentStatus::Idle => row.status.is_idle(),
                    expected => row.status == expected,
                }
        }
    }
}

/// The tab-strip glyph + color for a tab (dormant gets its own hollow
/// marker; live statuses reuse the workspace icons).
fn tab_glyph(filter: &TabFilter) -> Option<(&'static str, Option<AgentStatus>)> {
    match filter {
        TabFilter::All => None,
        TabFilter::Dormant => Some((DORMANT_GLYPH, None)),
        TabFilter::Status(status) => Some((status_icon(status), Some(*status))),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Column cursor
// ═══════════════════════════════════════════════════════════════════════

/// The row column the cursor acts on: `h`/`l` move it, Enter dispatches
/// per column (name opens/resumes, model opens the picker, effort
/// cycles).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentColumn {
    #[default]
    Name,
    Model,
    Effort,
}

// ═══════════════════════════════════════════════════════════════════════
// Key outcome
// ═══════════════════════════════════════════════════════════════════════

/// Result of forwarding a key event to the agents popup.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentsPopupKeyOutcome {
    /// The key was consumed but no action is needed.
    Consumed,
    /// Open (or switch to) the leaf of a live agent.
    OpenOrSwitch { name: String, agent_id: uuid::Uuid },
    /// Re-spawn a dormant identity from its stored config and land on
    /// the new leaf.
    Resume {
        path: agentik_types::AgentPath,
        config_json: serde_json::Value,
    },
    /// Start the new-agent naming flow for this kind.
    NewAgent(agentik_core::profile::AgentKind),
    /// Rename a stored identity (storage confirmed path update).
    Rename {
        agent_id: uuid::Uuid,
        new_path: agentik_types::AgentPath,
    },
    /// Delete a stored identity after "yes" confirmation.
    Delete(uuid::Uuid),
    /// Open the model quick-picker for this agent (single path).
    OpenModelPicker { agent: String },
    /// Open the model quick-picker for every filtered agent (bulk path).
    OpenModelPickerBulk { agents: Vec<String> },
    /// Cycle this agent's effort override one stop.
    CycleEffort { agent: String },
    /// Open the bulk effort picker (the filter snapshot travels with it).
    OpenEffortPicker { agents: Vec<(String, bool)> },
    /// Close the popup.
    Close,
}

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

/// What the text input at the top of the popup currently edits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum InputMode {
    #[default]
    None,
    Rename,
    DeleteConfirm,
}

/// State for the unified agents popup.
pub struct AgentsPopupState {
    /// Item list + query filtering + list navigation.
    pub picker: PickerState<AgentsPopupItem>,
    /// Every row (pre-tab-filter), for tab counts and rebuilds.
    all: Vec<AgentsPopupItem>,
    status_tab: usize,
    /// The row column cursor (`h`/`l` move it; Enter dispatches per
    /// column).
    pub column: AgentColumn,
    /// Search input editor; captures typing while in `Search` mode.
    textarea: TextArea,
    textarea_state: TextAreaState,
    input_mode: InputMode,
    /// Rename target + draft + validation error (inline confirmation).
    rename_id: Option<uuid::Uuid>,
    rename_path: Option<agentik_types::AgentPath>,
    rename_input: String,
    rename_error: Option<String>,
    /// Delete confirmation: the target and the typed "yes".
    delete_confirm_id: Option<uuid::Uuid>,
    delete_confirm_input: String,
    /// On-screen cursor position computed during the most recent render.
    pub cursor_pos: Option<(u16, u16)>,
}

impl Default for AgentsPopupState {
    fn default() -> Self {
        let mut textarea = TextArea::new();
        textarea.show_scrollbar = false;
        Self {
            picker: PickerState::new_empty(),
            all: Vec::new(),
            status_tab: 0,
            column: AgentColumn::Name,
            textarea,
            textarea_state: TextAreaState::default(),
            input_mode: InputMode::None,
            rename_id: None,
            rename_path: None,
            rename_input: String::new(),
            rename_error: None,
            delete_confirm_id: None,
            delete_confirm_input: String::new(),
            cursor_pos: None,
        }
    }
}

impl AgentsPopupState {
    pub fn is_visible(&self) -> bool {
        self.picker.visible
    }

    pub fn close(&mut self) {
        self.picker.close();
        self.reset_inputs();
        self.status_tab = 0;
        self.column = AgentColumn::Name;
        self.cursor_pos = None;
    }

    fn reset_inputs(&mut self) {
        self.textarea.set_text("");
        self.textarea.clear_history();
        self.input_mode = InputMode::None;
        self.rename_id = None;
        self.rename_path = None;
        self.rename_input.clear();
        self.rename_error = None;
        self.delete_confirm_id = None;
        self.delete_confirm_input.clear();
    }

    /// Open with a full row snapshot (new-agent entries are prepended
    /// here; identity order is the caller's).
    pub fn open_with(&mut self, rows: Vec<AgentRow>) {
        self.reset_inputs();
        self.status_tab = 0;
        self.column = AgentColumn::Name;
        self.set_rows(rows);
        self.picker.open();
    }

    /// Replace the identity rows (a stored-records page landed or the
    /// registry changed), keeping the query, tab, and cursor mode. The
    /// selection clamps to the new list.
    pub fn refresh_with(&mut self, rows: Vec<AgentRow>) {
        self.set_rows(rows);
    }

    fn set_rows(&mut self, rows: Vec<AgentRow>) {
        let mut items: Vec<AgentsPopupItem> = agentik_core::profile::AgentKind::ALL
            .iter()
            .map(|kind| AgentsPopupItem::new(PopupRow::NewAgent(*kind)))
            .collect();
        items.extend(
            rows.into_iter()
                .map(|row| AgentsPopupItem::new(PopupRow::Agent(row))),
        );
        self.all = items;
        self.apply_tab();
    }

    /// The current search query (drives the debounced server-side
    /// identity search in the app layer).
    pub fn query(&self) -> &str {
        &self.picker.query
    }

    /// Rebuild the picker's item set from the tab filter, keeping the
    /// query; selection clamps to the new list.
    fn apply_tab(&mut self) {
        let tab = self.status_tab;
        let items: Vec<AgentsPopupItem> = self
            .all
            .iter()
            .filter(|item| match &item.row {
                // New-agent entries live on the All tab (and match the
                // query) only.
                PopupRow::NewAgent(_) => tab == 0,
                PopupRow::Agent(agent) => tab_matches(tab, agent),
            })
            .cloned()
            .collect();
        let query = self.picker.query.clone();
        self.picker.set_items(items);
        self.picker.set_query(&query);
    }

    /// The identity row the cursor sits on, if any.
    pub fn selected_agent(&self) -> Option<AgentRow> {
        self.picker
            .selected_item()
            .and_then(|item| match &item.row {
                PopupRow::Agent(agent) => Some(agent.clone()),
                PopupRow::NewAgent(_) => None,
            })
    }

    /// The new-agent kind under the cursor, if any.
    pub fn selected_kind(&self) -> Option<agentik_core::profile::AgentKind> {
        self.picker
            .selected_item()
            .and_then(|item| match &item.row {
                PopupRow::NewAgent(kind) => Some(*kind),
                PopupRow::Agent(_) => None,
            })
    }

    /// Live agents passing the current tab + query filter (bulk targets).
    pub fn filtered_agents(&self) -> Vec<AgentRow> {
        self.picker
            .filtered
            .iter()
            .filter_map(|&idx| self.picker.items.get(idx))
            .filter_map(|item| match &item.row {
                PopupRow::Agent(agent) if agent.live => Some(agent.clone()),
                _ => None,
            })
            .collect()
    }

    // ── Rename / delete inline confirmations ──

    fn start_rename(&mut self) {
        if let Some(agent) = self.selected_agent() {
            if let Ok(path) = agentik_types::AgentPath::try_from(agent.name.as_str()) {
                self.rename_id = Some(agent.agent_id);
                self.rename_path = Some(path);
                self.rename_input = agent
                    .name
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                self.rename_error = None;
                self.input_mode = InputMode::Rename;
            }
        }
    }

    /// Validate the rename input and, if valid, return `(agent_id,
    /// new_path)`. The new path replaces the last segment of the stored
    /// path; the segment must be a valid `AgentPath` component
    /// (lowercase `[a-z0-9_]`, 1–32 chars).
    fn check_rename(&mut self) -> Option<(uuid::Uuid, agentik_types::AgentPath)> {
        let id = self.rename_id?;
        let base = self.rename_path.clone()?;
        let new_name = self.rename_input.trim();
        if let Err(e) = agentik_types::validate_segment(new_name) {
            self.rename_error = Some(e.to_string());
            return None;
        }
        self.rename_error = None;
        let new_path = match base.parent() {
            Some(parent) => parent.join(new_name).ok()?,
            None => agentik_types::AgentPath::root().join(new_name).ok()?,
        };
        self.rename_id = None;
        self.rename_path = None;
        self.rename_input.clear();
        self.input_mode = InputMode::None;
        Some((id, new_path))
    }

    fn start_delete_confirm(&mut self) {
        if let Some(agent) = self.selected_agent() {
            self.delete_confirm_id = Some(agent.agent_id);
            self.delete_confirm_input.clear();
            self.input_mode = InputMode::DeleteConfirm;
        }
    }

    fn check_delete_confirm(&mut self) -> Option<uuid::Uuid> {
        if self.delete_confirm_input.trim().eq_ignore_ascii_case("yes") {
            let id = self.delete_confirm_id.take();
            self.delete_confirm_input.clear();
            self.input_mode = InputMode::None;
            id
        } else {
            None
        }
    }

    /// Nearest status tab in the requested direction that has rows (the
    /// current tab and All are always reachable).
    fn step_tab(&self, forward: bool) -> usize {
        let len = TABS.len();
        let count = |tab: usize| {
            self.all
                .iter()
                .filter(|item| match &item.row {
                    PopupRow::NewAgent(_) => tab == 0,
                    PopupRow::Agent(agent) => tab_matches(tab, agent),
                })
                .count()
        };
        let mut candidate = self.status_tab;
        for _ in 0..len {
            candidate = if forward {
                (candidate + 1) % len
            } else {
                (candidate + len - 1) % len
            };
            if candidate == 0 || count(candidate) > 0 || candidate == self.status_tab {
                return candidate;
            }
        }
        self.status_tab
    }

    /// Esc closes (clearing an active search first via the caller's flow),
    /// typing searches, Tab/←→ switch status tabs, ↑↓ move the
    /// cursor, Alt+h/l move the column cursor, Enter dispatches per column,
    /// Ctrl+R/Ctrl+D rename/delete the selected identity, and m/M/e/E are
    /// the Alt-modified quick-switch shortcuts.
    pub fn handle_key(&mut self, key: KeyEvent) -> AgentsPopupKeyOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match self.input_mode {
            InputMode::Rename => return self.handle_rename_key(key, ctrl),
            InputMode::DeleteConfirm => return self.handle_delete_key(key, ctrl),
            InputMode::None => {}
        }

        if ctrl {
            return match key.code {
                KeyCode::Char('u') => {
                    self.textarea.set_text("");
                    self.picker.set_query("");
                    AgentsPopupKeyOutcome::Consumed
                }
                KeyCode::Char('p') => {
                    self.picker.move_up();
                    AgentsPopupKeyOutcome::Consumed
                }
                KeyCode::Char('n') => {
                    self.picker.move_down();
                    AgentsPopupKeyOutcome::Consumed
                }
                KeyCode::Char('r') => {
                    self.start_rename();
                    AgentsPopupKeyOutcome::Consumed
                }
                KeyCode::Char('d') => {
                    self.start_delete_confirm();
                    AgentsPopupKeyOutcome::Consumed
                }
                _ => AgentsPopupKeyOutcome::Consumed,
            };
        }

        match key.code {
            KeyCode::Esc if !self.textarea.is_empty() => {
                self.textarea.set_text("");
                self.picker.set_query("");
                AgentsPopupKeyOutcome::Consumed
            }
            KeyCode::Esc => AgentsPopupKeyOutcome::Close,
            KeyCode::Tab | KeyCode::Right => {
                self.status_tab = self.step_tab(true);
                self.apply_tab();
                AgentsPopupKeyOutcome::Consumed
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.status_tab = self.step_tab(false);
                self.apply_tab();
                AgentsPopupKeyOutcome::Consumed
            }
            KeyCode::Up => {
                self.picker.move_up();
                AgentsPopupKeyOutcome::Consumed
            }
            KeyCode::Down => {
                self.picker.move_down();
                AgentsPopupKeyOutcome::Consumed
            }
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.column = match self.column {
                    AgentColumn::Name => AgentColumn::Name,
                    AgentColumn::Model => AgentColumn::Name,
                    AgentColumn::Effort => AgentColumn::Model,
                };
                AgentsPopupKeyOutcome::Consumed
            }
            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.column = match self.column {
                    AgentColumn::Name => AgentColumn::Model,
                    AgentColumn::Model => AgentColumn::Effort,
                    AgentColumn::Effort => AgentColumn::Effort,
                };
                AgentsPopupKeyOutcome::Consumed
            }
            KeyCode::Enter => {
                if let Some(kind) = self.selected_kind() {
                    return AgentsPopupKeyOutcome::NewAgent(kind);
                }
                match self.selected_agent() {
                    None => AgentsPopupKeyOutcome::Consumed,
                    Some(agent) => {
                        if !agent.live {
                            // Dormant: re-spawn from the stored config.
                            return match agentik_types::AgentPath::try_from(agent.name.as_str()) {
                                Ok(path) => AgentsPopupKeyOutcome::Resume {
                                    path,
                                    config_json: agent.config_json.clone(),
                                },
                                Err(_) => AgentsPopupKeyOutcome::Consumed,
                            };
                        }
                        match self.column {
                            AgentColumn::Name => AgentsPopupKeyOutcome::OpenOrSwitch {
                                name: agent.name.clone(),
                                agent_id: agent.agent_id,
                            },
                            AgentColumn::Model => AgentsPopupKeyOutcome::OpenModelPicker {
                                agent: agent.name.clone(),
                            },
                            AgentColumn::Effort => AgentsPopupKeyOutcome::CycleEffort {
                                agent: agent.name.clone(),
                            },
                        }
                    }
                }
            }
            KeyCode::Char('m') if key.modifiers.contains(KeyModifiers::ALT) => {
                match self.selected_agent() {
                    None | Some(AgentRow { live: false, .. }) => AgentsPopupKeyOutcome::Consumed,
                    Some(agent) => AgentsPopupKeyOutcome::OpenModelPicker {
                        agent: agent.name.clone(),
                    },
                }
            }
            KeyCode::Char('M') if key.modifiers.contains(KeyModifiers::ALT) => {
                AgentsPopupKeyOutcome::OpenModelPickerBulk {
                    agents: self
                        .filtered_agents()
                        .into_iter()
                        .map(|agent| agent.name)
                        .collect(),
                }
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::ALT) => {
                match self.selected_agent() {
                    None | Some(AgentRow { live: false, .. }) => AgentsPopupKeyOutcome::Consumed,
                    Some(agent) => AgentsPopupKeyOutcome::CycleEffort {
                        agent: agent.name.clone(),
                    },
                }
            }
            KeyCode::Char('E') if key.modifiers.contains(KeyModifiers::ALT) => {
                AgentsPopupKeyOutcome::OpenEffortPicker {
                    agents: self
                        .filtered_agents()
                        .into_iter()
                        .map(|agent| (agent.name, agent.effort.is_some()))
                        .collect(),
                }
            }
            KeyCode::Char(_)
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Home
            | KeyCode::End => {
                self.textarea.input(key);
                self.picker.set_query(self.textarea.text());
                AgentsPopupKeyOutcome::Consumed
            }
            _ => AgentsPopupKeyOutcome::Consumed,
        }
    }

    fn handle_rename_key(&mut self, key: KeyEvent, ctrl: bool) -> AgentsPopupKeyOutcome {
        if ctrl {
            return AgentsPopupKeyOutcome::Consumed;
        }
        match key.code {
            KeyCode::Esc => {
                self.rename_id = None;
                self.rename_path = None;
                self.rename_input.clear();
                self.rename_error = None;
                self.input_mode = InputMode::None;
            }
            KeyCode::Backspace => {
                self.rename_input.pop();
            }
            KeyCode::Enter => {
                if let Some((agent_id, new_path)) = self.check_rename() {
                    return AgentsPopupKeyOutcome::Rename { agent_id, new_path };
                }
            }
            KeyCode::Char(c) => self.rename_input.push(c),
            _ => {}
        }
        AgentsPopupKeyOutcome::Consumed
    }

    fn handle_delete_key(&mut self, key: KeyEvent, ctrl: bool) -> AgentsPopupKeyOutcome {
        if ctrl {
            return AgentsPopupKeyOutcome::Consumed;
        }
        match key.code {
            KeyCode::Esc => {
                self.delete_confirm_id = None;
                self.delete_confirm_input.clear();
                self.input_mode = InputMode::None;
            }
            KeyCode::Backspace => {
                self.delete_confirm_input.pop();
            }
            KeyCode::Enter => {
                // Only commit when the typed confirmation matches "yes"
                // (case-insensitive). Partial / empty / typo input is
                // silently ignored so the user can keep typing without
                // losing context. Esc cancels.
                if let Some(agent_id) = self.check_delete_confirm() {
                    return AgentsPopupKeyOutcome::Delete(agent_id);
                }
            }
            KeyCode::Char(c) => self.delete_confirm_input.push(c),
            _ => {}
        }
        AgentsPopupKeyOutcome::Consumed
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Render
// ═══════════════════════════════════════════════════════════════════════

/// Render the unified agents popup: search row, status-tab strip, agent
/// list (new-agent entries + identities), inline rename/delete line,
/// footer.
pub fn render_agents_popup(frame_area: Rect, buf: &mut Buffer, state: &mut AgentsPopupState) {
    if !state.picker.visible {
        return;
    }

    let theme = crate::theme::current();
    let popup_w = (frame_area.width * 7 / 10).max(46);
    let popup = Popup::new(" Agents ", PopupControls::default())
        .accent(theme.accent)
        .width(popup_w);
    let inner = popup.render(frame_area, buf);

    let v_regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // search / inline input
            Constraint::Length(1), // status tabs
            Constraint::Length(1), // separator
            Constraint::Min(5),    // agent list
            Constraint::Length(1), // footer
        ])
        .split(inner);

    render_input_line(v_regions[0], buf, state);
    render_status_tabs(v_regions[1], buf, state);
    Widget::render(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(theme.text_faint)),
        v_regions[2],
        buf,
    );
    render_list(v_regions[3], buf, state);
    render_footer(v_regions[4], buf, state);
}

/// The top input row: search box, rename draft, or delete confirmation —
/// whichever mode is active.
fn render_input_line(area: Rect, buf: &mut Buffer, state: &mut AgentsPopupState) {
    let theme = crate::theme::current();
    match state.input_mode {
        InputMode::Rename => {
            let line = if let Some(err) = state.rename_error.clone() {
                Line::from(vec![
                    Span::styled(" x ", Style::default().fg(theme.error)),
                    Span::styled(err, Style::default().fg(theme.error)),
                ])
            } else {
                Line::from(vec![
                    Span::styled(" New name: ", Style::default().fg(theme.accent)),
                    Span::styled(state.rename_input.clone(), Style::default().fg(theme.text)),
                    Span::styled(
                        "▏ Enter 确认 · Esc 取消",
                        Style::default().fg(theme.text_faint),
                    ),
                ])
            };
            Paragraph::new(line).render(area, buf);
            state.cursor_pos = None;
        }
        InputMode::DeleteConfirm => {
            let name = state
                .selected_agent()
                .map(|agent| agent.name.clone())
                .unwrap_or_default();
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" 删除 {name}? 输入 yes 确认: "),
                    Style::default().fg(theme.error),
                ),
                Span::styled(
                    state.delete_confirm_input.clone(),
                    Style::default().fg(theme.text),
                ),
                Span::styled("▏", Style::default().fg(theme.error)),
            ]))
            .render(area, buf);
            state.cursor_pos = None;
        }
        _ => render_search_input(area, buf, state),
    }
}

fn render_search_input(area: Rect, buf: &mut Buffer, state: &mut AgentsPopupState) {
    let theme = crate::theme::current();
    let searching = true;
    let prompt_style = if searching || !state.textarea.is_empty() {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_faint)
    };
    buf.set_string(area.x, area.y, ">", prompt_style);
    let ta_area = match area.width.checked_sub(PROMPT_GUTTER) {
        Some(width) => Rect {
            x: area.x + PROMPT_GUTTER,
            width,
            ..area
        },
        None => {
            state.cursor_pos = None;
            return;
        }
    };

    if state.textarea.is_empty() {
        {
            buf.set_string(
                ta_area.x,
                ta_area.y,
                " Search agents (names, model specs)",
                Style::default().fg(theme.text_faint),
            );
        }
        state.cursor_pos = searching.then_some((ta_area.x, ta_area.y));
        return;
    }

    let textarea: &TextArea = &state.textarea;
    let ta_state: &mut TextAreaState = &mut state.textarea_state;
    textarea.render_ref(ta_area, buf, ta_state);
    state.cursor_pos = searching
        .then(|| textarea.cursor_pos_with_state(ta_area, state.textarea_state))
        .flatten();
}

/// One-row status-tab strip: `●3 ◌2 *1 …` with the selected tab expanded
/// to name its glyph and empty tabs folded out.
fn render_status_tabs(area: Rect, buf: &mut Buffer, state: &AgentsPopupState) {
    if area.width < 4 {
        return;
    }
    let theme = crate::theme::current();
    let selected_style = Style::default()
        .fg(theme.on_accent)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    let mut used = 1usize;
    let viewport = area.width as usize;
    for (tab, (name, filter)) in TABS.iter().enumerate() {
        let count = state
            .all
            .iter()
            .filter(|item| match &item.row {
                PopupRow::NewAgent(_) => tab == 0,
                PopupRow::Agent(agent) => tab_matches(tab, agent),
            })
            .count();
        let selected = tab == state.status_tab;
        // Empty statuses fold; the selected tab and All never do.
        let is_status_tab = !matches!(filter, TabFilter::All);
        if count == 0 && is_status_tab && !selected {
            continue;
        }
        let text_style = if selected {
            selected_style
        } else {
            Style::default().fg(theme.text_dim)
        };
        let glyph_style = match tab_glyph(filter) {
            Some((_, Some(status))) if !selected => Style::default().fg(status_color(&status)),
            _ if !selected => Style::default().fg(theme.text_dim),
            _ => selected_style,
        };
        let mut pieces: Vec<(String, Style)> = Vec::new();
        if let Some((glyph, _)) = tab_glyph(filter) {
            pieces.push((glyph.into(), glyph_style));
        }
        if selected {
            pieces.push((" ".into(), selected_style));
            pieces.push(((*name).into(), selected_style));
            pieces.push((" ".into(), selected_style));
        }
        pieces.push((count.to_string(), text_style));
        pieces.push(("  ".into(), Style::default().fg(theme.text_faint)));
        let width: usize = pieces.iter().map(|(piece, _)| piece.width()).sum();
        if used + width > viewport {
            break;
        }
        used += width;
        spans.extend(
            pieces
                .into_iter()
                .map(|(piece, style)| Span::styled(piece, style)),
        );
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

/// Agent rows: status glyph + name (flex, bold on the active leaf) with
/// trailing model and effort columns that drop out on narrow popups.
/// New-agent entries render with a `＋` marker. The visible window
/// scrolls around the selection.
fn render_list(area: Rect, buf: &mut Buffer, state: &mut AgentsPopupState) {
    let theme = crate::theme::current();
    if state.picker.items.is_empty() {
        let hint = if state.picker.query.is_empty() {
            "(no agents match this filter)"
        } else {
            "No matching agents"
        };
        Paragraph::new(hint)
            .style(Style::default().fg(theme.text_faint))
            .centered()
            .render(area, buf);
        return;
    }

    let rows = state.picker.filtered.len();
    let selected = state.picker.selected.min(rows.saturating_sub(1));
    let height = area.height as usize;

    // Scroll window: keep the selection visible, sticky at the top until
    // the tail enters view.
    let start = if rows <= height {
        0
    } else {
        selected
            .saturating_sub(height.saturating_sub(2))
            .min(rows - height)
    };

    // Column budgets: the name flexes; model/effort are fixed and drop out
    // (effort first) on narrow popups.
    let avail = area.width.saturating_sub(2) as usize; // glyph + space prefix
    let (name_w, model_w, effort_w) = if avail >= (MODEL_COL + 1 + EFFORT_COL + 1 + 6) as usize {
        (
            avail - (MODEL_COL + 1 + EFFORT_COL + 1) as usize,
            Some(MODEL_COL as usize),
            Some(EFFORT_COL as usize),
        )
    } else if avail >= (MODEL_COL + 1 + 3) as usize {
        (
            avail - (MODEL_COL + 1) as usize,
            Some(MODEL_COL as usize),
            None,
        )
    } else {
        (avail, None, None)
    };

    let mut y = area.y;
    for (index, item_index) in state.picker.filtered.iter().enumerate().skip(start) {
        if y >= area.bottom() || index >= start + height {
            break;
        }
        let is_selected = index == selected;
        let line = match &state.picker.items[*item_index].row {
            PopupRow::NewAgent(kind) => new_agent_line(kind, is_selected, name_w),
            PopupRow::Agent(agent) => {
                agent_row_line(agent, is_selected, state.column, name_w, model_w, effort_w)
            }
        };
        Paragraph::new(line)
            .style(if is_selected {
                Style::default().bg(theme.selected_bg)
            } else {
                Style::default()
            })
            .render(
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        y += 1;
    }

    let hidden = rows.saturating_sub(start + height);
    if hidden > 0 && y < area.bottom() {
        Paragraph::new(Line::from(Span::styled(
            format!("  +{hidden} more"),
            Style::default()
                .fg(theme.text_faint)
                .add_modifier(Modifier::DIM),
        )))
        .render(
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
            buf,
        );
    }
}

/// One new-agent row: `＋ <kind>` in the accent color.
fn new_agent_line(
    kind: &agentik_core::profile::AgentKind,
    selected: bool,
    name_w: usize,
) -> Line<'static> {
    let theme = crate::theme::current();
    let style = if selected {
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.accent)
    };
    let name = format!("new {}", kind.name());
    Line::from(vec![
        Span::styled("＋", style),
        Span::raw(" "),
        Span::styled(truncate_str(&name, name_w), style),
    ])
}

/// One identity row. Mirrors the column budget of `render_list`.
fn agent_row_line(
    agent: &AgentRow,
    selected: bool,
    column: AgentColumn,
    name_w: usize,
    model_w: Option<usize>,
    effort_w: Option<usize>,
) -> Line<'static> {
    let theme = crate::theme::current();
    let cell_style = if selected {
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_faint)
    };
    let (icon, icon_style, name_style) = if selected {
        (
            status_icon(&agent.status),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )
    } else if !agent.live {
        (
            DORMANT_GLYPH,
            Style::default().fg(theme.text_faint),
            Style::default().fg(theme.text_dim),
        )
    } else {
        (
            status_icon(&agent.status),
            Style::default().fg(status_color(&agent.status)),
            if agent.is_active {
                Style::default()
                    .fg(theme.active_marker)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_dim)
            },
        )
    };

    let name_text = truncate_str(agent.shown_name(), name_w);
    let mut spans = vec![
        Span::styled(icon, icon_style),
        Span::raw(" "),
        Span::styled(name_text, name_style),
    ];
    spans.push(Span::raw(
        " ".repeat(name_w.saturating_sub(agent.shown_name().chars().count())),
    ));

    // Open-leaf marker: a dot before the model column flags identities
    // with an existing view window.
    if agent.is_open && !selected {
        spans.push(Span::styled("·", Style::default().fg(theme.text_faint)));
    } else {
        spans.push(Span::raw(" "));
    }

    if let Some(model_w) = model_w {
        let model_text = model_cell_text(agent.model.as_deref(), model_w);
        let model_style = if selected && column == AgentColumn::Model {
            cell_style.add_modifier(Modifier::UNDERLINED)
        } else {
            cell_style
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(model_text, model_style));
    }
    if effort_w.is_some() {
        let effort_text = match (&agent.model, &agent.effort) {
            (Some(_), None) => "–".to_string(),
            (Some(_), Some(effort)) => truncate_str(effort, EFFORT_COL as usize),
            (None, _) => String::new(),
        };
        let effort_style = if selected && column == AgentColumn::Effort {
            cell_style.add_modifier(Modifier::UNDERLINED)
        } else {
            cell_style
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(effort_text, effort_style));
    }
    Line::from(spans)
}

/// Fit a model spec into the model column: verbatim when it fits, else the
/// `model` part alone (dropping the provider before the **first** colon —
/// ollama tags like `qwen3:32b` carry colons of their own), else that part
/// truncated.
fn model_cell_text(spec: Option<&str>, width: usize) -> String {
    let Some(spec) = spec else {
        return String::new();
    };
    if spec.chars().count() <= width {
        return spec.to_string();
    }
    let tail = spec.split_once(':').map(|(_, model)| model).unwrap_or(spec);
    truncate_str(tail, width)
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else if max_len == 0 {
        String::new()
    } else {
        let truncated: String = s.chars().take(max_len - 1).collect();
        format!("{truncated}…")
    }
}

/// Footer hints; the rename/delete modes replace them with their own
/// confirm/remind line (the input row already shows the flow).
fn render_footer(area: Rect, buf: &mut Buffer, state: &AgentsPopupState) {
    let theme = crate::theme::current();
    let pairs: Vec<(&str, &str)> = match state.input_mode {
        InputMode::Rename => vec![("Enter", " 确认改名  "), ("Esc", " 取消")],
        InputMode::DeleteConfirm => vec![("yes+Enter", " 确认删除  "), ("Esc", " 取消")],
        _ => vec![
            ("Enter", " 打开/恢复  "),
            ("Alt+m/M", " 模型  "),
            ("Alt+e/E", " 力度  "),
            ("Tab", " 过滤  "),
            ("^R", " 改名  "),
            ("^D", " 删除  "),
            ("Esc", " 关闭"),
        ],
    };
    let mut spans = Vec::new();
    for (key, label) in pairs {
        spans.push(Span::styled(
            key,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(label, Style::default().fg(theme.text_faint)));
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_row(name: &str, status: AgentStatus) -> AgentRow {
        AgentRow {
            name: name.to_string(),
            agent_id: uuid::Uuid::new_v4(),
            status,
            live: true,
            is_active: false,
            is_open: false,
            model: None,
            effort: None,
            config_json: serde_json::json!({"kind": "researcher"}),
        }
    }

    fn dormant_row(name: &str) -> AgentRow {
        AgentRow {
            live: false,
            status: AgentStatus::Idle,
            ..live_row(name, AgentStatus::Idle)
        }
    }

    fn key(state: &mut AgentsPopupState, code: KeyCode) -> AgentsPopupKeyOutcome {
        state.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl_key(state: &mut AgentsPopupState, c: char) -> AgentsPopupKeyOutcome {
        state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn alt_key(state: &mut AgentsPopupState, c: char) -> AgentsPopupKeyOutcome {
        state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
    }

    #[test]
    fn search_matches_names_and_model_specs() {
        let mut state = AgentsPopupState::default();
        let mut on_glm = live_row("/root/mrine", AgentStatus::Idle);
        on_glm.model = Some("zai:glm-5.3".into());
        state.open_with(vec![
            on_glm.clone(),
            live_row("/root/other", AgentStatus::Idle),
        ]);

        for c in "glm".chars() {
            key(&mut state, KeyCode::Char(c));
        }
        let filtered = state.filtered_agents();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "/root/mrine");

        // Ctrl+U clears the query.
        ctrl_key(&mut state, 'u');
        assert_eq!(state.filtered_agents().len(), 2);
        for c in "mrn".chars() {
            key(&mut state, KeyCode::Char(c));
        }
        assert_eq!(state.filtered_agents()[0].name, "/root/mrine");
        assert_eq!(state.filtered_agents().len(), 1);
        ctrl_key(&mut state, 'u');
        for c in "enirm".chars() {
            key(&mut state, KeyCode::Char(c));
        }
        assert!(state.filtered_agents().is_empty());
    }

    #[test]
    fn status_tabs_filter_and_skip_empty() {
        let mut state = AgentsPopupState::default();
        state.open_with(vec![
            live_row("/root/idle1", AgentStatus::Idle),
            live_row("/root/busy", AgentStatus::Streaming),
        ]);
        // Right steps to the first tab with rows: Dormant (empty, skipped)
        // → Idle.
        key(&mut state, KeyCode::Right);
        let filtered = state.filtered_agents();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "/root/idle1");
        // Another right skips empty tabs to Streaming.
        key(&mut state, KeyCode::Right);
        assert_eq!(state.filtered_agents()[0].name, "/root/busy");
    }

    #[test]
    fn dormant_tab_lists_only_dormant_identities() {
        let mut state = AgentsPopupState::default();
        state.open_with(vec![
            live_row("/root/live", AgentStatus::Idle),
            dormant_row("/root/sleepy"),
        ]);
        key(&mut state, KeyCode::Right); // Dormant
        let filtered = state.filtered_agents();
        assert!(filtered.is_empty(), "live rows never match Dormant");
        // The Dormant tab keeps its rows visible.
        let items = state.picker.items.len();
        assert_eq!(items, 1, "one dormant identity row");
    }

    #[test]
    fn enter_on_dormant_row_resumes() {
        let mut state = AgentsPopupState::default();
        state.open_with(vec![dormant_row("/root/sleepy")]);
        // New-agent entries precede identity rows — move past them.
        while state.selected_kind().is_some() {
            key(&mut state, KeyCode::Down);
        }
        match key(&mut state, KeyCode::Enter) {
            AgentsPopupKeyOutcome::Resume { path, .. } => {
                assert_eq!(path.as_str(), "/root/sleepy");
            }
            other => panic!("expected Resume, got {other:?}"),
        }
    }

    #[test]
    fn enter_on_new_agent_entry_starts_naming() {
        let mut state = AgentsPopupState::default();
        state.open_with(vec![live_row("/root/a", AgentStatus::Idle)]);
        // Cursor starts on the first new-agent entry.
        match key(&mut state, KeyCode::Enter) {
            AgentsPopupKeyOutcome::NewAgent(kind) => {
                assert_eq!(kind, agentik_core::profile::AgentKind::ALL[0]);
            }
            other => panic!("expected NewAgent, got {other:?}"),
        }
    }

    #[test]
    fn rename_flow_validates_and_commits() {
        let mut state = AgentsPopupState::default();
        state.open_with(vec![dormant_row("/root/researcher")]);
        while state.selected_kind().is_some() {
            key(&mut state, KeyCode::Down);
        }
        ctrl_key(&mut state, 'r');
        // The draft is pre-filled with the current leaf name — clear it.
        for _ in 0.."researcher".chars().count() {
            key(&mut state, KeyCode::Backspace);
        }
        // Invalid segment keeps the mode open.
        for c in "Bad Name!".chars() {
            key(&mut state, KeyCode::Char(c));
        }
        assert!(matches!(
            key(&mut state, KeyCode::Enter),
            AgentsPopupKeyOutcome::Consumed
        ));
        // Clear (Backspace per char) and type a valid name.
        for _ in 0.."Bad Name!".chars().count() {
            key(&mut state, KeyCode::Backspace);
        }
        for c in "writer".chars() {
            key(&mut state, KeyCode::Char(c));
        }
        match key(&mut state, KeyCode::Enter) {
            AgentsPopupKeyOutcome::Rename { new_path, .. } => {
                assert_eq!(new_path.as_str(), "/root/writer");
            }
            other => panic!("expected Rename, got {other:?}"),
        }
    }

    #[test]
    fn delete_requires_typed_yes() {
        let mut state = AgentsPopupState::default();
        let row = dormant_row("/root/doomed");
        let id = row.agent_id;
        state.open_with(vec![row]);
        while state.selected_kind().is_some() {
            key(&mut state, KeyCode::Down);
        }
        ctrl_key(&mut state, 'd');
        for c in "no".chars() {
            key(&mut state, KeyCode::Char(c));
        }
        assert!(matches!(
            key(&mut state, KeyCode::Enter),
            AgentsPopupKeyOutcome::Consumed
        ));
        for c in "yes".chars() {
            key(&mut state, KeyCode::Char(c));
        }
        // The "no" prefix is still there ("noyes") — clear it first.
        for _ in 0..5 {
            key(&mut state, KeyCode::Backspace);
        }
        for c in "yes".chars() {
            key(&mut state, KeyCode::Char(c));
        }
        assert_eq!(
            key(&mut state, KeyCode::Enter),
            AgentsPopupKeyOutcome::Delete(id)
        );
    }

    #[test]
    fn column_cursor_changes_enter_dispatch() {
        let mut state = AgentsPopupState::default();
        let mut active = live_row("/root/a", AgentStatus::Idle);
        active.is_active = true;
        state.open_with(vec![active.clone()]);
        while state.selected_kind().is_some() {
            key(&mut state, KeyCode::Down);
        }
        assert!(matches!(
            key(&mut state, KeyCode::Enter),
            AgentsPopupKeyOutcome::OpenOrSwitch { name, agent_id }
                if name == "/root/a" && agent_id == active.agent_id
        ));
        alt_key(&mut state, 'l');
        assert!(matches!(
            key(&mut state, KeyCode::Enter),
            AgentsPopupKeyOutcome::OpenModelPicker { agent } if agent == "/root/a"
        ));
        alt_key(&mut state, 'l');
        assert!(matches!(
            key(&mut state, KeyCode::Enter),
            AgentsPopupKeyOutcome::CycleEffort { agent } if agent == "/root/a"
        ));
        // Sticky at the last column.
        alt_key(&mut state, 'l');
        assert_eq!(state.column, AgentColumn::Effort);
    }

    #[test]
    fn model_shortcuts_ignore_dormant_rows() {
        let mut state = AgentsPopupState::default();
        state.open_with(vec![dormant_row("/root/sleepy")]);
        while state.selected_kind().is_some() {
            key(&mut state, KeyCode::Down);
        }
        assert!(matches!(
            alt_key(&mut state, 'm'),
            AgentsPopupKeyOutcome::Consumed
        ));
        assert!(matches!(
            alt_key(&mut state, 'e'),
            AgentsPopupKeyOutcome::Consumed
        ));
    }

    #[test]
    fn bulk_shortcuts_target_the_filtered_set() {
        let mut state = AgentsPopupState::default();
        state.open_with(vec![
            live_row("/root/a", AgentStatus::Idle),
            live_row("/root/b", AgentStatus::Error),
        ]);
        while state.selected_kind().is_some() {
            key(&mut state, KeyCode::Down);
        }
        // Tab to Error: only b remains (live rows only). Empty tabs
        // (Dormant, Waiting, …, Compacting) are skipped on the way.
        for _ in 0..8 {
            key(&mut state, KeyCode::Right);
        }
        assert!(matches!(
            alt_key(&mut state, 'M'),
            AgentsPopupKeyOutcome::OpenModelPickerBulk { agents } if agents == vec!["/root/b".to_string()]
        ));
    }

    #[test]
    fn refresh_keeps_query_and_tab() {
        let mut state = AgentsPopupState::default();
        state.open_with(vec![live_row("/root/a", AgentStatus::Streaming)]);
        for c in "root".chars() {
            key(&mut state, KeyCode::Char(c));
        }
        // Step to the Dormant tab while preserving the query.
        key(&mut state, KeyCode::Right); // Dormant tab
        state.refresh_with(vec![
            live_row("/root/a", AgentStatus::Streaming),
            dormant_row("/root/sleepy"),
        ]);
        assert_eq!(state.query(), "root", "query survives refresh");
        // Dormant tab still selected; the sleepy row is now visible
        // despite the query (path contains "root").
        assert_eq!(state.picker.items.len(), 1);
    }

    #[test]
    fn model_cell_drops_provider_before_the_first_colon() {
        assert_eq!(model_cell_text(Some("zai:glm-5.3"), 6), "glm-5…");
        assert_eq!(model_cell_text(Some("short"), 8), "short");
        assert_eq!(model_cell_text(None, 8), "");
    }
}
