//! Profile picker — standalone widget for selecting an agent profile.
//!
//! Two-column layout: a searchable profile list on the left, and a preview
//! pane on the right showing the selected profile's configuration details.
//! Self-contained (state + rendering + key handling all live here).

use agentik_core::AgentProfile;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget},
};

use crate::widgets::popup::Popup;

/// One selectable profile entry, carrying the full [`AgentProfile`].
#[derive(Clone)]
pub struct ProfileItem {
    pub profile: AgentProfile,
}

impl ProfileItem {
    pub fn name(&self) -> &str {
        &self.profile.name
    }
    pub fn description(&self) -> &str {
        &self.profile.description
    }
}

/// State for the profile picker.
#[derive(Default)]
pub struct ProfilePickerState {
    pub visible: bool,
    /// Search query string.
    pub query: String,
    items: Vec<ProfileItem>,
    filtered: Vec<usize>,
    selected: usize,
    list_state: ListState,
}

impl ProfilePickerState {
    pub fn open(&mut self) {
        self.visible = true;
        self.query.clear();
        self.selected = 0;
        self.refilter();
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
        self.sync_list_state();
    }

    pub fn move_down(&mut self) {
        let max = self.filtered.len().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
        self.sync_list_state();
    }

    /// Returns a clone of the currently selected item, if any.
    pub fn selected_item(&self) -> Option<ProfileItem> {
        let &idx = self.filtered.get(self.selected)?;
        self.items.get(idx).cloned()
    }

    /// Number of items currently displayed (post-filter).
    pub fn filtered_len(&self) -> usize {
        self.filtered.len()
    }

    /// Populate the picker from profiles.
    pub fn set_profiles(&mut self, profiles: Vec<AgentProfile>) {
        self.items = profiles
            .into_iter()
            .map(|p| ProfileItem { profile: p })
            .collect();
        self.refilter();
    }

    fn sync_list_state(&mut self) {
        self.list_state.select(
            if self.filtered.is_empty() || self.selected >= self.filtered.len() {
                None
            } else {
                Some(self.selected)
            },
        );
    }

    fn refilter(&mut self) {
        let needle = self.query.trim().to_lowercase();
        if needle.is_empty() {
            self.filtered = (0..self.items.len()).collect();
        } else {
            self.filtered = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    item.profile.name.to_lowercase().contains(&needle)
                        || item.profile.description.to_lowercase().contains(&needle)
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.selected = 0;
        self.sync_list_state();
    }
}

/// Standalone widget that renders the profile picker as a two-column popup.
pub struct ProfilePicker {
    pub accent: Color,
    /// Width of the popup (0 = auto, ~80% of frame).
    pub popup_width: u16,
    /// Width of the left list block.
    pub list_width: u16,
}

impl ProfilePicker {
    pub fn new() -> Self {
        Self {
            accent: Color::Blue,
            popup_width: 0,
            list_width: 28,
        }
    }

    pub fn accent(mut self, c: Color) -> Self {
        self.accent = c;
        self
    }

    pub fn popup_width(mut self, w: u16) -> Self {
        self.popup_width = w;
        self
    }

    pub fn list_width(mut self, w: u16) -> Self {
        self.list_width = w;
        self
    }
}

impl Default for ProfilePicker {
    fn default() -> Self {
        Self::new()
    }
}

impl StatefulWidget for ProfilePicker {
    type State = ProfilePickerState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut ProfilePickerState) {
        if !state.visible {
            return;
        }

        let popup = Popup::new(" Select Profile ")
            .accent(self.accent)
            .width(self.popup_width);
        let inner = popup.render(area, buf);

        // Top-level vertical: search row (1) + separator (1) + content row
        // (rest) + footer (1).
        let v_regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1), // separator line
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(inner);

        // ── Search input row (spans both blocks) ──
        let input_line = if state.query.is_empty() {
            Line::from(vec![
                Span::styled("/", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    " search profiles…",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled("/", Style::default().fg(self.accent)),
                Span::styled(state.query.clone(), Style::default().fg(Color::White)),
            ])
        };
        Widget::render(Paragraph::new(input_line), v_regions[0], buf);

        Widget::render(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray)),
            v_regions[1],
            buf,
        );

        // ── Content area: two horizontal blocks ──
        let h_regions = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(self.list_width), Constraint::Min(10)])
            .split(v_regions[2]);

        self.render_list_block(h_regions[0], buf, state);
        self.render_preview_block(h_regions[1], buf, state);

        // ── Footer ──
        let hint = " Enter spawn  ↑↓ navigate  Esc cancel";
        let p = Paragraph::new(hint).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        );
        Widget::render(p, v_regions[3], buf);
    }
}

impl ProfilePicker {
    /// Render the left block: list of profiles.
    fn render_list_block(&self, area: Rect, buf: &mut Buffer, state: &mut ProfilePickerState) {
        let block = Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " Profiles ",
                Style::default()
                    .fg(self.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        block.render(area, buf);

        if state.filtered.is_empty() {
            let line = Line::from(Span::styled(
                "  No profiles found.",
                Style::default().fg(Color::DarkGray),
            ));
            let area = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: 1,
            };
            Widget::render(Paragraph::new(line), area, buf);
            return;
        }

        let items: Vec<ListItem> = state
            .filtered
            .iter()
            .enumerate()
            .map(|(sel_i, &item_i)| {
                let item = &state.items[item_i];
                let is_selected = sel_i == state.selected;
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(self.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };

                Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled("+ ", Style::default().fg(self.accent)),
                    Span::styled(item.profile.name.clone(), style),
                ])
            })
            .map(ListItem::new)
            .collect();

        let list = List::new(items).highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(self.accent)
                .add_modifier(Modifier::BOLD),
        );
        StatefulWidget::render(list, inner, buf, &mut state.list_state);
    }

    /// Render the right block: preview of the currently selected profile.
    fn render_preview_block(&self, area: Rect, buf: &mut Buffer, state: &ProfilePickerState) {
        let block = Block::default().borders(Borders::NONE).title(Span::styled(
            " Preview ",
            Style::default()
                .fg(self.accent)
                .add_modifier(Modifier::BOLD),
        ));
        let inner = block.inner(area);
        block.render(area, buf);

        let Some(item) = state.selected_item() else {
            let line = Line::from(Span::styled(
                "  No profile selected.",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
            let area = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: 1,
            };
            Widget::render(Paragraph::new(line), area, buf);
            return;
        };

        let p = &item.profile;
        let mut lines: Vec<Line> = Vec::new();

        // Name.
        lines.push(Line::from(vec![
            Span::styled("  Name       ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                p.name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // Description.
        if !p.description.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  Desc       ", Style::default().fg(Color::DarkGray)),
                Span::styled(p.description.clone(), Style::default().fg(Color::Gray)),
            ]));
        }

        // Identity.
        lines.push(Line::from(vec![
            Span::styled("  Identity   ", Style::default().fg(Color::DarkGray)),
            Span::styled(p.agent_identity.clone(), Style::default().fg(Color::Cyan)),
        ]));

        // Preferred model.
        let model_str = p.preferred_model.as_deref().unwrap_or("(global default)");
        lines.push(Line::from(vec![
            Span::styled("  Model      ", Style::default().fg(Color::DarkGray)),
            Span::styled(model_str, Style::default().fg(Color::Gray)),
        ]));

        // Spacer.
        lines.push(Line::from(""));

        // Tool flags.
        lines.push(Line::from(Span::styled(
            "  Capabilities:",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )));

        let mut flag = |name: &str, on: bool| {
            let (icon, color) = if on {
                ("●", Color::Green)
            } else {
                ("○", Color::DarkGray)
            };
            lines.push(Line::from(vec![
                Span::styled("    ", Style::default()),
                Span::styled(icon, Style::default().fg(color)),
                Span::raw(" "),
                Span::styled(
                    name.to_string(),
                    Style::default().fg(if on { Color::Gray } else { Color::DarkGray }),
                ),
            ]));
        };
        flag("bibliography", p.enable_bibliography);
        flag("opengwas", p.enable_opengwas);
        flag("opentargets", p.enable_opentargets);
        flag("gwascatalog", p.enable_gwascatalog);
        flag("iceberg", p.enable_iceberg);
        flag("dag-history", p.enable_dag_history);

        // Truncate to fit available height.
        let max_lines = inner.height as usize;
        if lines.len() > max_lines {
            lines.truncate(max_lines.saturating_sub(1));
            lines.push(Line::from(Span::styled(
                "    …",
                Style::default().fg(Color::DarkGray),
            )));
        }

        let paragraph = Paragraph::new(lines);
        Widget::render(paragraph, inner, buf);
    }
}
