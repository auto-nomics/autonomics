//! Left-sidebar widget for the Agent tab.
//!
//! Two independently-bordered blocks stacked vertically, each taking up to
//! half the available height and scrolling internally when content overflows:
//! - **Running** — active agent sessions with live status indicators
//! - **Profiles** — available blueprints the user can spawn from

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::state::AgentStatus;

/// Which sub-list within the sidebar has keyboard focus for navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarSection {
    #[default]
    Agents,
    Profiles,
}

/// Data needed to render the sidebar. All owned to avoid borrow conflicts
/// with the chat panel.
pub struct AgentSidebar {
    pub profile_names: Vec<(String, String)>, // (name, description)
    pub session_names: Vec<(String, AgentStatus)>,
    pub active_idx: usize,
    pub profile_selected: usize,
    /// Which section is currently navigable.
    pub section: SidebarSection,
    pub focused: bool,
}

impl Widget for AgentSidebar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let hint_height = 2u16;

        Block::default()
            .style(Style::default().bg(Color::Black))
            .render(area, buf);

        // Split into: agents block (up to 50%), profiles block (up to 50%),
        // footer hint.
        let regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(50), // Running agents
                Constraint::Percentage(50), // Profiles
                Constraint::Length(hint_height),
            ])
            .split(area);

        self.render_agents_block(regions[0], buf);
        self.render_profiles_block(regions[1], buf);
        self.render_hint(regions[2], buf);
    }
}

impl AgentSidebar {
    fn render_agents_block(&self, area: Rect, buf: &mut Buffer) {
        let is_active_section = self.focused && self.section == SidebarSection::Agents;
        let border_style = if is_active_section {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let title = if is_active_section {
            " Running ◀ "
        } else {
            " Running "
        };

        let block = Block::default()
            // .borders(Borders::TOP | Borders::BOTTOM)
            .title(Span::styled(
                title,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(border_style);
        let inner = block.inner(area);
        block.render(area, buf);

        if self.session_names.is_empty() {
            Paragraph::new("  (no agents running)")
                .style(Style::default().fg(Color::DarkGray))
                .render(inner, buf);
            return;
        }

        let items: Vec<ListItem> = self
            .session_names
            .iter()
            .enumerate()
            .map(|(i, (name, status))| {
                let (icon, color) = match status {
                    AgentStatus::Idle => ("●", Color::Green),
                    AgentStatus::Requesting => ("◐", Color::Cyan),
                    AgentStatus::Streaming => ("◑", Color::Blue),
                    AgentStatus::Error => ("✗", Color::Red),
                };
                let is_active = i == self.active_idx;
                let prefix = if is_active && is_active_section {
                    "▶ "
                } else {
                    "  "
                };
                let style = if is_active {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                ListItem::new(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(icon, Style::default().fg(color)),
                    Span::raw(" "),
                    Span::styled(name, style),
                ]))
            })
            .collect();

        // Ratatui's List auto-scrolls to keep the selected item visible.
        let mut state = ListState::default();
        if is_active_section {
            state.select(Some(self.active_idx));
        }
        StatefulWidget::render(List::new(items), inner, buf, &mut state);
    }

    fn render_profiles_block(&self, area: Rect, buf: &mut Buffer) {
        let is_active_section = self.focused && self.section == SidebarSection::Profiles;
        let border_style = if is_active_section {
            Style::default().fg(Color::Blue)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let title = if is_active_section {
            " Profiles ◀ "
        } else {
            " Profiles "
        };

        let block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .title(Span::styled(
                title,
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(border_style);
        let inner = block.inner(area);
        block.render(area, buf);

        if self.profile_names.is_empty() {
            Paragraph::new("  (no profiles)")
                .style(Style::default().fg(Color::DarkGray))
                .render(inner, buf);
            return;
        }

        let mut items: Vec<ListItem> = Vec::new();
        for (i, (name, desc)) in self.profile_names.iter().enumerate() {
            let is_selected = i == self.profile_selected && is_active_section;
            let prefix = if is_selected { "▶ " } else { "  " };
            let style = if is_selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };

            items.push(ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled("+ ", Style::default().fg(Color::DarkGray)),
                Span::styled(name, style),
            ])));

            if is_selected && !desc.is_empty() {
                items.push(ListItem::new(Line::from(vec![Span::styled(
                    format!("    {}", desc),
                    Style::default().fg(Color::DarkGray),
                )])));
            }
        }

        // Ratatui's List auto-scrolls to keep the selected item visible.
        let mut state = ListState::default();
        if is_active_section {
            state.select(Some(self.profile_selected));
        }
        StatefulWidget::render(List::new(items), inner, buf, &mut state);
    }

    fn render_hint(&self, area: Rect, buf: &mut Buffer) {
        let hint = match self.section {
            SidebarSection::Agents => " Enter switch  ↑↓ navigate",
            SidebarSection::Profiles => " Enter spawn  ↑↓ navigate",
        };
        Paragraph::new(hint)
            .style(
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )
            .alignment(Alignment::Left)
            .render(area, buf);
    }
}
