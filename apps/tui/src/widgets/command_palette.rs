//! Popup command palette (Ctrl+P).
//!
//! A fuzzy-filterable command list rendered as a centered overlay above the
//! active tab. The user types to filter, Up/Down to navigate, Enter to execute,
//! Esc to dismiss. State is held on [`CommandPaletteState`] so the App can
//! inspect it; [`CommandPalette`] is the renderer.
//!
//! Commands are a fixed catalogue built once in [`CommandPaletteState::new`];
//! each carries a [`CommandAction`] the App matches on when the user presses
//! Enter. Adding a new command is a one-line entry in that catalogue.

use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

/// Actionable operations the palette can emit. The App is responsible for
/// executing them — the palette only selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    /// Switch to a main tab by index (matches `state::TABS`).
    SwitchTab(usize),
    /// Quit the application.
    Quit,
    /// Cancel the running agent (cooperative).
    CancelAgent,
    /// Enter the composer (Agent tab input mode).
    EnterInput,
    /// Toggle auto-scroll-to-bottom on the Agent tab.
    ToggleAutoScroll,
    /// Jump to the bottom of the transcript.
    ScrollToBottom,
    /// Jump to the top of the transcript.
    ScrollToTop,
    /// Start an incremental Ctrl+R history search.
    HistorySearch,
    /// Clear the conversation transcript (keeps agent context).
    ClearTranscript,
    /// Reload the model config panel from the database.
    ReloadConfig,
}

/// A single selectable command entry.
#[derive(Debug, Clone)]
pub struct Command {
    /// Display title (also the primary filter target).
    pub title: &'static str,
    /// Optional secondary keywords used for filtering but not shown.
    pub keywords: &'static str,
    /// Optional category label shown dim on the right.
    pub category: &'static str,
    pub action: CommandAction,
}

/// Mutable state for the command palette.
pub struct CommandPaletteState {
    pub visible: bool,
    pub query: String,
    /// All commands (catalogue). Built once.
    commands: Vec<Command>,
    /// Indices into `commands` that match the current query, in match order.
    filtered: Vec<usize>,
    /// Navigation state over `filtered`.
    list_state: ListState,
}

impl Default for CommandPaletteState {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandPaletteState {
    pub fn new() -> Self {
        let commands = vec![
            Command {
                title: "Switch to Agent tab",
                keywords: "chat agent conversation",
                category: "tab",
                action: CommandAction::SwitchTab(0),
            },
            Command {
                title: "Switch to Config tab",
                keywords: "settings providers model",
                category: "tab",
                action: CommandAction::SwitchTab(1),
            },
            Command {
                title: "New conversation",
                keywords: "clear transcript reset chat",
                category: "agent",
                action: CommandAction::ClearTranscript,
            },
            Command {
                title: "Send a message",
                keywords: "compose input prompt type",
                category: "agent",
                action: CommandAction::EnterInput,
            },
            Command {
                title: "Search history",
                keywords: "ctrl r recall previous prompt",
                category: "agent",
                action: CommandAction::HistorySearch,
            },
            Command {
                title: "Cancel agent",
                keywords: "stop abort interrupt request",
                category: "agent",
                action: CommandAction::CancelAgent,
            },
            Command {
                title: "Scroll to bottom",
                keywords: "tail follow latest end",
                category: "view",
                action: CommandAction::ScrollToBottom,
            },
            Command {
                title: "Scroll to top",
                keywords: "home beginning first",
                category: "view",
                action: CommandAction::ScrollToTop,
            },
            Command {
                title: "Toggle auto-scroll",
                keywords: "follow pin lock ctrl g",
                category: "view",
                action: CommandAction::ToggleAutoScroll,
            },
            Command {
                title: "Reload config",
                keywords: "refresh providers model",
                category: "config",
                action: CommandAction::ReloadConfig,
            },
            Command {
                title: "Quit",
                keywords: "exit close bye",
                category: "app",
                action: CommandAction::Quit,
            },
        ];

        let filtered: Vec<usize> = (0..commands.len()).collect();
        let mut list_state = ListState::default();
        list_state.select(Some(0));

        Self {
            visible: false,
            query: String::new(),
            commands,
            filtered,
            list_state,
        }
    }

    pub fn open(&mut self) {
        self.visible = true;
        self.query.clear();
        self.refilter();
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn toggle(&mut self) {
        if self.visible {
            self.close();
        } else {
            self.open();
        }
    }

    /// Move the selection up (towards the first match).
    pub fn move_up(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = i.saturating_sub(1);
        self.list_state.select(Some(next));
    }

    /// Move the selection down (towards the last match).
    pub fn move_down(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1).min(self.filtered.len() - 1);
        self.list_state.select(Some(next));
    }

    /// Append a typed character to the query and refilter.
    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    /// Delete the last query character and refilter.
    pub fn pop_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    /// Return the action behind the currently selected match, if any.
    pub fn selected_action(&self) -> Option<CommandAction> {
        let idx = self.list_state.selected()?;
        let cmd_idx = *self.filtered.get(idx)?;
        Some(self.commands[cmd_idx].action)
    }

    /// Recompute `filtered` from `query` and reset selection to the top.
    fn refilter(&mut self) {
        let needle = self.query.trim().to_lowercase();
        if needle.is_empty() {
            self.filtered = (0..self.commands.len()).collect();
        } else {
            self.filtered = self
                .commands
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.title.to_lowercase().contains(&needle)
                        || c.keywords.to_lowercase().contains(&needle)
                })
                .map(|(i, _)| i)
                .collect();
        }
        self.list_state.select(if self.filtered.is_empty() {
            None
        } else {
            Some(0)
        });
    }

    /// Build read-only `ListItem`s for the currently filtered commands.
    fn list_items<'a>(&self) -> Vec<ListItem<'a>> {
        self.filtered
            .iter()
            .map(|&i| {
                let c = &self.commands[i];
                ListItem::new(Line::from(vec![
                    Span::styled(c.title, Style::default().fg(Color::Gray)),
                    Span::raw("  "),
                    Span::styled(c.category, Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect()
    }
}

/// Popup renderer. Caller is responsible for drawing a `Clear` widget on the
/// same area first so underlying content doesn't bleed through.
pub struct CommandPalette<'a> {
    pub state: &'a mut CommandPaletteState,
}

impl CommandPalette<'_> {
    /// Render the palette as an overlay within `frame_area`. Sized to ~60% of
    /// width (clamped) and as tall as needed for up to 9 visible rows + the
    /// input row.
    pub fn render(self, frame_area: Rect, buf: &mut ratatui::prelude::Buffer) {
        // Centered popup box.
        let popup_width = frame_area.width.min(80).max(40);
        let popup_height = frame_area
            .height
            .min((self.state.filtered.len() as u16 + 5).max(8));
        let x = frame_area.x + (frame_area.width.saturating_sub(popup_width)) / 2;
        let y = frame_area.y + (frame_area.height.saturating_sub(popup_height)) / 3;
        let area = Rect::new(x, y, popup_width, popup_height);

        // Drop a backdrop so previous content doesn't show through.
        Clear.render(area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                " Command Palette ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        block.render(area, buf);

        // Split into input + list.
        let chunks = Layout::default()
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);

        // Input row: "❯ <query>" with a trailing caret.
        let input_line = Line::from(vec![
            Span::styled("❯ ", Style::default().fg(Color::Cyan)),
            Span::raw(&self.state.query),
            Span::styled("▏", Style::default().fg(Color::DarkGray)),
        ]);
        Paragraph::new(input_line).render(chunks[0], buf);

        // List of matches.
        let items = self.state.list_items();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ")
            .style(Style::default().fg(Color::Gray));
        ratatui::widgets::StatefulWidget::render(list, chunks[1], buf, &mut self.state.list_state);

        // Footer hint.
        if area.height >= 4 {
            let footer = Paragraph::new(Line::from(vec![
                Span::styled("↑↓ navigate", Style::default().fg(Color::DarkGray)),
                Span::raw("  "),
                Span::styled("↵ run", Style::default().fg(Color::DarkGray)),
                Span::raw("  "),
                Span::styled("esc close", Style::default().fg(Color::DarkGray)),
            ]))
            .alignment(Alignment::Center);
            // Render at the bottom border row of the popup.
            let footer_area = Rect {
                x: area.x + 1,
                y: area.y + area.height.saturating_sub(1),
                width: area.width.saturating_sub(2),
                height: 1,
            };
            // Only draw the footer if the popup is tall enough that it doesn't
            // collide with the list (otherwise skip silently).
            if footer_area.y > chunks[1].bottom() {
                footer.render(footer_area, buf);
            }
        }
    }
}
