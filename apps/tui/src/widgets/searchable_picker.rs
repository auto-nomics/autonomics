//! Generic searchable selectable-list popup.
//!
//! Shared infrastructure for the command palette and the profile picker
//! (and any future "pick from a list with fuzzy search" popup).
//!
//! Each item type implements [`PickerItem`]. The widget renders a centered
//! [`Popup`] containing a search input row, a filtered list, and a footer
//! hint. Navigation (Up/Down/Enter/Esc) is handled by the caller via
//! [`PickerState`] methods.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Widget},
};

use crate::widgets::popup::{Popup, PopupControls};

// ═══════════════════════════════════════════════════════════════════════
// Trait
// ═══════════════════════════════════════════════════════════════════════

/// An item in a [`SearchablePicker`] list.
pub trait PickerItem: Clone {
    /// Primary display text (also the primary filter target).
    fn title(&self) -> &str;
    /// Secondary filter text (not displayed).
    fn keywords(&self) -> &str;
    /// Optional description shown for the selected item only.
    fn description(&self) -> Option<&str> {
        None
    }
    /// Optional category tag shown after the title.
    fn category(&self) -> Option<&str> {
        None
    }
}

// ═══════════════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════════════

/// State for a searchable picker popup.
pub struct PickerState<T: PickerItem> {
    pub visible: bool,
    pub query: String,
    pub items: Vec<T>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub list_state: ListState,
}

impl<T: PickerItem> PickerState<T> {
    /// Create an empty picker state.
    pub fn new_empty() -> Self {
        Self {
            visible: false,
            query: String::new(),
            items: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
        }
    }
}

impl<T: PickerItem> PickerState<T> {
    // ── Visibility ──

    pub fn open(&mut self) {
        self.visible = true;
        self.query.clear();
        self.selected = 0;
        self.refilter();
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    // ── Navigation ──

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

    fn sync_list_state(&mut self) {
        self.list_state.select(
            if self.filtered.is_empty() || self.selected >= self.filtered.len() {
                None
            } else {
                Some(self.selected)
            },
        );
    }

    // ── Selection ──

    /// Returns a clone of the selected item, if any.
    pub fn selected_item(&self) -> Option<T> {
        let &idx = self.filtered.get(self.selected)?;
        self.items.get(idx).cloned()
    }

    // ── Data ──

    pub fn set_items(&mut self, items: Vec<T>) {
        self.items = items;
        if self.visible {
            self.refilter();
        }
    }


    /// Replace the query and re-run the filter. Used by callers that
    /// manage their own text input (e.g. an embedded `TextArea`) and
    /// need to sync external edits into the picker state.
    pub fn set_query(&mut self, query: &str) {
        self.query.clear();
        self.query.push_str(query);
        self.refilter();
    }

    // ── Filtering ──

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
                    item.title().to_lowercase().contains(&needle)
                        || item.keywords().to_lowercase().contains(&needle)
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.selected = 0;
        self.sync_list_state();
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Widget
// ═══════════════════════════════════════════════════════════════════════

/// Renders a searchable picker as a centered popup overlay.
///
/// Pass `frame_area` (the full terminal area) for centering. The popup
/// auto-sizes to ~60% width, content-driven height.
pub struct SearchablePicker<'a> {
    /// Accent color for title/border.
    pub accent: Color,
    /// Footer hint line.
    pub footer_hint: &'a str,
}

impl<'a> SearchablePicker<'a> {
    pub fn new(_title: &'a str) -> Self {
        Self {
            accent: Color::Cyan,
            footer_hint: " Enter select  ↑↓ navigate  Esc cancel",
        }
    }


    pub fn accent(mut self, c: Color) -> Self {
        self.accent = c;
        self
    }

    pub fn footer_hint(mut self, h: &'a str) -> Self {
        self.footer_hint = h;
        self
    }

    pub fn render_list_and_footer<T: PickerItem>(
        &self,
        list_area: Rect,
        footer_area: Rect,
        buf: &mut Buffer,
        state: &mut PickerState<T>,
    ) {
        // ── List ──
        if state.filtered.is_empty() {
            let line = Line::from(Span::styled(
                "  No matches.",
                Style::default().fg(Color::DarkGray),
            ));
            let area = Rect {
                x: list_area.x,
                y: list_area.y + 1,
                width: list_area.width,
                height: 1,
            };
            Widget::render(ratatui::widgets::Paragraph::new(line), area, buf);
        } else {
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

                    let mut spans = vec![
                        Span::styled("  ", Style::default()),
                        Span::styled(item.title(), style),
                    ];

                    if let Some(cat) = item.category() {
                        spans.push(Span::raw("  "));
                        spans.push(Span::styled(cat, Style::default().fg(Color::DarkGray)));
                    }

                    let mut lines = vec![Line::from(spans)];

                    if is_selected {
                        if let Some(desc) = item.description() {
                            if !desc.is_empty() {
                                lines.push(Line::from(Span::styled(
                                    format!("    {}", desc),
                                    Style::default().fg(Color::DarkGray),
                                )));
                            }
                        }
                    }

                    ListItem::new(lines)
                })
                .collect();

            let list = List::new(items).highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(self.accent)
                    .add_modifier(Modifier::BOLD),
            );

            ratatui::widgets::StatefulWidget::render(list, list_area, buf, &mut state.list_state);
        }

        // ── Footer ──
        let p = ratatui::widgets::Paragraph::new(self.footer_hint).style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        );
        Widget::render(p, footer_area, buf);
    }
}
