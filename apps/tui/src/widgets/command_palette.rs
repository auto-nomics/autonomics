//! Popup command palette (Ctrl+P).
//!
//! A fuzzy-filterable command list rendered as a centered overlay above the
//! active tab. The user types to filter, Up/Down to navigate, Enter to execute,
//! Esc to dismiss. State is held on [`CommandPaletteState`] so the App can
//! inspect it; [`CommandPalette`] is the renderer.
//!
//! Commands are rebuilt dynamically — profile-based spawn commands are added
//! when the profile list changes via [`CommandPaletteState::set_profiles`].

use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

/// Actionable operations the palette can emit. The App is responsible for
/// executing them — the palette only selects.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Spawn a new agent from a profile by name.
    SpawnAgent(String),
}

/// A single selectable command entry.
#[derive(Debug, Clone)]
pub struct Command {
    pub title: String,
    pub keywords: String,
    pub category: String,
    pub action: CommandAction,
}

/// Mutable state for the command palette.
pub struct CommandPaletteState {
    pub visible: bool,
    pub query: String,
    commands: Vec<Command>,
    filtered: Vec<usize>,
    list_state: ListState,
}

impl Default for CommandPaletteState {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandPaletteState {
    pub fn new() -> Self {
        let mut state = Self {
            visible: false,
            query: String::new(),
            commands: Vec::new(),
            filtered: Vec::new(),
            list_state: ListState::default(),
        };
        state.rebuild_commands(&[]);
        state.list_state.select(Some(0));
        state
    }

    /// Rebuild the command list, injecting one "New agent from {name}" entry
    /// per profile. Call this whenever the profile list changes.
    pub fn set_profiles(&mut self, profiles: &[agentik_core::AgentProfile]) {
        self.rebuild_commands(profiles);
        self.refilter();
    }

    fn rebuild_commands(&mut self, profiles: &[agentik_core::AgentProfile]) {
        let mut commands = vec![
            Command {
                title: "Switch to Agent tab".into(),
                keywords: "chat agent conversation".into(),
                category: "tab".into(),
                action: CommandAction::SwitchTab(0),
            },
            Command {
                title: "Switch to Config tab".into(),
                keywords: "settings providers model".into(),
                category: "tab".into(),
                action: CommandAction::SwitchTab(1),
            },
        ];

        // Dynamic: one spawn command per profile.
        for p in profiles {
            commands.push(Command {
                title: format!("New agent: {}", p.name),
                keywords: format!("spawn create start {} agent", p.name),
                category: "agent".into(),
                action: CommandAction::SpawnAgent(p.name.clone()),
            });
        }

        commands.extend([
            Command {
                title: "New conversation".into(),
                keywords: "clear transcript reset chat".into(),
                category: "agent".into(),
                action: CommandAction::ClearTranscript,
            },
            Command {
                title: "Send a message".into(),
                keywords: "compose input prompt type".into(),
                category: "agent".into(),
                action: CommandAction::EnterInput,
            },
            Command {
                title: "Search history".into(),
                keywords: "ctrl r recall previous prompt".into(),
                category: "agent".into(),
                action: CommandAction::HistorySearch,
            },
            Command {
                title: "Cancel agent".into(),
                keywords: "stop abort interrupt request".into(),
                category: "agent".into(),
                action: CommandAction::CancelAgent,
            },
            Command {
                title: "Scroll to bottom".into(),
                keywords: "tail follow latest end".into(),
                category: "view".into(),
                action: CommandAction::ScrollToBottom,
            },
            Command {
                title: "Scroll to top".into(),
                keywords: "home beginning first".into(),
                category: "view".into(),
                action: CommandAction::ScrollToTop,
            },
            Command {
                title: "Toggle auto-scroll".into(),
                keywords: "follow pin lock ctrl g".into(),
                category: "view".into(),
                action: CommandAction::ToggleAutoScroll,
            },
            Command {
                title: "Reload config".into(),
                keywords: "refresh providers model".into(),
                category: "config".into(),
                action: CommandAction::ReloadConfig,
            },
            Command {
                title: "Quit".into(),
                keywords: "exit close bye".into(),
                category: "app".into(),
                action: CommandAction::Quit,
            },
        ]);

        self.commands = commands;
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

    pub fn move_up(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = i.saturating_sub(1);
        self.list_state.select(Some(next));
    }

    pub fn move_down(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1).min(self.filtered.len() - 1);
        self.list_state.select(Some(next));
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    pub fn selected_action(&self) -> Option<CommandAction> {
        let idx = self.list_state.selected()?;
        let cmd_idx = *self.filtered.get(idx)?;
        Some(self.commands[cmd_idx].action.clone())
    }

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

    fn list_items<'a>(&self) -> Vec<ListItem<'a>> {
        self.filtered
            .iter()
            .map(|&i| {
                let c = &self.commands[i];
                ListItem::new(Line::from(vec![
                    Span::styled(c.title.clone(), Style::default().fg(Color::Gray)),
                    Span::raw("  "),
                    Span::styled(
                        c.category.clone(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect()
    }
}

/// Popup renderer.
pub struct CommandPalette<'a> {
    pub state: &'a mut CommandPaletteState,
}

impl CommandPalette<'_> {
    pub fn render(self, frame_area: Rect, buf: &mut ratatui::prelude::Buffer) {
        let popup_width = frame_area.width.min(80).max(40);
        let popup_height = frame_area
            .height
            .min((self.state.filtered.len() as u16 + 5).max(8));
        let x = frame_area.x + (frame_area.width.saturating_sub(popup_width)) / 2;
        let y = frame_area.y + (frame_area.height.saturating_sub(popup_height)) / 3;
        let area = Rect::new(x, y, popup_width, popup_height);

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

        let chunks = Layout::default()
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);

        // Input line.
        let input_line = format!("❯ {}", self.state.query);
        Paragraph::new(input_line)
            .style(Style::default().fg(Color::Cyan))
            .render(chunks[0], buf);

        // Command list.
        let items = self.state.list_items();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");
        StatefulWidget::render(list, chunks[1], buf, &mut self.state.list_state);
    }
}

use ratatui::widgets::StatefulWidget;
