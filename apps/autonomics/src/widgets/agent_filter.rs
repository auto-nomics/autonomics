//! Search and lifecycle tabs shared by the docked agents list and its keys.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::state::AgentStatus;
use crate::widgets::agent_workspace::{display_name, status_color, status_icon};
use crate::widgets::sidebar::RunningAgent;

const TABS: [(&str, Option<AgentStatus>); 10] = [
    ("All", None),
    ("Idle", Some(AgentStatus::Idle)),
    ("Waiting", Some(AgentStatus::Waiting)),
    ("Requesting", Some(AgentStatus::Requesting)),
    ("Streaming", Some(AgentStatus::Streaming)),
    ("Tools", Some(AgentStatus::ToolRunning)),
    ("Retrying", Some(AgentStatus::Retrying)),
    ("Compacting", Some(AgentStatus::Compacting)),
    ("Error", Some(AgentStatus::Error)),
    ("Cancelled", Some(AgentStatus::Cancelled)),
];

#[derive(Default)]
pub struct AgentFilterState {
    pub query: String,
    pub searching: bool,
    status_tab: usize,
    tab_areas: Vec<(Rect, usize)>,
    search_area: Option<Rect>,
    counts: Option<Vec<usize>>,
}

impl AgentFilterState {
    fn matches_status(&self, status: AgentStatus) -> bool {
        matches_tab(self.status_tab, status)
    }

    pub fn matches(&self, agent: &RunningAgent) -> bool {
        let query = self.query.trim().to_lowercase();
        self.matches_status(agent.status)
            && (query.is_empty()
                || agent.name.to_lowercase().contains(&query)
                || display_name(&agent.name).to_lowercase().contains(&query)
                // The model column is searchable too — `/glm` keeps every
                // agent currently on that model, including bulk targets for
                // M/E. Effort stays unsearchable: `–`/blank is noise.
                || agent
                    .model
                    .as_deref()
                    .is_some_and(|spec| spec.to_lowercase().contains(&query)))
    }

    /// A tab takes part in navigation while it renders: All (the total),
    /// every status with agents, and the selected tab, which never folds.
    /// Until the first render the counts are unknown — every tab then
    /// counts as reachable so keyboard state stays total.
    fn tab_reachable(&self, tab: usize) -> bool {
        match &self.counts {
            None => true,
            Some(counts) => tab == 0 || counts[tab] > 0 || tab == self.status_tab,
        }
    }

    /// Nearest reachable neighbour in the requested direction, wrapping
    /// around the ends; the origin itself when nothing else renders.
    fn step_from(&self, tab: usize, forward: bool) -> usize {
        let len = TABS.len();
        let mut candidate = tab;
        for _ in 0..len {
            candidate = if forward {
                (candidate + 1) % len
            } else {
                (candidate + len - 1) % len
            };
            if self.tab_reachable(candidate) {
                return candidate;
            }
        }
        tab
    }

    /// Returns true when the agents search/status controls consumed the key.
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return false;
        }
        match key.code {
            KeyCode::Left | KeyCode::BackTab => {
                self.status_tab = self.step_from(self.status_tab, false)
            }
            KeyCode::Right | KeyCode::Tab => {
                self.status_tab = self.step_from(self.status_tab, true)
            }
            KeyCode::Char('/') if !self.searching => self.searching = true,
            KeyCode::Char(c) if self.searching => self.query.push(c),
            KeyCode::Backspace if self.searching => {
                self.query.pop();
            }
            KeyCode::Enter if self.searching => self.searching = false,
            KeyCode::Esc if self.searching || !self.query.is_empty() => {
                self.query.clear();
                self.searching = false;
            }
            _ => return false,
        }
        true
    }

    pub fn clear_hit_areas(&mut self) {
        self.tab_areas.clear();
        self.search_area = None;
    }

    /// Click controls without switching or closing an agent.
    pub fn click(&mut self, x: u16, y: u16) -> bool {
        let contains =
            |area: Rect| x >= area.x && x < area.right() && y >= area.y && y < area.bottom();
        if self.search_area.is_some_and(contains) {
            self.searching = true;
            return true;
        }
        if let Some(tab) = self
            .tab_areas
            .iter()
            .find_map(|(area, tab)| contains(*area).then_some(*tab))
        {
            self.status_tab = tab;
            return true;
        }
        false
    }

    /// Two control rows; the caller renders the filtered agents below them.
    /// Returns the number of rows actually used (fewer on tiny areas).
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        agents: &[RunningAgent],
        focused: bool,
    ) -> u16 {
        self.clear_hit_areas();
        if area.is_empty() {
            return 0;
        }
        let search = Rect { height: 1, ..area };
        self.search_area = Some(search);
        let label = if self.query.is_empty() && !self.searching {
            " / search agents".to_string()
        } else {
            format!(
                " /{}{}",
                self.query,
                if self.searching && focused { "▏" } else { "" }
            )
        };
        Paragraph::new(label)
            .style(Style::default().fg(if self.searching && focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }))
            .render(search, buf);
        if area.height < 2 || area.width < 5 {
            return 1;
        }
        let tabs = Rect {
            y: area.y + 1,
            height: 1,
            ..area
        };
        let viewport = Rect {
            x: tabs.x + 1,
            width: tabs.width - 2,
            ..tabs
        };
        // Captions reuse the agent-row status glyphs, so they are
        // multi-byte: every offset below counts terminal cells
        // (unicode_width), never bytes, keeping hit areas and the scroll
        // glued to what Paragraph renders.
        let selected_style = Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let mut captions: Vec<(Vec<Span<'static>>, usize, usize)> = Vec::with_capacity(TABS.len());
        let mut counts = vec![0usize; TABS.len()];
        for (tab, (name, status)) in TABS.iter().enumerate() {
            let count = agents
                .iter()
                .filter(|agent| matches_tab(tab, agent.status))
                .count();
            counts[tab] = count;
            let selected = tab == self.status_tab;
            // Resting captions stay compact (" ●7 "); the selected tab
            // expands to name its glyph (" ⚙ Tools 1 ") so the vocabulary
            // keeps teaching itself. All is not a status: it rests as the
            // bare total (" 11 ") and gains the word only while selected.
            // Empty statuses fold out of the row entirely so the live tabs
            // fill the viewport; navigation hops between the tabs that
            // render (see tab_reachable), wrapping around the ends.
            if count == 0 && status.is_some() && !selected {
                continue;
            }
            let text_style = if selected {
                selected_style
            } else {
                Style::default().fg(Color::Gray)
            };
            let glyph_style = match (selected, status) {
                (true, _) => selected_style,
                (false, Some(status)) => Style::default().fg(status_color(status)),
                (false, None) => text_style,
            };
            let mut pieces: Vec<(String, Style)> = vec![(" ".into(), text_style)];
            if let Some(status) = status {
                pieces.push((status_icon(status).into(), glyph_style));
            }
            if selected {
                pieces.push((" ".into(), selected_style));
                pieces.push(((*name).into(), selected_style));
                pieces.push((" ".into(), selected_style));
            }
            pieces.push((count.to_string(), text_style));
            pieces.push((" ".into(), text_style));
            let width = pieces.iter().map(|(piece, _)| piece.width()).sum::<usize>();
            captions.push((
                pieces
                    .into_iter()
                    .map(|(piece, style)| Span::styled(piece, style))
                    .collect(),
                width,
                tab,
            ));
        }
        let selected_pos = captions
            .iter()
            .position(|&(_, _, tab)| tab == self.status_tab)
            .expect("the selected tab never folds");
        self.counts = Some(counts);
        let end: usize = captions[..=selected_pos]
            .iter()
            .map(|(_, width, _)| *width)
            .sum();
        let offset = end.saturating_sub(viewport.width as usize);
        let mut column = 0;
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (tab_spans, width, tab) in captions {
            let end = column + width;
            let left = column.max(offset);
            let right = end.min(offset + viewport.width as usize);
            if left < right {
                self.tab_areas.push((
                    Rect::new(
                        viewport.x + (left - offset) as u16,
                        viewport.y,
                        (right - left) as u16,
                        1,
                    ),
                    tab,
                ));
            }
            column = end;
            spans.extend(tab_spans);
        }
        Paragraph::new(Line::from(spans))
            .scroll((0, offset as u16))
            .render(viewport, buf);
        if offset > 0 {
            let arrow = Rect::new(tabs.x, tabs.y, 1, 1);
            Paragraph::new("‹").render(arrow, buf);
            self.tab_areas
                .push((arrow, self.step_from(self.status_tab, false)));
        }
        if column > offset + viewport.width as usize {
            let arrow = Rect::new(tabs.right() - 1, tabs.y, 1, 1);
            Paragraph::new("›").render(arrow, buf);
            self.tab_areas
                .push((arrow, self.step_from(self.status_tab, true)));
        }
        2
    }
}

fn matches_tab(tab: usize, status: AgentStatus) -> bool {
    match TABS[tab].1 {
        None => true,
        Some(AgentStatus::Idle) => status.is_idle(),
        Some(expected) => status == expected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str, status: AgentStatus) -> RunningAgent {
        RunningAgent {
            name: name.into(),
            status,
            agent_id: uuid::Uuid::new_v4(),
            is_active: false,
            model: None,
            effort: None,
        }
    }

    fn key(filter: &mut AgentFilterState, code: KeyCode) {
        assert!(filter.handle_key(&KeyEvent::new(code, KeyModifiers::NONE)));
    }

    #[test]
    fn search_matches_model_specs() {
        let mut filter = AgentFilterState::default();
        let mut on_glm = agent("/root/mrine", AgentStatus::Idle);
        on_glm.model = Some("zai:glm-5.3".into());
        key(&mut filter, KeyCode::Char('/'));
        for c in "glm".chars() {
            key(&mut filter, KeyCode::Char(c));
        }
        // Matches the spec's model part (bulk M/E target selection relies
        // on this), and the provider part narrows the same way.
        assert!(filter.matches(&on_glm));
        assert!(!filter.matches(&agent("/root/mrine", AgentStatus::Idle)));
        key(&mut filter, KeyCode::Backspace);
        key(&mut filter, KeyCode::Backspace);
        key(&mut filter, KeyCode::Backspace);
        for c in "zai".chars() {
            key(&mut filter, KeyCode::Char(c));
        }
        assert!(filter.matches(&on_glm));
    }

    #[test]
    fn search_and_tabs_intersect_and_follow_live_status() {
        let mut filter = AgentFilterState::default();
        let streaming = agent("/root/w1", AgentStatus::Streaming);
        // Prime the tab counts with a render pass so empty tabs fold and
        // navigation hops between the tabs that actually render.
        let mut buf = ratatui::prelude::Buffer::empty(ratatui::layout::Rect::new(0, 0, 60, 3));
        filter.render(
            ratatui::layout::Rect::new(0, 0, 60, 3),
            &mut buf,
            std::slice::from_ref(&streaming),
            false,
        );
        key(&mut filter, KeyCode::Char('/'));
        for c in "w1".chars() {
            key(&mut filter, KeyCode::Char(c));
        }
        key(&mut filter, KeyCode::Enter);
        // Right-arrow hops to the first tab with agents — Streaming.
        key(&mut filter, KeyCode::Right);
        assert!(filter.matches(&streaming));
        assert!(!filter.matches(&agent("/root/other", AgentStatus::Streaming)));
    }

    #[test]
    fn idle_tab_groups_idle_and_aborted_like_statuses() {
        let mut filter = AgentFilterState::default();
        // Select the Idle tab (index 1) via repeated right steps; with only
        // idle agents present, every status tab but Idle folds.
        key(&mut filter, KeyCode::Right);
        assert!(filter.matches(&agent("a", AgentStatus::Idle)));
        assert!(!filter.matches(&agent("a", AgentStatus::Error)));
    }
}
