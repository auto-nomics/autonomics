//! Agents popup (`Ctrl+T`): a picker-style switcher over every running
//! agent, built on [`PickerState`] like the other popups.
//!
//! Rows carry the status glyph, the workspace-root-stripped name (bold red
//! on the active leaf), and trailing model / effort columns. A status-tab
//! strip (glyph + count) filters by lifecycle state; `/` focuses the
//! search input (names and model specs match); the `h`/`l` column cursor
//! picks what Enter does on the selected row — switch leaf, open the model
//! quick-picker, or cycle the effort override. `m`/`M`/`e`/`E` are the
//! direct shortcuts (the capital forms fan out over the filtered set via
//! the shared y/n confirmation).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, StatefulWidgetRef, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::state::AgentStatus;
use crate::widgets::agent_workspace::{display_name, status_color, status_icon};
use crate::widgets::popup::{Popup, PopupControls};
use crate::widgets::searchable_picker::{PickerItem, PickerState};
use crate::xai_textarea::{TextArea, TextAreaState};

const ACCENT: Color = Color::Cyan;
/// Prompt gutter width for the search input: `❯ ` = 2 cols.
const PROMPT_GUTTER: u16 = 2;

/// Fixed widths of the trailing columns. The effort column fits the longest
/// common effort id (`medium`).
const MODEL_COL: u16 = 12;
const EFFORT_COL: u16 = 6;

// ═══════════════════════════════════════════════════════════════════════
// Data
// ═══════════════════════════════════════════════════════════════════════

/// One running agent entry in the popup, supplied by the app when the
/// popup opens (a snapshot — statuses are live only for that moment).
#[derive(Clone, Debug)]
pub struct RunningAgent {
    pub name: String,
    pub agent_id: uuid::Uuid,
    pub status: AgentStatus,
    pub is_active: bool,
    /// The agent's live model spec (`provider:model`) from the render cache;
    /// `None` while the per-agent fetch is still in flight (column blank).
    pub model: Option<String>,
    /// The reasoning effort this agent's next request would send; `None`
    /// on non-thinking models (column `–`) and alongside a `None` model.
    pub effort: Option<String>,
}

impl RunningAgent {
    /// The text shown for this agent: the workspace-root-stripped path.
    pub fn shown_name(&self) -> &str {
        display_name(&self.name)
    }
}

/// Order running agents shallow-path-first, then lexically — a stable order
/// across opens.
pub fn sort_running_agents(agents: &mut [RunningAgent]) {
    agents.sort_by(|left, right| {
        let depth = |name: &str| name.split('/').filter(|part| !part.is_empty()).count();
        depth(&left.name)
            .cmp(&depth(&right.name))
            .then_with(|| left.name.cmp(&right.name))
    });
}

/// Picker item wrapping a [`RunningAgent`] with a pre-built search blob
/// (path + display name + model spec), so `PickerState`'s query filter
/// matches everything the user would reasonably type.
#[derive(Clone, Debug)]
pub struct AgentsPopupItem {
    pub agent: RunningAgent,
    search: String,
}

impl AgentsPopupItem {
    fn new(agent: RunningAgent) -> Self {
        let mut search = format!("{} {} ", agent.name, display_name(&agent.name),);
        if let Some(model) = &agent.model {
            search.push_str(model);
        }
        Self { agent, search }
    }
}

impl PickerItem for AgentsPopupItem {
    fn title(&self) -> &str {
        self.agent.shown_name()
    }
    fn keywords(&self) -> &str {
        &self.search
    }
    fn category(&self) -> Option<&str> {
        self.agent.model.as_deref()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Status tabs
// ═══════════════════════════════════════════════════════════════════════

const TABS: [(&str, Option<AgentStatus>); 10] = [
    ("All", None),
    ("Idle", Some(AgentStatus::Idle)),
    ("Waiting", Some(AgentStatus::Waiting)),
    ("Requesting", Some(AgentStatus::Requesting)),
    ("Streaming", Some(AgentStatus::Streaming)),
    ("Tools", Some(AgentStatus::ToolRunning)),
    ("Retrying", Some(AgentStatus::Retrying)),
    ("Compacting", Some(AgentStatus::Compacting)),
    ("Error", Some(AgentStatus::Error)),
    ("Cancelled", Some(AgentStatus::Cancelled)),
];

fn tab_matches(tab: usize, status: AgentStatus) -> bool {
    match TABS[tab].1 {
        None => true,
        Some(AgentStatus::Idle) => status.is_idle(),
        Some(expected) => status == expected,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Column cursor
// ═══════════════════════════════════════════════════════════════════════

/// The row column the cursor acts on: `h`/`l` move it, Enter dispatches
/// per column (name switches, model opens the picker, effort cycles).
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsPopupKeyOutcome {
    /// The key was consumed but no action is needed.
    Consumed,
    /// Switch the active leaf to this agent and close the popup.
    Switch(uuid::Uuid),
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

/// State for the agents popup.
pub struct AgentsPopupState {
    /// Item list + query filtering + list navigation.
    pub picker: PickerState<AgentsPopupItem>,
    /// Every running agent (pre-tab-filter), for tab counts and rebuilds.
    all: Vec<AgentsPopupItem>,
    status_tab: usize,
    /// The row column cursor (`h`/`l` move it; Enter dispatches per
    /// column).
    pub column: AgentColumn,
    /// Search input editor; only captures typing while `searching`.
    textarea: TextArea,
    textarea_state: TextAreaState,
    searching: bool,
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
            searching: false,
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
        self.textarea.set_text("");
        self.textarea.clear_history();
        self.cursor_pos = None;
        self.searching = false;
    }

    /// Open with a snapshot of the running agents (shallow-path-first
    /// order is the caller's; this only wraps them).
    pub fn open_with(&mut self, agents: Vec<RunningAgent>) {
        self.all = agents.into_iter().map(AgentsPopupItem::new).collect();
        self.status_tab = 0;
        self.column = AgentColumn::Name;
        self.textarea.set_text("");
        self.textarea.clear_history();
        self.searching = false;
        self.apply_tab();
        self.picker.open();
    }

    /// Rebuild the picker's item set from the tab filter, keeping the
    /// query; selection clamps to the new list.
    fn apply_tab(&mut self) {
        let tab = self.status_tab;
        let items: Vec<AgentsPopupItem> = self
            .all
            .iter()
            .filter(|item| tab_matches(tab, item.agent.status))
            .cloned()
            .collect();
        let query = self.picker.query.clone();
        self.picker.set_items(items);
        self.picker.set_query(&query);
    }

    /// The row the cursor sits on.
    pub fn selected_agent(&self) -> Option<RunningAgent> {
        self.picker.selected_item().map(|item| item.agent.clone())
    }

    /// Agents passing the current tab + query filter (bulk targets).
    pub fn filtered_agents(&self) -> Vec<RunningAgent> {
        self.picker
            .filtered
            .iter()
            .filter_map(|&idx| self.picker.items.get(idx))
            .map(|item| item.agent.clone())
            .collect()
    }

    /// Nearest status tab in the requested direction that has agents (the
    /// current tab and All are always reachable).
    fn step_tab(&self, forward: bool) -> usize {
        let len = TABS.len();
        let count = |tab: usize| {
            self.all
                .iter()
                .filter(|item| tab_matches(tab, item.agent.status))
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
    /// `/` focuses search, Tab/←→ switch status tabs, ↑↓/j/k move the
    /// cursor, h/l move the column cursor, Enter dispatches per column,
    /// and m/M/e/E are the quick-switch shortcuts. Plain characters go to
    /// the search input only while searching.
    pub fn handle_key(&mut self, key: KeyEvent) -> AgentsPopupKeyOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if self.searching {
            if ctrl {
                match key.code {
                    KeyCode::Char('u') => {
                        self.textarea.set_text("");
                        self.picker.set_query("");
                        return AgentsPopupKeyOutcome::Consumed;
                    }
                    _ => return AgentsPopupKeyOutcome::Consumed,
                }
            }
            match key.code {
                KeyCode::Esc => {
                    if !self.textarea.is_empty() {
                        self.textarea.set_text("");
                        self.picker.set_query("");
                    } else {
                        self.searching = false;
                    }
                    return AgentsPopupKeyOutcome::Consumed;
                }
                KeyCode::Enter | KeyCode::Tab => {
                    self.searching = false;
                    return AgentsPopupKeyOutcome::Consumed;
                }
                KeyCode::Up => {
                    self.picker.move_up();
                    return AgentsPopupKeyOutcome::Consumed;
                }
                KeyCode::Down => {
                    self.picker.move_down();
                    return AgentsPopupKeyOutcome::Consumed;
                }
                _ => {}
            }
            self.textarea.input(key);
            self.picker.set_query(self.textarea.text());
            return AgentsPopupKeyOutcome::Consumed;
        }

        if ctrl {
            return match key.code {
                KeyCode::Char('p') => {
                    self.picker.move_up();
                    AgentsPopupKeyOutcome::Consumed
                }
                KeyCode::Char('n') => {
                    self.picker.move_down();
                    AgentsPopupKeyOutcome::Consumed
                }
                _ => AgentsPopupKeyOutcome::Consumed,
            };
        }

        match key.code {
            KeyCode::Esc => AgentsPopupKeyOutcome::Close,
            KeyCode::Char('/') => {
                self.searching = true;
                AgentsPopupKeyOutcome::Consumed
            }
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
            KeyCode::Up | KeyCode::Char('k') => {
                self.picker.move_up();
                AgentsPopupKeyOutcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.picker.move_down();
                AgentsPopupKeyOutcome::Consumed
            }
            KeyCode::Char('h') => {
                self.column = match self.column {
                    AgentColumn::Name => AgentColumn::Name,
                    AgentColumn::Model => AgentColumn::Name,
                    AgentColumn::Effort => AgentColumn::Model,
                };
                AgentsPopupKeyOutcome::Consumed
            }
            KeyCode::Char('l') => {
                self.column = match self.column {
                    AgentColumn::Name => AgentColumn::Model,
                    AgentColumn::Model => AgentColumn::Effort,
                    AgentColumn::Effort => AgentColumn::Effort,
                };
                AgentsPopupKeyOutcome::Consumed
            }
            KeyCode::Enter => match self.selected_agent() {
                None => AgentsPopupKeyOutcome::Consumed,
                Some(agent) => match self.column {
                    AgentColumn::Name => AgentsPopupKeyOutcome::Switch(agent.agent_id),
                    AgentColumn::Model => AgentsPopupKeyOutcome::OpenModelPicker {
                        agent: agent.name.clone(),
                    },
                    AgentColumn::Effort => AgentsPopupKeyOutcome::CycleEffort {
                        agent: agent.name.clone(),
                    },
                },
            },
            KeyCode::Char('m') => match self.selected_agent() {
                None => AgentsPopupKeyOutcome::Consumed,
                Some(agent) => AgentsPopupKeyOutcome::OpenModelPicker {
                    agent: agent.name.clone(),
                },
            },
            KeyCode::Char('M') => AgentsPopupKeyOutcome::OpenModelPickerBulk {
                agents: self
                    .filtered_agents()
                    .into_iter()
                    .map(|agent| agent.name)
                    .collect(),
            },
            KeyCode::Char('e') => match self.selected_agent() {
                None => AgentsPopupKeyOutcome::Consumed,
                Some(agent) => AgentsPopupKeyOutcome::CycleEffort {
                    agent: agent.name.clone(),
                },
            },
            KeyCode::Char('E') => AgentsPopupKeyOutcome::OpenEffortPicker {
                agents: self
                    .filtered_agents()
                    .into_iter()
                    .map(|agent| (agent.name, agent.effort.is_some()))
                    .collect(),
            },
            _ => AgentsPopupKeyOutcome::Consumed,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Render
// ═══════════════════════════════════════════════════════════════════════

/// Render the agents popup: search row, status-tab strip, three-column
/// agent list, footer.
pub fn render_agents_popup(frame_area: Rect, buf: &mut Buffer, state: &mut AgentsPopupState) {
    if !state.picker.visible {
        return;
    }

    let popup_w = (frame_area.width * 7 / 10).max(46);
    let popup = Popup::new(" Agents ", PopupControls::default())
        .accent(ACCENT)
        .width(popup_w);
    let inner = popup.render(frame_area, buf);

    let v_regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // search input
            Constraint::Length(1), // status tabs
            Constraint::Length(1), // separator
            Constraint::Min(5),    // agent list
            Constraint::Length(1), // footer
        ])
        .split(inner);

    render_search_input(v_regions[0], buf, state);
    render_status_tabs(v_regions[1], buf, state);
    Widget::render(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
        v_regions[2],
        buf,
    );
    render_list(v_regions[3], buf, state);

    let footer = Line::from(vec![
        Span::styled(
            " Enter",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" switch  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "m/M",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" model  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "e/E",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" effort  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "/",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" search  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "Tab",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" filter  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "Esc",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" close", Style::default().fg(Color::DarkGray)),
    ]);
    Widget::render(Paragraph::new(footer), v_regions[4], buf);
}

fn render_search_input(area: Rect, buf: &mut Buffer, state: &mut AgentsPopupState) {
    let prompt_style = if state.searching || !state.textarea.is_empty() {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    buf.set_string(area.x, area.y, "❯", prompt_style);
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
        if !state.searching {
            buf.set_string(
                ta_area.x,
                ta_area.y,
                " / search agents (names, model specs)",
                Style::default().fg(Color::DarkGray),
            );
        }
        state.cursor_pos = state.searching.then_some((ta_area.x, ta_area.y));
        return;
    }

    let textarea: &TextArea = &state.textarea;
    let ta_state: &mut TextAreaState = &mut state.textarea_state;
    textarea.render_ref(ta_area, buf, ta_state);
    state.cursor_pos = state
        .searching
        .then(|| textarea.cursor_pos_with_state(ta_area, state.textarea_state))
        .flatten();
}

/// One-row status-tab strip: `●3 ◐1 ⚙2 …` with the selected tab expanded
/// to name its glyph and empty tabs folded out.
fn render_status_tabs(area: Rect, buf: &mut Buffer, state: &AgentsPopupState) {
    if area.width < 4 {
        return;
    }
    let selected_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    let mut used = 1usize;
    let viewport = area.width as usize;
    for (tab, (name, status)) in TABS.iter().enumerate() {
        let count = state
            .all
            .iter()
            .filter(|item| tab_matches(tab, item.agent.status))
            .count();
        let selected = tab == state.status_tab;
        // Empty statuses fold; the selected tab and All never do.
        if count == 0 && status.is_some() && !selected {
            continue;
        }
        let text_style = if selected {
            selected_style
        } else {
            Style::default().fg(Color::Gray)
        };
        let glyph_style = match status {
            Some(status) if !selected => Style::default().fg(status_color(status)),
            _ => selected_style,
        };
        let mut pieces: Vec<(String, Style)> = Vec::new();
        if let Some(status) = status {
            pieces.push((status_icon(status).into(), glyph_style));
        }
        if selected {
            pieces.push((" ".into(), selected_style));
            pieces.push(((*name).into(), selected_style));
            pieces.push((" ".into(), selected_style));
        }
        pieces.push((count.to_string(), text_style));
        pieces.push(("  ".into(), Style::default().fg(Color::DarkGray)));
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

/// Three-column agent rows: status glyph + name (flex, bold red on the
/// active leaf) with trailing model and effort columns that drop out on
/// narrow popups. The visible window scrolls around the selection.
fn render_list(area: Rect, buf: &mut Buffer, state: &mut AgentsPopupState) {
    if state.picker.items.is_empty() {
        let hint = if state.picker.query.is_empty() {
            "(no agents match this filter)"
        } else {
            "No matching agents"
        };
        Paragraph::new(hint)
            .style(Style::default().fg(Color::DarkGray))
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
        let agent = &state.picker.items[*item_index].agent;
        let is_selected = index == selected;
        let line = agent_row_line(agent, is_selected, state.column, name_w, model_w, effort_w);
        Paragraph::new(line)
            .style(if is_selected {
                Style::default().bg(Color::Rgb(35, 55, 70))
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
                .fg(Color::DarkGray)
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

/// One agent row. Mirrors the column budget of `render_list`.
fn agent_row_line(
    agent: &RunningAgent,
    selected: bool,
    column: AgentColumn,
    name_w: usize,
    model_w: Option<usize>,
    effort_w: Option<usize>,
) -> Line<'static> {
    let cell_style = if selected {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let (icon_style, name_style) = if selected {
        (
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(status_color(&agent.status)),
            if agent.is_active {
                Style::default()
                    .fg(Color::LightRed)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        )
    };

    let name_text = truncate_str(agent.shown_name(), name_w);
    let mut spans = vec![
        Span::styled(status_icon(&agent.status), icon_style),
        Span::raw(" "),
        Span::styled(name_text, name_style),
    ];
    spans.push(Span::raw(
        " ".repeat(name_w.saturating_sub(agent.shown_name().chars().count())),
    ));

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

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str, status: AgentStatus) -> RunningAgent {
        RunningAgent {
            name: name.to_string(),
            agent_id: uuid::Uuid::new_v4(),
            status,
            is_active: false,
            model: None,
            effort: None,
        }
    }

    fn key(state: &mut AgentsPopupState, code: KeyCode) -> AgentsPopupKeyOutcome {
        state.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn search_matches_names_and_model_specs() {
        let mut state = AgentsPopupState::default();
        let mut on_glm = agent("/root/mrine", AgentStatus::Idle);
        on_glm.model = Some("zai:glm-5.3".into());
        state.open_with(vec![
            on_glm.clone(),
            agent("/root/other", AgentStatus::Idle),
        ]);

        key(&mut state, KeyCode::Char('/'));
        for c in "glm".chars() {
            key(&mut state, KeyCode::Char(c));
        }
        let filtered = state.filtered_agents();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "/root/mrine");

        // Ctrl+U clears the query.
        state.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(state.filtered_agents().len(), 2);
    }

    #[test]
    fn status_tabs_filter_and_skip_empty() {
        let mut state = AgentsPopupState::default();
        state.open_with(vec![
            agent("/root/idle1", AgentStatus::Idle),
            agent("/root/busy", AgentStatus::Streaming),
        ]);
        // Right steps to the first tab with agents: Idle (tab 1).
        key(&mut state, KeyCode::Right);
        let filtered = state.filtered_agents();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "/root/idle1");
        // Another right skips empty tabs to Streaming.
        key(&mut state, KeyCode::Right);
        assert_eq!(state.filtered_agents()[0].name, "/root/busy");
    }

    #[test]
    fn column_cursor_changes_enter_dispatch() {
        let mut state = AgentsPopupState::default();
        let mut active = agent("/root/a", AgentStatus::Idle);
        active.is_active = true;
        state.open_with(vec![active.clone()]);
        assert!(matches!(
            key(&mut state, KeyCode::Enter),
            AgentsPopupKeyOutcome::Switch(id) if id == active.agent_id
        ));
        key(&mut state, KeyCode::Char('l'));
        assert!(matches!(
            key(&mut state, KeyCode::Enter),
            AgentsPopupKeyOutcome::OpenModelPicker { agent } if agent == "/root/a"
        ));
        key(&mut state, KeyCode::Char('l'));
        assert!(matches!(
            key(&mut state, KeyCode::Enter),
            AgentsPopupKeyOutcome::CycleEffort { agent } if agent == "/root/a"
        ));
        // Sticky at the last column.
        key(&mut state, KeyCode::Char('l'));
        assert_eq!(state.column, AgentColumn::Effort);
    }

    #[test]
    fn bulk_shortcuts_target_the_filtered_set() {
        let mut state = AgentsPopupState::default();
        state.open_with(vec![
            agent("/root/a", AgentStatus::Idle),
            agent("/root/b", AgentStatus::Error),
        ]);
        // Tab to Error: only b remains.
        key(&mut state, KeyCode::Right); // Idle
        key(&mut state, KeyCode::Right); // Waiting (empty, skipped)
        key(&mut state, KeyCode::Right); // Requesting (empty, skipped)
        key(&mut state, KeyCode::Right); // Streaming (empty, skipped)
        key(&mut state, KeyCode::Right); // Tools (empty, skipped)
        key(&mut state, KeyCode::Right); // Retrying (empty, skipped)
        key(&mut state, KeyCode::Right); // Compacting (empty, skipped)
        key(&mut state, KeyCode::Right); // Error
        assert!(matches!(
            key(&mut state, KeyCode::Char('M')),
            AgentsPopupKeyOutcome::OpenModelPickerBulk { agents } if agents == vec!["/root/b".to_string()]
        ));
    }

    #[test]
    fn model_cell_drops_provider_before_the_first_colon() {
        assert_eq!(model_cell_text(Some("zai:glm-5.3"), 6), "glm-5…");
        assert_eq!(model_cell_text(Some("short"), 8), "short");
        assert_eq!(model_cell_text(None, 8), "");
    }
}
