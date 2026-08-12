//! Editor mode state machine.

use crossterm::event::KeyEvent;

/// One of the modes the TUI editor can be in.
///
/// Most key presses are interpreted by the active mode. Mode transitions
/// are managed in [`super::state::AppState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Default navigation mode: pan, zoom, select.
    Normal,
    /// Typing in the inspector / SOP editor / spec form.
    Insert,
    /// Currently drawing an edge from one port to another.
    EdgeDrawing,
    /// Picking a node kind to drop onto the canvas.
    NodePicker,
    /// Editing the selected node's params (JSON-schema form).
    SpecEditor,
    /// Editing the workflow-level SOP.
    SopEditor,
    /// Command palette (`:w`, `:q`, `:r`, `:h`, ...).
    CommandPalette,
    /// Help overlay.
    HelpOverlay,
    /// Snapshot history panel is open.
    HistoryPanel,
    /// Workflow is currently running (async task active).
    Running,
}

impl Mode {
    /// Human-readable name for the status bar.
    pub fn label(&self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Insert => "insert",
            Mode::EdgeDrawing => "edge",
            Mode::NodePicker => "picker",
            Mode::SpecEditor => "spec",
            Mode::SopEditor => "sop",
            Mode::CommandPalette => "command",
            Mode::HelpOverlay => "help",
            Mode::HistoryPanel => "history",
            Mode::Running => "running",
        }
    }

    /// True if the mode consumes text input (insert, spec, sop, command).
    pub fn is_text_input(&self) -> bool {
        matches!(
            self,
            Mode::Insert | Mode::SpecEditor | Mode::SopEditor | Mode::CommandPalette
        )
    }
}

/// Side of the focus split. Tab rotates between these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// Left pane: node palette.
    Palette,
    /// Center pane: canvas.
    Canvas,
    /// Right pane: inspector / SOP / spec form.
    Inspector,
}

/// Helper: does this key start text input in the current mode?
pub fn is_char_event(k: &KeyEvent) -> bool {
    matches!(
        k.code,
        crossterm::event::KeyCode::Char(_) | crossterm::event::KeyCode::Backspace
    )
}