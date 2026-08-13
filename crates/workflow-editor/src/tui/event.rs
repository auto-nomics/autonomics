//! TUI events and the actor channel.
//!
//! The app spawns an event-loop task that reads `crossterm` events and
//! forwards them through an `mpsc::UnboundedSender<AppEvent>` to the main
//! `App`. Long-running tasks (workflow runs) report progress through the
//! same channel.

use crossterm::event::{KeyEvent, MouseEvent};
use tokio::sync::mpsc;

/// One event the TUI handles.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// A key was pressed.
    Key(KeyEvent),
    /// A mouse event (reserved for Phase 4 hover/click).
    Mouse(MouseEvent),
    /// Terminal was resized.
    Resize(u16, u16),
    /// Periodic tick (for status bar updates + cursor blink).
    Tick,
    /// A workflow run finished; carries the result or error.
    RunFinished(crate::executor::WorkflowResult),
    /// A workflow run errored.
    RunError(String),
}

/// Channel sender half used by the event loop.
pub type AppEventSender = mpsc::UnboundedSender<AppEvent>;
/// Channel receiver half used by the `App`.
pub type AppEventReceiver = mpsc::UnboundedReceiver<AppEvent>;
