//! TUI editor — three-pane layout (palette / canvas / inspector) over
//! `ratatui`, with mode-aware key bindings and undo / redo.
//!
//! Entry point: [`App::new`] + [`App::run`].

#[cfg(feature = "tui")]
pub mod app;

#[cfg(feature = "tui")]
pub mod event;

#[cfg(feature = "tui")]
pub mod keymap;

#[cfg(feature = "tui")]
pub mod layout;

#[cfg(feature = "tui")]
pub mod mode;

#[cfg(feature = "tui")]
pub mod state;

#[cfg(feature = "tui")]
pub mod widgets;

#[cfg(feature = "tui")]
pub use app::App;
#[cfg(feature = "tui")]
pub use event::{AppEvent, AppEventReceiver, AppEventSender};
#[cfg(feature = "tui")]
pub use layout::PaneLayout;
#[cfg(feature = "tui")]
pub use mode::{Focus, Mode};
#[cfg(feature = "tui")]
pub use state::{AppState, CommandKind, UndoEntry};