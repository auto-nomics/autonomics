//! Stateful bibliography workspace widget.
//!
//! The module is split by responsibility:
//!
//! - [`state`]: owned data, selection, focus, and pending actions
//! - [`input`]: keyboard event translation
//! - [`query`]: article filtering
//! - [`render`] and [`text`]: Ratatui layout and formatted content
//! - [`widget`]: the public `StatefulWidget` implementation

mod input;
mod query;
mod render;
mod state;
mod text;
mod widget;

#[cfg(test)]
mod tests;

pub use state::{BibliographyAction, BibliographyFocus, BibliographyState};
pub use widget::BibliographyWidget;
