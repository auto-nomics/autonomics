//! Top tab strip: one segment per open agent leaf.
//!
//! Pure view state — tabs are UI windows onto agents, not the agents
//! themselves. Opening, closing, and switching tabs never touches agent
//! lifecycle: under virtual-actor mode the daemon owns instances, and a
//! closed tab's agent simply idles until the daemon passivates it (its
//! history stays in storage either way).

use ratatui::layout::Rect;
use ratatui::prelude::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::state::{AgentSession, AgentStatus};
use crate::widgets::agent_workspace::{display_name, status_color, status_icon};

const STRIP_BG: Color = Color::Rgb(24, 24, 28);
const ACTIVE_BG: Color = Color::Rgb(48, 48, 58);

/// One tab segment's geometry within the strip — the single layout both
/// the renderer and the mouse hit-test consume, so what you see is what
/// you click.
pub struct TabSegment {
    pub x: u16,
    pub width: u16,
    /// Index into the sessions list.
    pub idx: usize,
}

fn session_status(session: &AgentSession) -> AgentStatus {
    session
        .context
        .as_ref()
        .map(|context| context.tab_state.status)
        .unwrap_or(session.pending_tab_state.status)
}

/// Compute the tab segments that fit into `bar_width` (separator column
/// included in each width).
pub fn segments(sessions: &[AgentSession], bar_width: u16) -> Vec<TabSegment> {
    let mut out = Vec::new();
    let mut x = 0u16;
    for (idx, session) in sessions.iter().enumerate() {
        let name = display_name(&session.name);
        let width = 1 + 1 + name.chars().count() as u16 + 1 + 1; // pad + icon + name + pad + sep
        if x + width > bar_width {
            break;
        }
        out.push(TabSegment { x, width, idx });
        x += width;
    }
    out
}

/// Hit-test a click at `column` on the strip; returns the session index.
pub fn hit_test(sessions: &[AgentSession], bar_width: u16, column: u16) -> Option<usize> {
    segments(sessions, bar_width)
        .into_iter()
        .find(|segment| column >= segment.x && column < segment.x + segment.width)
        .map(|segment| segment.idx)
}

/// Render the strip. The active tab is bold on an accent background;
/// every tab carries its agent's lifecycle status icon in status color.
pub struct TabBar<'a> {
    pub sessions: &'a [AgentSession],
    pub active_idx: usize,
}

impl Widget for TabBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let strip = Style::default().bg(STRIP_BG);
        for column in area.x..area.right() {
            buf[(column, area.y)].set_style(strip).set_symbol(" ");
        }

        for segment in segments(self.sessions, area.width) {
            let session = &self.sessions[segment.idx];
            let status = session_status(session);
            let active = segment.idx == self.active_idx;
            let name = display_name(&session.name);
            let base = if active {
                Style::default().bg(ACTIVE_BG).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray).bg(STRIP_BG)
            };
            let line = Line::from(vec![
                Span::styled(" ", base),
                Span::styled(
                    status_icon(&status).to_string(),
                    base.fg(status_color(&status)),
                ),
                Span::styled(format!(" {name} "), base),
                Span::styled("│", strip.fg(Color::DarkGray)),
            ]);
            buf.set_line(area.x + segment.x, area.y, &line, segment.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AgentTabState;

    fn session(name: &str, status: AgentStatus) -> AgentSession {
        let mut session = AgentSession {
            name: name.to_string(),
            agent_id: uuid::Uuid::new_v4(),
            context: None,
            pending_tab_state: AgentTabState::default(),
        };
        session.pending_tab_state.status = status;
        session
    }

    #[test]
    fn segments_fit_and_hit_test_round_trips() {
        let sessions = vec![
            session("/root/alpha", AgentStatus::Idle),
            session("/root/beta", AgentStatus::Streaming),
        ];
        let segs = segments(&sessions, 200);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].x, 0);
        assert_eq!(segs[1].x, segs[0].width);
        assert_eq!(hit_test(&sessions, 200, 0), Some(0));
        assert_eq!(hit_test(&sessions, 200, segs[1].x), Some(1));
        assert_eq!(hit_test(&sessions, 200, segs[1].x + segs[1].width), None);
    }

    #[test]
    fn segments_stop_at_the_strip_edge() {
        let sessions = vec![
            session("/root/alpha", AgentStatus::Idle),
            session("/root/beta-with-a-really-long-name", AgentStatus::Idle),
        ];
        assert_eq!(segments(&sessions, 10).len(), 1);
    }
}
