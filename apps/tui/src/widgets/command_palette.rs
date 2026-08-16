//! Command palette (Ctrl+P) — built on [`SearchablePicker`].
//!
//! A fuzzy-filterable command list rendered as a centered overlay. The user
//! types to filter, Up/Down to navigate, Enter to execute, Esc to dismiss.
//!
//! The search input is backed by [`TextArea`](crate::xai_textarea::TextArea)
//! so it supports cursor motion, word deletion, undo/redo, kill/yank, and
//! paste while remaining a single-line field.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    widgets::StatefulWidgetRef,
};

use crate::widgets::popup::Popup;
use crate::widgets::searchable_picker::{PickerItem, PickerState, SearchablePicker};
use crate::xai_textarea::{TextArea, TextAreaState};

// ═══════════════════════════════════════════════════════════════════════
// Action enum (unchanged — the App matches on these)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    Quit,
    CancelAgent,
    EnterInput,
    ToggleAutoScroll,
    ScrollToBottom,
    ScrollToTop,
    HistorySearch,
    ClearTranscript,
    ReloadConfig,
    SpawnAgent(String),
    NewAgent,
    /// Open the model config popup for the active agent.
    ModelConfig,
    /// Open the agent resume picker.
    ResumeAgent,
    /// Close the active agent leaf and terminate its background process.
    CloseAgent,
    /// Delete the active agent permanently (removes all stored data).
    DeleteAgent,
    /// Open the session picker for the currently active agent.
    OpenSessions,
    /// Create a new conversation session within the active agent.
    NewSession,
    /// Toggle collapse for thinking blocks.
    ToggleCollapseThinking,
    /// Toggle collapse for tool call blocks.
    ToggleCollapseToolCalls,
    /// Toggle collapse for tool result blocks.
    ToggleCollapseToolResults,
    /// Open the message picker to copy a chat message to the clipboard.
    CopyMessage,
    /// Manually compact the active agent's conversation history.
    Compact,
}

// ═══════════════════════════════════════════════════════════════════════
// Command item (implements PickerItem)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct Command {
    pub title: String,
    pub keywords: String,
    pub category: String,
    pub action: CommandAction,
}

impl PickerItem for Command {
    fn title(&self) -> &str {
        &self.title
    }
    fn keywords(&self) -> &str {
        &self.keywords
    }
    fn category(&self) -> Option<&str> {
        Some(&self.category)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Key outcome
// ═══════════════════════════════════════════════════════════════════════

/// Result of forwarding a key event to the command palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandPaletteKeyOutcome {
    /// The key was consumed by search editing or list navigation.
    Consumed,
    /// Enter was pressed with a selected command.
    Execute(CommandAction),
    /// Esc was pressed.
    Close,
}

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

/// State for the command palette.
pub struct CommandPaletteState {
    /// Item list, query mirror, filtering, and list navigation state.
    pub picker: PickerState<Command>,
    /// Search input editor.
    textarea: TextArea,
    /// Render state for the textarea.
    textarea_state: TextAreaState,
    /// Hardware cursor position recorded by the most recent render.
    pub cursor_pos: Option<(u16, u16)>,
}

impl CommandPaletteState {
    /// Build the default command list.
    pub fn new() -> Self {
        let mut picker = PickerState::new_empty();
        picker.set_items(default_commands());
        let mut textarea = TextArea::new();
        textarea.show_scrollbar = false;
        Self {
            picker,
            textarea,
            textarea_state: TextAreaState::default(),
            cursor_pos: None,
        }
    }

    /// Compatibility no-op (profiles are handled by the picker popup).
    pub fn set_profiles(&mut self, _profiles: &[agentik_core::AgentProfile]) {}

    /// Whether the palette is visible.
    pub fn is_visible(&self) -> bool {
        self.picker.visible
    }

    /// Open the palette with an empty search.
    pub fn open(&mut self) {
        self.reset_search();
        self.picker.open();
    }

    /// Close the palette and clear search editing state.
    pub fn close(&mut self) {
        self.picker.close();
        self.reset_search();
        self.sync_query();
        self.cursor_pos = None;
    }

    /// Toggle palette visibility.
    pub fn toggle(&mut self) {
        if self.is_visible() {
            self.close();
        } else {
            self.open();
        }
    }

    /// Returns the action of the currently selected command.
    pub fn selected_action(&self) -> Option<CommandAction> {
        self.picker.selected_item().map(|c| c.action)
    }

    /// Forward a key event to popup controls or the embedded textarea.
    pub fn handle_key(&mut self, key: KeyEvent) -> CommandPaletteKeyOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => return CommandPaletteKeyOutcome::Close,
            KeyCode::Enter => {
                return match self.selected_action() {
                    Some(action) => CommandPaletteKeyOutcome::Execute(action),
                    None => CommandPaletteKeyOutcome::Consumed,
                };
            }
            KeyCode::Up => {
                self.picker.move_up();
                return CommandPaletteKeyOutcome::Consumed;
            }
            KeyCode::Down => {
                self.picker.move_down();
                return CommandPaletteKeyOutcome::Consumed;
            }
            KeyCode::Char('u') if ctrl => {
                self.textarea.set_text("");
                self.textarea.clear_history();
                self.sync_query();
                return CommandPaletteKeyOutcome::Consumed;
            }
            _ => {}
        }

        self.textarea.input(key);
        self.sync_query();
        CommandPaletteKeyOutcome::Consumed
    }

    fn sync_query(&mut self) {
        let query = self.textarea.text().to_string();
        self.picker.set_query(&query);
    }

    fn reset_search(&mut self) {
        self.textarea.set_text("");
        self.textarea.clear_history();
        self.textarea_state = TextAreaState::default();
    }
}

impl Default for CommandPaletteState {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Render
// ═══════════════════════════════════════════════════════════════════════

/// Render the command palette popup.
pub fn render_command_palette(
    frame_area: ratatui::layout::Rect,
    buf: &mut ratatui::prelude::Buffer,
    state: &mut CommandPaletteState,
) {
    let picker = SearchablePicker::new(" Command Palette ")
        .accent(ratatui::style::Color::Cyan)
        .footer_hint(" Enter run  ↑↓ navigate  Esc cancel");

    if !state.picker.visible {
        return;
    }

    let popup = Popup::new(" Command Palette ").accent(Color::Cyan);
    let inner = popup.render(frame_area, buf);
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Search input
            Constraint::Min(3),    // List
            Constraint::Length(1), // Footer
        ])
        .split(inner);

    render_search_input(regions[0], buf, state);
    picker.render_list_and_footer(regions[1], regions[2], buf, &mut state.picker);
}

const ACCENT: Color = Color::Cyan;
const PROMPT_GUTTER: u16 = 2;

fn render_search_input(area: Rect, buf: &mut Buffer, state: &mut CommandPaletteState) {
    let prompt_style = if state.textarea.is_empty() {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    };
    buf.set_string(area.x, area.y, ">", prompt_style);

    let ta_area = if area.width > PROMPT_GUTTER {
        Rect {
            x: area.x + PROMPT_GUTTER,
            width: area.width - PROMPT_GUTTER,
            ..area
        }
    } else {
        area
    };

    if state.textarea.is_empty() {
        let placeholder_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);
        let placeholder = "search commands…";
        let truncated: String = placeholder.chars().take(ta_area.width as usize).collect();
        buf.set_string(ta_area.x, ta_area.y, truncated, placeholder_style);
        state.cursor_pos = Some((ta_area.x, ta_area.y));
    } else {
        let textarea: &TextArea = &state.textarea;
        let textarea_state: &mut TextAreaState = &mut state.textarea_state;
        textarea.render_ref(ta_area, buf, textarea_state);
        state.cursor_pos = state
            .textarea
            .cursor_pos_with_state(ta_area, state.textarea_state);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Default command catalogue
// ═══════════════════════════════════════════════════════════════════════

fn default_commands() -> Vec<Command> {
    vec![
        Command {
            title: "New agent".into(),
            keywords: "spawn create start profile picker".into(),
            category: "agent".into(),
            action: CommandAction::NewAgent,
        },
        Command {
            title: "Resume agent".into(),
            keywords: "restore reconnect continue previous session agent record".into(),
            category: "agent".into(),
            action: CommandAction::ResumeAgent,
        },
        Command {
            title: "Sessions".into(),
            keywords: "conversation switch fork list branch".into(),
            category: "agent".into(),
            action: CommandAction::OpenSessions,
        },
        Command {
            title: "Close agent".into(),
            keywords: "close kill terminate shutdown tab leaf".into(),
            category: "agent".into(),
            action: CommandAction::CloseAgent,
        },
        Command {
            title: "Delete agent".into(),
            keywords: "delete remove destroy purge storage permanent erase".into(),
            category: "agent".into(),
            action: CommandAction::DeleteAgent,
        },
        Command {
            title: "Select model".into(),
            keywords: "model config provider api key switch agent".into(),
            category: "agent".into(),
            action: CommandAction::ModelConfig,
        },
        Command {
            title: "New session".into(),
            keywords: "new conversation clear transcript reset chat branch".into(),
            category: "agent".into(),
            action: CommandAction::NewSession,
        },
        Command {
            title: "Search history".into(),
            keywords: "ctrl r recall previous prompt".into(),
            category: "agent".into(),
            action: CommandAction::HistorySearch,
        },
        Command {
            title: "Cancel agent".into(),
            keywords: "stop abort interrupt request".into(),
            category: "agent".into(),
            action: CommandAction::CancelAgent,
        },
        Command {
            title: "Compact conversation".into(),
            keywords: "compact compress summarize context memory shrink reduce".into(),
            category: "agent".into(),
            action: CommandAction::Compact,
        },
        Command {
            title: "Scroll to bottom".into(),
            keywords: "tail follow latest end".into(),
            category: "view".into(),
            action: CommandAction::ScrollToBottom,
        },
        Command {
            title: "Scroll to top".into(),
            keywords: "home beginning first".into(),
            category: "view".into(),
            action: CommandAction::ScrollToTop,
        },
        Command {
            title: "Toggle auto-scroll".into(),
            keywords: "follow pin lock ctrl g".into(),
            category: "view".into(),
            action: CommandAction::ToggleAutoScroll,
        },
        Command {
            title: "Toggle thinking blocks".into(),
            keywords: "collapse fold hide thinking reasoning".into(),
            category: "view".into(),
            action: CommandAction::ToggleCollapseThinking,
        },
        Command {
            title: "Toggle tool calls".into(),
            keywords: "collapse fold hide tool call function".into(),
            category: "view".into(),
            action: CommandAction::ToggleCollapseToolCalls,
        },
        Command {
            title: "Toggle tool results".into(),
            keywords: "collapse fold hide tool result output response".into(),
            category: "view".into(),
            action: CommandAction::ToggleCollapseToolResults,
        },
        Command {
            title: "Copy message".into(),
            keywords: "copy clipboard snippet transcript chat pick select text message".into(),
            category: "clipboard".into(),
            action: CommandAction::CopyMessage,
        },
        Command {
            title: "Reload config".into(),
            keywords: "refresh providers model".into(),
            category: "config".into(),
            action: CommandAction::ReloadConfig,
        },
        Command {
            title: "Quit".into(),
            keywords: "exit close bye".into(),
            category: "app".into(),
            action: CommandAction::Quit,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn type_text(state: &mut CommandPaletteState, text: &str) {
        for character in text.chars() {
            state.handle_key(key(KeyCode::Char(character), KeyModifiers::NONE));
        }
    }

    #[test]
    fn textarea_editing_syncs_picker_query() {
        let mut state = CommandPaletteState::new();
        state.open();
        type_text(&mut state, "agent");

        assert_eq!(state.textarea.text(), "agent");
        assert_eq!(state.picker.query, "agent");
        assert!(state.picker.filtered.len() > 1);
    }

    #[test]
    fn textarea_cursor_motion_inserts_at_cursor() {
        let mut state = CommandPaletteState::new();
        state.open();
        type_text(&mut state, "ab");
        state.handle_key(key(KeyCode::Left, KeyModifiers::NONE));
        state.handle_key(key(KeyCode::Char('x'), KeyModifiers::NONE));

        assert_eq!(state.textarea.text(), "axb");
        assert_eq!(state.picker.query, "axb");
    }

    #[test]
    fn ctrl_u_clears_search_and_refilters() {
        let mut state = CommandPaletteState::new();
        state.open();
        let all = state.picker.filtered.len();
        type_text(&mut state, "model");
        state.handle_key(key(KeyCode::Char('u'), KeyModifiers::CONTROL));

        assert!(state.textarea.is_empty());
        assert_eq!(state.picker.query, "");
        assert_eq!(state.picker.filtered.len(), all);
    }

    #[test]
    fn close_clears_textarea_and_render_state() {
        let mut state = CommandPaletteState::new();
        state.open();
        type_text(&mut state, "session");
        state.close();

        assert!(state.textarea.is_empty());
        assert!(state.picker.query.is_empty());
        assert!(!state.picker.visible);
        assert!(state.cursor_pos.is_none());
    }

    #[test]
    fn enter_returns_selected_command_action() {
        let mut state = CommandPaletteState::new();
        state.open();
        let outcome = state.handle_key(key(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            outcome,
            CommandPaletteKeyOutcome::Execute(CommandAction::NewAgent)
        );
    }
}
