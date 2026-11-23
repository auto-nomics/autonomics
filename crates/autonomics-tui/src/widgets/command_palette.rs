//! Command palette (Ctrl+P) — built on [`SearchablePicker`].
//!
//! A fuzzy-filterable command list rendered as a centered overlay. The user
//! types to filter, Up/Down to navigate, Enter to execute, Esc to dismiss.
//!
//! The search input is backed by [`TextArea`](crate::xai_textarea::TextArea)
//! so it supports cursor motion, word deletion, undo/redo, kill/yank, and
//! paste while remaining a single-line field.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use i18n::t;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Modifier, Style},
    widgets::StatefulWidgetRef,
};

use crate::widgets::popup::{Popup, PopupControls};
use crate::widgets::searchable_picker::{PickerItem, PickerState, SearchablePicker};
use crate::xai_textarea::{TextArea, TextAreaState};

// ═══════════════════════════════════════════════════════════════════════
// Action enum (unchanged — the App matches on these)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    SplitVertical,
    SplitHorizontal,
    FocusOtherSplit,
    CloseSplit,
    Quit,
    /// Toggle the command palette itself (the `<leader><leader>` slot).
    OpenCommandPalette,
    /// Open the plan popup (the active agent's task plan).
    OpenPlanPopup,
    /// Open the tasks popup (the active agent's background tool tasks).
    OpenTasksPopup,
    CancelAgent,
    ToggleAutoScroll,
    ScrollToBottom,
    ScrollToTop,
    HistorySearch,
    ReloadConfig,
    NewAgent,
    /// Open the model config popup for the active agent.
    ModelConfig,
    /// Open the HPC (Slurm) site config + job monitor panel.
    HpcPanel,
    /// Open the VFS browser overlay (files, mounts, container outputs).
    VfsBrowser,
    EmbeddingConfig,
    /// Open per-agent runtime settings for the active agent.
    AgentConfig,
    /// Open the active agent's data-engine DAG view.
    ViewDag,
    /// Open the bibliography library browser in a popup.
    BibliographyPanel,
    /// Open the agent resume picker.
    ResumeAgent,
    /// Close the active agent leaf and terminate its background process.
    CloseAgent,
    /// Delete the active agent permanently (removes all stored data).
    DeleteAgent,
    /// Toggle collapse for thinking blocks.
    ToggleCollapseThinking,
    /// Toggle collapse for tool call blocks.
    ToggleCollapseToolCalls,
    /// Toggle collapse for tool result blocks.
    ToggleCollapseToolResults,
    /// Chat-only transcript view: drop non-dialogue lines entirely instead
    /// of rendering collapsed placeholders.
    ToggleChatOnly,
    /// Move keyboard focus to the sidebar's docked agents panel.
    FocusAgentsPanel,
    /// Open the message picker to copy a chat message to the clipboard.
    CopyMessage,
    /// Manually compact the active agent's conversation history.
    Compact,
    /// Cycle the global color theme to the next preset (dark → light).
    CycleTheme,
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

    /// Compatibility no-op (agent kinds are handled by the picker popup).
    pub fn set_profiles(&mut self, _profiles: &[agentik_core::AgentKind]) {}

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
    let picker_title = t!("tui.commandPalette.title");
    let picker_footer = t!("tui.commandPalette.footer");
    let picker = SearchablePicker::new(&picker_title)
        .accent(crate::theme::current().accent)
        .footer_hint(&picker_footer);

    if !state.picker.visible {
        return;
    }

    let popup = Popup::new(&picker_title, PopupControls::default())
        .accent(crate::theme::current().accent);
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

const PROMPT_GUTTER: u16 = 2;

fn render_search_input(area: Rect, buf: &mut Buffer, state: &mut CommandPaletteState) {
    let theme = crate::theme::current();
    let prompt_style = if state.textarea.is_empty() {
        Style::default().fg(theme.text_faint)
    } else {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
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
            .fg(theme.text_faint)
            .add_modifier(Modifier::DIM);
        let placeholder = t!("tui.commandPalette.placeholder").to_string();
        let placeholder = placeholder.as_str();
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
            title: t!("tui.commandPalette.newAgent").to_string(),
            keywords: "spawn create start profile picker".into(),
            category: t!("tui.commandPalette.catAgent").to_string(),
            action: CommandAction::NewAgent,
        },
        Command {
            title: t!("tui.commandPalette.resumeAgent").to_string(),
            keywords: "restore reconnect continue previous session agent record".into(),
            category: t!("tui.commandPalette.catAgent").to_string(),
            action: CommandAction::ResumeAgent,
        },
        Command {
            title: t!("tui.commandPalette.closeAgent").to_string(),
            keywords: "close kill terminate shutdown tab leaf".into(),
            category: t!("tui.commandPalette.catAgent").to_string(),
            action: CommandAction::CloseAgent,
        },
        Command {
            title: t!("tui.commandPalette.deleteAgent").to_string(),
            keywords: "delete remove destroy purge storage permanent erase".into(),
            category: t!("tui.commandPalette.catAgent").to_string(),
            action: CommandAction::DeleteAgent,
        },
        Command {
            title: t!("tui.commandPalette.selectModel").to_string(),
            keywords: "model config provider api key switch agent".into(),
            category: t!("tui.commandPalette.catAgent").to_string(),
            action: CommandAction::ModelConfig,
        },
        Command {
            title: t!("tui.commandPalette.computeDashboard").to_string(),
            keywords: "hpc slurm cluster site config jobs monitor ssh partition apptainer sbatch"
                .into(),
            category: t!("tui.commandPalette.catSystem").to_string(),
            action: CommandAction::HpcPanel,
        },
        Command {
            title: t!("tui.commandPalette.vfsBrowser").to_string(),
            keywords:
                "vfs files browse mounts container outputs upload downloads rename delete mkdir"
                    .into(),
            category: t!("tui.commandPalette.catSystem").to_string(),
            action: CommandAction::VfsBrowser,
        },
        Command {
            title: t!("tui.commandPalette.embeddingModel").to_string(),
            keywords: "embedding ollama model provider config vector".into(),
            category: t!("tui.commandPalette.catModel").to_string(),
            action: CommandAction::EmbeddingConfig,
        },
        Command {
            title: t!("tui.commandPalette.agentSettings").to_string(),
            keywords: "agent settings memory runtime use generate per agent".into(),
            category: t!("tui.commandPalette.catAgent").to_string(),
            action: CommandAction::AgentConfig,
        },
        Command {
            title: t!("tui.commandPalette.viewDag").to_string(),
            keywords: "dag graph pipeline topology data engine node edges ctrl o".into(),
            category: t!("tui.commandPalette.catData").to_string(),
            action: CommandAction::ViewDag,
        },
        Command {
            title: t!("tui.commandPalette.bibliographyLibrary").to_string(),
            keywords: "bib bibliography literature papers articles fulltext collections".into(),
            category: t!("tui.commandPalette.catResearch").to_string(),
            action: CommandAction::BibliographyPanel,
        },
        Command {
            title: t!("tui.commandPalette.searchHistory").to_string(),
            keywords: "ctrl r recall previous prompt".into(),
            category: t!("tui.commandPalette.catAgent").to_string(),
            action: CommandAction::HistorySearch,
        },
        Command {
            title: t!("tui.commandPalette.cancelAgent").to_string(),
            keywords: "stop abort interrupt request".into(),
            category: t!("tui.commandPalette.catAgent").to_string(),
            action: CommandAction::CancelAgent,
        },
        Command {
            title: t!("tui.commandPalette.compactConversation").to_string(),
            keywords: "compact compress summarize context memory shrink reduce".into(),
            category: t!("tui.commandPalette.catAgent").to_string(),
            action: CommandAction::Compact,
        },
        Command {
            title: t!("tui.commandPalette.scrollToBottom").to_string(),
            keywords: "tail follow latest end".into(),
            category: t!("tui.commandPalette.catView").to_string(),
            action: CommandAction::ScrollToBottom,
        },
        Command {
            title: t!("tui.commandPalette.scrollToTop").to_string(),
            keywords: "home beginning first".into(),
            category: t!("tui.commandPalette.catView").to_string(),
            action: CommandAction::ScrollToTop,
        },
        Command {
            title: t!("tui.commandPalette.toggleAutoScroll").to_string(),
            keywords: "follow pin lock ctrl g".into(),
            category: t!("tui.commandPalette.catView").to_string(),
            action: CommandAction::ToggleAutoScroll,
        },
        Command {
            title: t!("tui.commandPalette.toggleThinkingBlocks").to_string(),
            keywords: "collapse fold hide thinking reasoning".into(),
            category: t!("tui.commandPalette.catView").to_string(),
            action: CommandAction::ToggleCollapseThinking,
        },
        Command {
            title: t!("tui.commandPalette.toggleToolCalls").to_string(),
            keywords: "collapse fold hide tool call function".into(),
            category: t!("tui.commandPalette.catView").to_string(),
            action: CommandAction::ToggleCollapseToolCalls,
        },
        Command {
            title: t!("tui.commandPalette.chatOnlyView").to_string(),
            keywords: "clean dialogue hide tools thinking transcript chat only".into(),
            category: t!("tui.commandPalette.catView").to_string(),
            action: CommandAction::ToggleChatOnly,
        },
        Command {
            title: t!("tui.commandPalette.agentsPopup").to_string(),
            keywords: "switch agent picker popup search filter ctrl t".into(),
            category: t!("tui.commandPalette.catAgent").to_string(),
            action: CommandAction::FocusAgentsPanel,
        },
        Command {
            title: t!("tui.commandPalette.toggleToolResults").to_string(),
            keywords: "collapse fold hide tool result output response".into(),
            category: t!("tui.commandPalette.catView").to_string(),
            action: CommandAction::ToggleCollapseToolResults,
        },
        Command {
            title: t!("tui.commandPalette.copyMessage").to_string(),
            keywords: "copy clipboard snippet transcript chat pick select text message".into(),
            category: t!("tui.commandPalette.catClipboard").to_string(),
            action: CommandAction::CopyMessage,
        },
        Command {
            title: t!("tui.commandPalette.reloadConfig").to_string(),
            keywords: "refresh providers model".into(),
            category: t!("tui.commandPalette.catConfig").to_string(),
            action: CommandAction::ReloadConfig,
        },
        Command {
            title: t!("tui.commandPalette.toggleTheme").to_string(),
            keywords: "theme color colourscheme dark light appearance skin".into(),
            category: t!("tui.commandPalette.catApp").to_string(),
            action: CommandAction::CycleTheme,
        },
        Command {
            title: t!("tui.commandPalette.quit").to_string(),
            keywords: "exit close bye".into(),
            category: t!("tui.commandPalette.catApp").to_string(),
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
