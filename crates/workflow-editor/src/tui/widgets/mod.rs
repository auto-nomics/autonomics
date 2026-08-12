//! TUI widgets — canvas, palette, inspector, command palette, etc.

#[cfg(feature = "tui")]
pub mod canvas;

#[cfg(feature = "tui")]
pub mod command_palette;

#[cfg(feature = "tui")]
pub mod help_overlay;

#[cfg(feature = "tui")]
pub mod history_panel;

#[cfg(feature = "tui")]
pub mod inspector;

#[cfg(feature = "tui")]
pub mod node_palette;

#[cfg(feature = "tui")]
pub mod sop_editor;

#[cfg(feature = "tui")]
pub mod spec_form;

#[cfg(feature = "tui")]
pub mod status_bar;