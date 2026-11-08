//! Top tab strip: one segment per open agent leaf.
//!
//! Pure view state — tabs are UI windows onto agents, not the agents
//! themselves. Opening, closing, and switching tabs never touches agent
//! lifecycle: under virtual-actor mode the daemon owns instances, and a
//! closed tab's agent simply idles until the daemon passivates it (its
//! history stays in storage either way).
//!
//! When the leaves outnumber the strip width, the strip degrades the way
//! LazyVim's bufferline does: the visible window slides so the **active
//! leaf is always on screen**, and a dim `+N` stub on each clipped edge
//! counts the hidden tabs. Clicking a stub opens the switch-agent picker
//! (Ctrl+T) — the strip stays a pure view, never a lifecycle surface.

use ratatui::layout::Rect;
use ratatui::prelude::Buffer;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::state::{AgentSession, AgentStatus};
use crate::widgets::agent_workspace::{display_name, status_color, status_icon};

/// One tab segment's geometry within the strip — the single layout both
/// the renderer and the mouse hit-test consume, so what you see is what
/// you click.
pub struct TabSegment {
    pub x: u16,
    pub width: u16,
    /// Index into the sessions list.
    pub idx: usize,
}

/// What a click on the strip resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabClick {
    /// Switch to the session at this index.
    Switch(usize),
    /// An overflow stub (`+N`): more leaves than the strip can show.
    Overflow,
}

/// Computed strip layout: the visible window of segments plus the two
/// overflow stubs. Produced by [`layout`]; consumed by rendering and
/// hit-testing so both agree on the geometry.
pub struct TabLayout {
    pub segments: Vec<TabSegment>,
    /// Leaves hidden left/right of the visible window.
    pub left_hidden: usize,
    pub right_hidden: usize,
    /// `(x, width)` of each stub; zero width when that side is not
    /// clipped (or the strip is too narrow to draw stubs at all).
    pub left_indicator: (u16, u16),
    pub right_indicator: (u16, u16),
}

fn session_status(session: &AgentSession) -> AgentStatus {
    session
        .context
        .as_ref()
        .map(|context| context.tab_state.status)
        .unwrap_or(session.pending_tab_state.status)
}

/// Full width of one tab segment, separator column included.
fn tab_width(session: &AgentSession) -> u16 {
    let name = display_name(&session.name);
    1 + 1 + name.chars().count() as u16 + 1 + 1 // pad + icon + name + pad + sep
}

/// Width of a ` +N` stub; zero when nothing is hidden on that side.
fn indicator_width(hidden: usize) -> u16 {
    if hidden == 0 {
        0
    } else {
        hidden.to_string().len() as u16 + 2 // space + '+' + digits
    }
}

/// Compute the strip layout for `bar_width`.
///
/// The visible window starts at leaf 0. When the leaves overflow, stub
/// width is reserved on each clipped edge and the window slides right
/// just far enough to keep `active_idx` on screen. On pathologically
/// narrow strips (no room for both stubs and one segment) the layout
/// degrades to plain left-anchored truncation.
pub fn layout(sessions: &[AgentSession], bar_width: u16, active_idx: usize) -> TabLayout {
    let len = sessions.len();
    let active = active_idx.min(len.saturating_sub(1));
    let widths: Vec<u16> = sessions.iter().map(tab_width).collect();

    // How many consecutive segments from `start` fit into `avail`.
    let fit_from = |start: usize, avail: u16| -> usize {
        let mut x = 0u16;
        let mut n = 0usize;
        for idx in start..len {
            if x.saturating_add(widths[idx]) > avail {
                break;
            }
            x += widths[idx];
            n += 1;
        }
        n
    };

    let plain = if len == 0 { 0 } else { fit_from(0, bar_width) };

    let (mut start, mut shown) = (0usize, plain);
    let (mut left, mut right) = (0usize, 0usize);
    let mut stubs_fit = true;

    if plain < len {
        // Iterate to a fixed point: stub widths depend on the hidden
        // counts, which depend on the reserved widths. `start` only ever
        // advances (bounded by `len`), so the loop converges; the bound
        // is a safety net, not the expected exit.
        let bound = len.saturating_mul(2).saturating_add(4);
        for _ in 0..bound {
            let (lw, rw) = (indicator_width(left), indicator_width(right));
            let Some(avail) = bar_width.checked_sub(lw + rw) else {
                stubs_fit = false;
                break;
            };
            let mut n = fit_from(start, avail);
            if n > 0 && active >= start + n {
                // Slide the window so the active leaf is the last
                // visible segment (bufferline's keep-active-visible).
                start = active + 1 - n;
                n = fit_from(start, avail);
            }
            shown = n;
            let counts = (start, len - start - n);
            if counts == (left, right) {
                break;
            }
            (left, right) = counts;
        }
        // A stub that crowds out every segment is worse than no stub —
        // fall back to plain truncation.
        if shown == 0 && plain > 0 {
            stubs_fit = false;
        }
    }

    if !stubs_fit {
        (start, shown) = (0, plain);
        (left, right) = (0, 0);
    }

    let lw = if stubs_fit { indicator_width(left) } else { 0 };
    let rw = if stubs_fit { indicator_width(right) } else { 0 };

    let mut segments = Vec::with_capacity(shown);
    let mut x = lw;
    for idx in start..start + shown {
        segments.push(TabSegment {
            x,
            width: widths[idx],
            idx,
        });
        x += widths[idx];
    }

    TabLayout {
        segments,
        left_hidden: left,
        right_hidden: right,
        left_indicator: if lw > 0 { (0, lw) } else { (0, 0) },
        right_indicator: if rw > 0 { (bar_width - rw, rw) } else { (0, 0) },
    }
}

/// Resolve a click at `column` on the strip into a tab switch or an
/// overflow-stub hit. The same [`layout`] the renderer used produces the
/// geometry, so the click lands on exactly what was drawn.
pub fn click(
    sessions: &[AgentSession],
    bar_width: u16,
    active_idx: usize,
    column: u16,
) -> Option<TabClick> {
    if sessions.is_empty() || column >= bar_width {
        return None;
    }
    let lay = layout(sessions, bar_width, active_idx);
    let (lx, lw) = lay.left_indicator;
    if lw > 0 && column >= lx && column < lx + lw {
        return Some(TabClick::Overflow);
    }
    let (rx, rw) = lay.right_indicator;
    if rw > 0 && column >= rx && column < rx + rw {
        return Some(TabClick::Overflow);
    }
    lay.segments
        .iter()
        .find(|segment| column >= segment.x && column < segment.x + segment.width)
        .map(|segment| TabClick::Switch(segment.idx))
}

/// Render the strip. The active tab is bold on an accent background;
/// every tab carries its agent's lifecycle status icon in status color;
/// clipped edges show a dim `+N` overflow stub.
pub struct TabBar<'a> {
    pub sessions: &'a [AgentSession],
    pub active_idx: usize,
}

impl Widget for TabBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let theme = crate::theme::current();
        let strip = Style::default().bg(theme.strip_bg);
        for column in area.x..area.right() {
            buf[(column, area.y)].set_style(strip).set_symbol(" ");
        }

        let lay = layout(self.sessions, area.width, self.active_idx);
        let dim = Style::default().fg(theme.text_faint).bg(theme.strip_bg);

        if lay.left_indicator.1 > 0 {
            let line = Line::styled(format!(" +{}", lay.left_hidden), dim);
            buf.set_line(
                area.x + lay.left_indicator.0,
                area.y,
                &line,
                lay.left_indicator.1,
            );
        }

        for segment in &lay.segments {
            let session = &self.sessions[segment.idx];
            let status = session_status(session);
            let active = segment.idx == self.active_idx;
            let name = display_name(&session.name);
            let base = if active {
                Style::default()
                    .bg(theme.strip_active)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_faint).bg(theme.strip_bg)
            };
            let line = Line::from(vec![
                Span::styled(" ", base),
                Span::styled(
                    status_icon(&status).to_string(),
                    base.fg(status_color(&status)),
                ),
                Span::styled(format!(" {name} "), base),
                Span::styled("│", strip.fg(theme.text_faint)),
            ]);
            buf.set_line(area.x + segment.x, area.y, &line, segment.width);
        }

        if lay.right_indicator.1 > 0 {
            let line = Line::styled(format!(" +{}", lay.right_hidden), dim);
            buf.set_line(
                area.x + lay.right_indicator.0,
                area.y,
                &line,
                lay.right_indicator.1,
            );
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

    fn names(name: &str) -> usize {
        display_name(name).chars().count()
    }

    #[test]
    fn all_tabs_fit_no_stubs() {
        let sessions = vec![
            session("/root/alpha", AgentStatus::Idle),
            session("/root/beta", AgentStatus::Streaming),
        ];
        let lay = layout(&sessions, 200, 0);
        assert_eq!(lay.segments.len(), 2);
        assert_eq!(lay.segments[0].x, 0);
        assert_eq!(lay.segments[1].x, lay.segments[0].width);
        assert_eq!(lay.left_hidden, 0);
        assert_eq!(lay.right_hidden, 0);
        assert_eq!(lay.left_indicator.1, 0);
        assert_eq!(lay.right_indicator.1, 0);
        assert_eq!(click(&sessions, 200, 0, 0), Some(TabClick::Switch(0)));
        assert_eq!(
            click(&sessions, 200, 0, lay.segments[1].x),
            Some(TabClick::Switch(1))
        );
        assert_eq!(
            click(&sessions, 200, 0, lay.segments[1].x + lay.segments[1].width),
            None
        );
    }

    #[test]
    fn overflow_shows_right_stub_and_reserves_width() {
        // "alpha" is 9 wide, "beta" doesn't fit; a 12-col strip holds the
        // segment plus the 3-wide ` +1` stub right-aligned.
        let sessions = vec![
            session("/root/alpha", AgentStatus::Idle),
            session("/root/beta", AgentStatus::Idle),
        ];
        let lay = layout(&sessions, 12, 0);
        assert_eq!(lay.segments.len(), 1);
        assert_eq!(lay.right_hidden, 1);
        let (rx, rw) = lay.right_indicator;
        assert_eq!(rw, indicator_width(1));
        assert_eq!(rx + rw, 12); // right-aligned at the strip edge
        assert_eq!(click(&sessions, 12, 0, rx), Some(TabClick::Overflow));
        assert_eq!(
            click(&sessions, 12, 0, rx + rw - 1),
            Some(TabClick::Overflow)
        );
    }

    #[test]
    fn active_leaf_is_always_visible() {
        // Five 6-wide leaves (`/root/a`..`/root/e`), strip fits three
        // plus stubs. Focus on the last leaf: the window must slide so
        // it stays on screen, with a left stub counting the skipped tabs.
        let sessions: Vec<_> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|n| session(&format!("/root/{n}"), AgentStatus::Idle))
            .collect();
        let seg_w = (1 + 1 + names("/root/a") + 1 + 1) as u16;

        // San-check the fixture: three segments + stubs fit in 3*6+6.
        let bar = seg_w * 3 + 6;
        let lay = layout(&sessions, bar, 4);
        let visible: Vec<usize> = lay.segments.iter().map(|s| s.idx).collect();
        assert!(
            visible.contains(&4),
            "active leaf must be visible: {visible:?}"
        );
        assert_eq!(lay.left_hidden, 2);
        assert_eq!(lay.right_hidden, 0);
        assert_eq!(lay.left_indicator.1, indicator_width(2));
        // Clicking the left stub opens the picker, not a phantom tab.
        assert_eq!(click(&sessions, bar, 4, 0), Some(TabClick::Overflow));
        // Segments still hit-test to their true session indices.
        assert_eq!(
            click(&sessions, bar, 4, lay.segments.last().unwrap().x),
            Some(TabClick::Switch(4))
        );
    }

    #[test]
    fn narrow_strip_degrades_to_plain_truncation() {
        // Strip too narrow for segment + stub: keep at least one segment
        // and draw no stubs (matches the pre-stub behaviour).
        let sessions = vec![
            session("/root/alpha", AgentStatus::Idle),
            session("/root/beta-with-a-really-long-name", AgentStatus::Idle),
        ];
        let lay = layout(&sessions, 10, 0);
        assert_eq!(lay.segments.len(), 1);
        assert_eq!(lay.segments[0].idx, 0);
        assert_eq!(lay.left_indicator.1, 0);
        assert_eq!(lay.right_indicator.1, 0);
    }

    #[test]
    fn empty_sessions_lay_out_empty() {
        let lay = layout(&[], 80, 0);
        assert!(lay.segments.is_empty());
        assert_eq!(click(&[], 80, 0, 0), None);
    }
}
