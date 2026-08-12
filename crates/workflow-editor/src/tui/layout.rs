//! Three-pane layout (palette / canvas / inspector) + status bar.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Result of splitting the editor area into regions.
#[derive(Debug, Clone, Copy)]
pub struct PaneLayout {
    /// Whole area, equal to the input.
    pub root: Rect,
    /// Top: workflow header (title + dirty marker).
    pub header: Rect,
    /// Middle row (palette + canvas + inspector).
    pub body: Rect,
    /// Bottom: status / hint bar.
    pub status: Rect,
    /// Body split into [palette, canvas, inspector].
    pub palette: Rect,
    pub canvas: Rect,
    pub inspector: Rect,
}

impl PaneLayout {
    /// Split `area` into the standard three-pane + header + status layout.
    /// Width allocations: 22 columns for palette, 24 for inspector, the
    /// rest for canvas.
    pub fn new(area: Rect) -> Self {
        let root = area;
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Min(3),    // body
                Constraint::Length(1), // status
            ])
            .split(root);
        let body_cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(22),
                Constraint::Min(20),
                Constraint::Length(28),
            ])
            .split(chunks[1]);
        Self {
            root,
            header: chunks[0],
            body: chunks[1],
            status: chunks[2],
            palette: body_cols[0],
            canvas: body_cols[1],
            inspector: body_cols[2],
        }
    }
}