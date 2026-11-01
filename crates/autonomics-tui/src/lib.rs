//! The `autonomics` interactive TUI frontend, as a standalone library.
//!
//! Extracted from the `autonomics` binary crate: this crate owns the
//! ratatui application (`app`), all widgets, the `xai_textarea` editor,
//! the KMS browser TUI, and the shared UI state/event plumbing. The
//! `autonomics` binary keeps the CLI, the one-shot commands, the daemon
//! (`serve`) and the readiness diagnostics, and calls into this crate
//! for everything terminal-interactive.
//!
//! Module names are unchanged from the original binary layout, so
//! internal `crate::` paths needed no rewriting.

pub mod app;
pub mod app_event;
pub mod app_event_sender;
pub mod clipboard_copy;
pub mod kms_tui;
pub mod state;
pub mod widgets;
pub mod xai_textarea;

/// Launch the interactive TUI (the `autonomics tui` subcommand body).
pub fn run_tui() -> color_eyre::Result<()> {
    let mut app = app::App::new()?;
    app.start()
}
