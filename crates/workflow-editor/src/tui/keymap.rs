//! Key bindings.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::event::AppEvent;
use super::mode::Mode;

/// Centralized key bindings. Each entry says: "in mode M, this key triggers
/// this `Action`." Tying all bindings to one file makes the help overlay
/// trivial to populate and prevents accidental rebinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // ── Always-on ──
    Quit,
    ShowHelp,
    SaveWorkflow,
    RunWorkflow,
    Undo,
    Redo,
    OpenCommandPalette,
    OpenHistory,
    OpenNodePicker,
    FocusPalette,
    FocusCanvas,
    FocusInspector,

    // ── Normal-mode (canvas) ──
    PanUp,
    PanDown,
    PanLeft,
    PanRight,
    ZoomIn,
    ZoomOut,
    SelectNextNode,
    SelectPrevNode,
    DeleteSelection,
    EditSelection,
    StartEdgeDraw,
    EditSop,
    EnterInsertMode,

    // ── History panel ──
    SelectNextHistory,
    SelectPrevHistory,
    CheckoutSnapshot,
}

impl Action {
    /// Look up the action for a key event in the given mode.
    pub fn for_key(mode: Mode, k: &KeyEvent) -> Option<Self> {
        // Ctrl-* bindings are mode-independent.
        if k.modifiers.contains(KeyModifiers::CONTROL) {
            return match k.code {
                KeyCode::Char('c') => Some(Action::Quit),
                KeyCode::Char('s') => Some(Action::SaveWorkflow),
                KeyCode::Char('r') => Some(Action::RunWorkflow),
                KeyCode::Char('z') => Some(Action::Undo),
                KeyCode::Char('y') => Some(Action::Redo),
                KeyCode::Char('p') => Some(Action::OpenCommandPalette),
                _ => None,
            };
        }
        match mode {
            Mode::Normal => normal_mode(k),
            Mode::Insert => insert_mode(k),
            Mode::NodePicker => node_picker_mode(k),
            Mode::SpecEditor => insert_mode(k),
            Mode::SopEditor => insert_mode(k),
            Mode::CommandPalette => insert_mode(k),
            Mode::EdgeDrawing => edge_drawing_mode(k),
            Mode::HistoryPanel => history_panel_mode(k),
            _ => None,
        }
    }
}

fn normal_mode(k: &KeyEvent) -> Option<Action> {
    match k.code {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('?') => Some(Action::ShowHelp),
        KeyCode::Char(':') => Some(Action::OpenCommandPalette),
        KeyCode::Char('h') => Some(Action::OpenHistory),
        KeyCode::Char('n') => Some(Action::OpenNodePicker),
        KeyCode::Char('e') => Some(Action::EditSelection),
        KeyCode::Char('s') => Some(Action::EditSop),
        KeyCode::Char('i') => Some(Action::EnterInsertMode),
        KeyCode::Char('d') | KeyCode::Delete => Some(Action::DeleteSelection),
        KeyCode::Tab => Some(Action::FocusInspector),
        KeyCode::BackTab => Some(Action::FocusPalette),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::PanUp),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::PanDown),
        KeyCode::Char('l') | KeyCode::Right => Some(Action::PanRight),
        KeyCode::Char('+') | KeyCode::Char('=') => Some(Action::ZoomIn),
        KeyCode::Char('-') => Some(Action::ZoomOut),
        KeyCode::Char(']') | KeyCode::PageDown => Some(Action::SelectNextNode),
        KeyCode::Char('[') | KeyCode::PageUp => Some(Action::SelectPrevNode),
        KeyCode::Char(' ') | KeyCode::Enter => Some(Action::StartEdgeDraw),
        _ => None,
    }
}

fn insert_mode(k: &KeyEvent) -> Option<Action> {
    match k.code {
        KeyCode::Esc => Some(Action::FocusCanvas),
        _ => None,
    }
}

fn node_picker_mode(k: &KeyEvent) -> Option<Action> {
    match k.code {
        KeyCode::Esc => Some(Action::FocusCanvas),
        KeyCode::Enter => Some(Action::EditSelection),
        _ => None,
    }
}

fn edge_drawing_mode(k: &KeyEvent) -> Option<Action> {
    match k.code {
        KeyCode::Esc => Some(Action::FocusCanvas),
        _ => None,
    }
}

fn history_panel_mode(k: &KeyEvent) -> Option<Action> {
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') => Some(Action::FocusCanvas),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::SelectNextHistory),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::SelectPrevHistory),
        KeyCode::Enter => Some(Action::CheckoutSnapshot),
        _ => None,
    }
}

/// Hint string shown in the status bar for the current mode.
pub fn hint(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => {
            "n new │ e edit │ d del │ s sop │ : cmd │ h hist │ ? help │ ^s save │ ^r run │ ^q quit"
        }
        Mode::Insert => "Esc to leave insert mode",
        Mode::EdgeDrawing => "click target port │ Esc cancel",
        Mode::NodePicker => "type to filter │ Enter place │ Esc cancel",
        Mode::SpecEditor => "Esc to commit & return",
        Mode::SopEditor => "Esc to commit & return",
        Mode::CommandPalette => "type a command │ Enter run │ Esc cancel",
        Mode::HelpOverlay => "Esc / q to close",
        Mode::HistoryPanel => "j/k next/prev │ Enter checkout │ Esc close",
        Mode::Running => "running…",
    }
}

/// Convert an [`AppEvent`] key into an [`Action`] using the current mode.
pub fn action_for(mode: Mode, ev: &AppEvent) -> Option<Action> {
    if let AppEvent::Key(k) = ev {
        Action::for_key(mode, k)
    } else {
        None
    }
}
