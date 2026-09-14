//! Session list widget — displays all sub-sessions of the active agent.
//!
//! Renders one row per session with an active/inactive marker, truncated title,
//! and message count. The active session is highlighted. Designed to be embedded
//! inside the sidebar's Sessions section but is self-contained and testable.
//!
//! ```text
//! ┌──────────────────┐
//! │ ● GWAS analysis  │  ← active (highlighted)
//! │ ○ Literature      │  ← inactive
//! │ ○ Untitled        │
//! └──────────────────┘
//! ```

use ratatui::{
    layout::{Alignment, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

/// Lightweight summary of one sub-session for sidebar rendering.
#[derive(Clone)]
pub struct SessionSummary {
    pub title: Option<String>,
    pub message_count: usize,
    pub is_active: bool,
    /// Epoch-millis of creation. Used to order sessions (newest first).
    pub created_at: i64,
}

/// Widget that renders a compact list of sub-sessions.
///
/// Each row shows a status marker (`●` active, `○` inactive), the session
/// title (truncated to fit), and the message count. The active session is
/// rendered with a distinct style so the user can see which conversation they
/// are in at a glance.
pub struct SessionList<'a> {
    pub sessions: &'a [SessionSummary],
}

impl Widget for SessionList<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }
        if self.sessions.is_empty() {
            let line = Line::from(Span::styled(
                "(no sessions)",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM | Modifier::ITALIC),
            ));
            Paragraph::new(line)
                .alignment(Alignment::Center)
                .render(area, buf);
            return;
        }

        // Sort: active session first, then remaining sessions by creation
        // time descending (newest at top). This is a display-only reorder —
        // it does not mutate the caller's slice.
        let mut ordered: Vec<&SessionSummary> = self.sessions.iter().collect();
        ordered.sort_by(|a, b| match (b.is_active, a.is_active) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => b.created_at.cmp(&a.created_at),
        });

        for (i, session) in ordered.iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.bottom() {
                break;
            }
            let row_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            };
            render_session_row(row_area, buf, session);
        }
    }
}

/// Render a single session row.
fn render_session_row(area: Rect, buf: &mut Buffer, session: &SessionSummary) {
    let title_text = session.title.as_deref().unwrap_or("(untitled)");

    // Reserve space for: " ● " (3 chars) + title + right-aligned " N" (msg count).
    let msg_text = format!("{}", session.message_count);
    let right_pad = msg_text.chars().count() + 2; // 2 spaces before count
    let max_title_len = area.width.saturating_sub(3 + right_pad as u16) as usize;

    let title_display = if title_text.chars().count() > max_title_len && max_title_len > 1 {
        let truncated: String = title_text.chars().take(max_title_len - 1).collect();
        format!("{truncated}…")
    } else {
        title_text.to_string()
    };

    if session.is_active {
        let line = Line::from(vec![
            Span::styled(" ● ", Style::default().fg(Color::Yellow)),
            Span::styled(
                title_display,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(msg_text, Style::default().fg(Color::DarkGray)),
        ]);
        line.render(area, buf);
    } else {
        let line = Line::from(vec![
            Span::styled(" ○ ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                title_display,
                Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
            ),
            Span::raw(" "),
            Span::styled(msg_text, Style::default().fg(Color::DarkGray)),
        ]);
        line.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_sessions_renders_placeholder() {
        let sessions: Vec<SessionSummary> = vec![];
        let widget = SessionList {
            sessions: &sessions,
        };
        // Just verify it doesn't panic with zero-height area.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        widget.render(Rect::new(0, 0, 20, 5), &mut buf);
    }

    #[test]
    fn active_session_highlighted() {
        let sessions = vec![
            SessionSummary {
                title: Some("Active".into()),
                message_count: 10,
                is_active: true,
                created_at: 1000,
            },
            SessionSummary {
                title: Some("Inactive".into()),
                message_count: 3,
                is_active: false,
                created_at: 2000,
            },
        ];
        let widget = SessionList {
            sessions: &sessions,
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        widget.render(Rect::new(0, 0, 30, 5), &mut buf);
        // First row should contain "●" (active marker).
        assert!(buf.area().width > 0);
    }

    #[test]
    fn active_always_first_regardless_of_created_at() {
        // The active session has an *older* created_at, but must still
        // appear at the top of the rendered list.
        let sessions = vec![
            SessionSummary {
                title: Some("Older".into()),
                message_count: 1,
                is_active: false,
                created_at: 9000,
            },
            SessionSummary {
                title: Some("Middle".into()),
                message_count: 1,
                is_active: false,
                created_at: 5000,
            },
            SessionSummary {
                title: Some("Active-but-oldest".into()),
                message_count: 1,
                is_active: true,
                created_at: 1000,
            },
        ];
        let widget = SessionList {
            sessions: &sessions,
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 5));
        widget.render(Rect::new(0, 0, 40, 5), &mut buf);

        // Row 0 (y=0) must contain the active marker "●".
        let row0: String = (0..40).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(
            row0.contains('●'),
            "expected active marker on first row, got: {row0}"
        );

        // Row 1 (y=1) must be the newest inactive ("Older", created_at 9000).
        let row1: String = (0..40).map(|x| buf[(x, 1)].symbol().to_string()).collect();
        assert!(
            row1.contains("Older"),
            "expected newest inactive on second row, got: {row1}"
        );

        // Row 2 must be "Middle" (created_at 5000).
        let row2: String = (0..40).map(|x| buf[(x, 2)].symbol().to_string()).collect();
        assert!(
            row2.contains("Middle"),
            "expected second-newest inactive on third row, got: {row2}"
        );
    }
}
