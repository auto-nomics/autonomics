//! Model quick-picker for the docked agents panel (`m` / `M`).
//!
//! Lists every catalogue model under a **configured** provider so any
//! running agent — not just the active leaf — can be hot-swapped from the
//! sidebar. Selection fans out through [`PickerTarget`]: a single agent or
//! every agent currently passing the panel's filter (bulk path then shows
//! the shared `y/n` confirmation popup before applying).
//!
//! The search input is backed by [`TextArea`](crate::xai_textarea::TextArea)
//! (same rich-editing behavior as the message picker) and filtering /
//! navigation live in [`PickerState`].

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, StatefulWidgetRef, Widget},
};

use crate::widgets::model_config_widget::ModelConfigState;
use crate::widgets::popup::{Popup, PopupControls};
use crate::widgets::searchable_picker::{PickerItem, PickerState};
use crate::xai_textarea::{TextArea, TextAreaState};

const ACCENT: Color = Color::Cyan;
/// Prompt gutter width for the search input: `❯ ` = 2 cols.
const PROMPT_GUTTER: u16 = 2;

// ═══════════════════════════════════════════════════════════════════════
// Item + target
// ═══════════════════════════════════════════════════════════════════════

/// One catalogue model offered for hot-swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentModelItem {
    /// Full `provider:model` spec — the exact string `PUT /model` expects.
    pub spec: String,
    pub context_length: u64,
    pub thinking: bool,
    /// Precomputed list tag (`131k`, `2M ·think`, …) — `PickerItem::category`
    /// borrows, so the string must live in the item.
    pub tag: String,
}

impl AgentModelItem {
    /// Build an item, deriving the context/thinking tag.
    pub fn new(spec: String, context_length: u64, thinking: bool) -> Self {
        let ctx = if context_length >= 1_000_000 {
            format!("{}M", context_length / 1_000_000)
        } else {
            format!("{}k", context_length / 1_000)
        };
        let tag = if thinking {
            format!("{ctx} ·think")
        } else {
            ctx
        };
        Self {
            spec,
            context_length,
            thinking,
            tag,
        }
    }
}

impl PickerItem for AgentModelItem {
    fn title(&self) -> &str {
        &self.spec
    }
    fn keywords(&self) -> &str {
        &self.spec
    }
    fn category(&self) -> Option<&str> {
        Some(&self.tag)
    }
}

/// Who a confirmed selection applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerTarget {
    /// The one agent whose row/cell opened the picker.
    Single { agent: String },
    /// Every agent passing the panel filter at open time (bulk, confirmed
    /// via the shared popup before anything is sent).
    AllFiltered { agents: Vec<String> },
}

impl PickerTarget {
    /// Short header describing the apply scope.
    pub fn describe(&self) -> String {
        match self {
            Self::Single { agent } => agent.clone(),
            Self::AllFiltered { agents } => format!("{} filtered agents", agents.len()),
        }
    }
}

/// Build the pickable catalogue from the in-memory config state — no HTTP,
/// the same rows the model config panel shows. Only **configured**
/// providers contribute (an unconfigured provider cannot resolve a swap).
pub fn collect_catalog_models(state: &ModelConfigState) -> Vec<AgentModelItem> {
    let mut items = Vec::new();
    for provider in &state.providers {
        if !provider.configured {
            continue;
        }
        for model in &provider.models {
            // Spec construction must stay identical to the config
            // panel's SelectModel command (full "provider:model").
            items.push(AgentModelItem::new(
                format!("{}:{}", provider.name, model.model_name),
                model.context_length,
                model.supports_thinking,
            ));
        }
    }
    items
}

// ═══════════════════════════════════════════════════════════════════════
// Key outcome
// ═══════════════════════════════════════════════════════════════════════

/// Result of forwarding a key event to the model picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentModelPickerKeyOutcome {
    /// The key was consumed but no action is needed.
    Consumed,
    /// The user pressed Enter with a selection — apply `spec` to `target`.
    Confirm { spec: String, target: PickerTarget },
    /// The user pressed Esc — close the picker.
    Close,
}

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

/// State for the agents-panel model quick-picker popup.
pub struct AgentModelPickerState {
    /// Item list + filtering state.
    pub picker: PickerState<AgentModelItem>,
    /// Search input editor (single-line).
    pub textarea: TextArea,
    /// Render state for the textarea.
    pub textarea_state: TextAreaState,
    /// On-screen cursor position computed during the most recent render.
    pub cursor_pos: Option<(u16, u16)>,
    /// Who a confirmed selection applies to.
    pub target: Option<PickerTarget>,
    /// The single target's current spec — its row gets the `●` marker.
    pub current_spec: Option<String>,
}

impl Default for AgentModelPickerState {
    fn default() -> Self {
        let mut textarea = TextArea::new();
        textarea.show_scrollbar = false;
        Self {
            picker: PickerState::new_empty(),
            textarea,
            textarea_state: TextAreaState::default(),
            cursor_pos: None,
            target: None,
            current_spec: None,
        }
    }
}

impl AgentModelPickerState {
    pub fn is_visible(&self) -> bool {
        self.picker.visible
    }

    pub fn close(&mut self) {
        self.picker.close();
        self.textarea.set_text("");
        self.textarea.clear_history();
        self.cursor_pos = None;
        self.target = None;
        self.current_spec = None;
    }

    /// Open with a catalogue snapshot and apply target. `current_spec` only
    /// matters for `Single` (the `●` marker on the agent's live model).
    pub fn open_with(
        &mut self,
        items: Vec<AgentModelItem>,
        target: PickerTarget,
        current_spec: Option<String>,
    ) {
        self.textarea.set_text("");
        self.textarea.clear_history();
        self.current_spec = match &target {
            PickerTarget::Single { .. } => current_spec,
            PickerTarget::AllFiltered { .. } => None,
        };
        self.target = Some(target);
        self.picker.set_items(items);
        self.picker.open();
    }

    /// Clone of the currently selected item, if any.
    #[allow(dead_code)] // the Confirm outcome already carries the spec
    pub fn selected_item(&self) -> Option<AgentModelItem> {
        self.picker.selected_item()
    }

    // ── Key handling ──

    /// Esc → close. Enter → confirm (needs a selection). Up/Down → list
    /// navigation. Ctrl+U clears the query. Everything else edits the
    /// textarea and re-filters.
    pub fn handle_key(&mut self, key: KeyEvent) -> AgentModelPickerKeyOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return AgentModelPickerKeyOutcome::Close,
            KeyCode::Enter => {
                if let (Some(item), Some(target)) =
                    (self.picker.selected_item(), self.target.clone())
                {
                    return AgentModelPickerKeyOutcome::Confirm {
                        spec: item.spec,
                        target,
                    };
                }
                return AgentModelPickerKeyOutcome::Consumed;
            }
            KeyCode::Up => {
                self.picker.move_up();
                return AgentModelPickerKeyOutcome::Consumed;
            }
            KeyCode::Down => {
                self.picker.move_down();
                return AgentModelPickerKeyOutcome::Consumed;
            }
            KeyCode::Char('u') if ctrl => {
                self.textarea.set_text("");
                self.sync_query();
                return AgentModelPickerKeyOutcome::Consumed;
            }
            _ => {}
        }
        self.textarea.input(key);
        self.sync_query();
        AgentModelPickerKeyOutcome::Consumed
    }

    fn sync_query(&mut self) {
        let text = self.textarea.text().to_string();
        self.picker.set_query(&text);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Render
// ═══════════════════════════════════════════════════════════════════════

/// Render the model quick-picker: target header, search row, one-column
/// model list (spec + context/think tag, `●` on the target's live model),
/// footer.
pub fn render_agent_model_picker(
    frame_area: Rect,
    buf: &mut Buffer,
    state: &mut AgentModelPickerState,
) {
    if !state.picker.visible {
        return;
    }

    let popup_w = (frame_area.width * 6 / 10).max(44);
    let popup = Popup::new(" Switch Model ", PopupControls::default())
        .accent(ACCENT)
        .width(popup_w);
    let inner = popup.render(frame_area, buf);

    let v_regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header (target)
            Constraint::Length(1), // search input
            Constraint::Length(1), // separator
            Constraint::Min(5),    // model list
            Constraint::Length(1), // footer
        ])
        .split(inner);

    // ── Target header ──
    let target_label = state
        .target
        .as_ref()
        .map(PickerTarget::describe)
        .unwrap_or_default();
    Widget::render(
        Paragraph::new(Line::from(vec![
            Span::styled(" Apply to ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                target_label,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        v_regions[0],
        buf,
    );

    // ── Search input ──
    render_search_input(v_regions[1], buf, state);

    // ── Separator ──
    Widget::render(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
        v_regions[2],
        buf,
    );

    // ── Model list ──
    render_list(v_regions[3], buf, state);

    // ── Footer ──
    let footer = Line::from(vec![
        Span::styled(
            " Enter",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" select  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "↑↓",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "Esc",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
    ]);
    Widget::render(Paragraph::new(footer), v_regions[4], buf);
}

fn render_search_input(area: Rect, buf: &mut Buffer, state: &mut AgentModelPickerState) {
    let prompt_style = if state.textarea.is_empty() {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    };
    buf.set_string(area.x, area.y, "❯", prompt_style);

    let ta_area = if area.width > PROMPT_GUTTER {
        Rect {
            x: area.x + PROMPT_GUTTER,
            width: area.width - PROMPT_GUTTER,
            ..area
        }
    } else {
        return;
    };

    if state.textarea.is_empty() {
        let hint = " filter models";
        buf.set_string(
            ta_area.x,
            ta_area.y,
            hint,
            Style::default().fg(Color::DarkGray),
        );
        state.cursor_pos = Some((ta_area.x, ta_area.y));
        return;
    }

    let textarea: &TextArea = &state.textarea;
    let ta_state: &mut TextAreaState = &mut state.textarea_state;
    textarea.render_ref(ta_area, buf, ta_state);
    state.cursor_pos = textarea.cursor_pos_with_state(ta_area, state.textarea_state);
}

fn render_list(area: Rect, buf: &mut Buffer, state: &mut AgentModelPickerState) {
    if state.picker.items.is_empty() {
        let hint = if state.picker.query.is_empty() {
            " No configured providers with models — add one in Model Config (Ctrl+M)"
        } else {
            " No matching models"
        };
        Paragraph::new(hint)
            .style(Style::default().fg(Color::DarkGray))
            .centered()
            .render(area, buf);
        return;
    }

    let rows = state.picker.filtered.len();
    let selected = state.picker.selected.min(rows.saturating_sub(1));

    for (index, item_index) in state.picker.filtered.iter().enumerate() {
        let y = area.y + index as u16;
        if y >= area.bottom() {
            break;
        }
        let item = &state.picker.items[*item_index];
        let row_style = if index == selected {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let current = state
            .current_spec
            .as_deref()
            .is_some_and(|spec| spec == item.spec);
        let prefix = if current { "● " } else { "  " };
        let title_width = area.width.saturating_sub(20) as usize;
        let title = truncate_str(item.title(), title_width);
        let line = Line::from(vec![
            Span::styled(
                prefix,
                Style::default().fg(if current { ACCENT } else { Color::DarkGray }),
            ),
            Span::styled(title, row_style),
            Span::styled(
                format!("  {}", item.tag),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        Widget::render(
            Paragraph::new(line).style(if index == selected {
                Style::default().bg(Color::Rgb(35, 55, 70))
            } else {
                Style::default()
            }),
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
