//! Command palette (Ctrl+P) — built on [`SearchablePicker`].
//!
//! A fuzzy-filterable command list rendered as a centered overlay. The user
//! types to filter, Up/Down to navigate, Enter to execute, Esc to dismiss.

use crate::widgets::searchable_picker::{PickerItem, PickerState, SearchablePicker};

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
// State (type alias)
// ═══════════════════════════════════════════════════════════════════════

/// State for the command palette. Backed by [`PickerState<Command>`].
pub type CommandPaletteState = PickerState<Command>;

impl CommandPaletteState {
    /// Build the default command list.
    pub fn new() -> Self {
        let mut state = PickerState::new_empty();
        state.set_items(default_commands());
        state
    }

    /// Compatibility no-op (profiles are handled by the picker popup).
    pub fn set_profiles(&mut self, _profiles: &[agentik_core::AgentProfile]) {}

    /// Returns the action of the currently selected command.
    pub fn selected_action(&self) -> Option<CommandAction> {
        self.selected_item().map(|c| c.action)
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
    SearchablePicker::new(" Command Palette ")
        .accent(ratatui::style::Color::Cyan)
        .placeholder(" search commands…")
        .footer_hint(" Enter run  ↑↓ navigate  Esc cancel")
        .render(frame_area, buf, state);
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
