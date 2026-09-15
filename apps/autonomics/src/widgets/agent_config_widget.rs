//! Per-agent runtime configuration popup.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};

use crate::widgets::popup::{Popup, PopupControls};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentConfigCommand {
    None,
    Close,
    Save {
        runtime: agentik_core::AgentRuntimeOverrides,
    },
}

#[derive(Default)]
pub struct AgentConfigState {
    pub visible: bool,
    pub loading: bool,
    pub saving: bool,
    pub agent_path: String,
    pub runtime: agentik_core::AgentRuntimeOverrides,
    pub effective: Option<agentik_core::AgentRuntimeConfig>,
    pub cursor: usize,
}

impl AgentConfigState {
    pub fn open(&mut self, agent_path: impl Into<String>) {
        self.visible = true;
        self.loading = true;
        self.saving = false;
        self.agent_path = agent_path.into();
        self.runtime = agentik_core::AgentRuntimeOverrides::default();
        self.effective = None;
        self.cursor = 0;
    }

    pub fn loaded(&mut self, view: gateway::proto::AgentRuntimeConfigView) {
        self.loading = false;
        self.saving = false;
        self.runtime = view.runtime;
        self.effective = Some(view.effective);
    }

    pub fn saved(&mut self, view: gateway::proto::AgentRuntimeConfigView) {
        self.loading = false;
        self.saving = false;
        self.runtime = view.runtime;
        self.effective = Some(view.effective);
    }

    pub fn failed(&mut self) {
        self.loading = false;
        self.saving = false;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> AgentConfigCommand {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return AgentConfigCommand::None;
        }

        match key.code {
            KeyCode::Esc => AgentConfigCommand::Close,
            KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                AgentConfigCommand::None
            }
            KeyCode::Down => {
                self.cursor = (self.cursor + 1).min(1);
                AgentConfigCommand::None
            }
            KeyCode::Left => {
                self.cycle_cursor(-1);
                AgentConfigCommand::None
            }
            KeyCode::Char(' ') | KeyCode::Right | KeyCode::Enter => {
                if key.code == KeyCode::Enter && !self.loading && !self.saving {
                    return AgentConfigCommand::Save {
                        runtime: self.runtime.clone(),
                    };
                }
                self.cycle_cursor(1);
                AgentConfigCommand::None
            }
            KeyCode::Char('r') => {
                if !self.loading && !self.saving {
                    self.runtime = agentik_core::AgentRuntimeOverrides::default();
                }
                AgentConfigCommand::None
            }
            KeyCode::Char('s') if !self.loading && !self.saving => AgentConfigCommand::Save {
                runtime: self.runtime.clone(),
            },
            _ => AgentConfigCommand::None,
        }
    }

    fn cycle_cursor(&mut self, direction: i32) {
        if self.loading || self.saving {
            return;
        }
        let current = match self.cursor {
            0 => self.runtime.use_memory,
            _ => self.runtime.generate_memory,
        };
        let next = match (current, direction.is_positive()) {
            (None, true) => Some(true),
            (Some(true), true) => Some(false),
            (Some(false), true) => None,
            (None, false) => Some(false),
            (Some(false), false) => Some(true),
            (Some(true), false) => None,
        };
        match self.cursor {
            0 => self.runtime.use_memory = next,
            _ => self.runtime.generate_memory = next,
        }
    }
}

pub fn render_agent_config(frame_area: Rect, buf: &mut Buffer, state: &mut AgentConfigState) {
    if !state.visible {
        return;
    }

    let popup = Popup::new(" Agent Config ", PopupControls::default())
        .width(64.min(frame_area.width))
        .height(13.min(frame_area.height))
        .vertically_centered(true)
        .accent(Color::Cyan);
    let inner = popup.render(frame_area, buf);
    Clear.render(inner, buf);

    let mut lines = vec![
        Line::from(vec![
            Span::styled(" Agent  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                state.agent_path.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
    ];

    if state.loading {
        lines.push(Line::from(Span::styled(
            " Loading runtime config...",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(setting_line(
            "Memory injection",
            state.runtime.use_memory,
            state.effective.map(|config| config.use_memory),
            state.cursor == 0,
        ));
        lines.push(setting_line(
            "Memory generation",
            state.runtime.generate_memory,
            state.effective.map(|config| config.generate_memory),
            state.cursor == 1,
        ));
    }

    lines.push(Line::raw(""));
    if state.saving {
        lines.push(Line::from(Span::styled(
            " Saving...",
            Style::default().fg(Color::Yellow),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            " Up/Down select  Space cycle  Enter save  Esc cancel",
            Style::default().fg(Color::DarkGray),
        )));
    }

    Paragraph::new(lines).render(inner, buf);
}

fn setting_line(
    name: &str,
    value: Option<bool>,
    effective: Option<bool>,
    selected: bool,
) -> Line<'static> {
    let override_text = match value {
        None => "inherit".to_string(),
        Some(true) => "on".to_string(),
        Some(false) => "off".to_string(),
    };
    let effective_text = match effective {
        Some(true) => "effective on".to_string(),
        Some(false) => "effective off".to_string(),
        None => String::new(),
    };
    let marker = if selected { ">" } else { " " };
    let value_color = match value {
        Some(true) => Color::Green,
        Some(false) => Color::DarkGray,
        None => Color::Cyan,
    };

    Line::from(vec![
        Span::styled(marker, Style::default().fg(Color::Cyan)),
        Span::raw(" "),
        Span::styled(
            name.to_string(),
            Style::default().fg(if selected { Color::White } else { Color::Gray }),
        ),
        Span::raw("  "),
        Span::styled(
            override_text,
            Style::default()
                .fg(value_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(effective_text, Style::default().fg(Color::DarkGray)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn space_cycles_inherit_on_off_and_back() {
        let mut state = AgentConfigState::default();
        state.visible = true;
        state.loading = false;

        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(state.runtime.use_memory, Some(true));
        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(state.runtime.use_memory, Some(false));
        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(state.runtime.use_memory, None);
    }

    #[test]
    fn left_reverses_the_override_cycle() {
        let mut state = AgentConfigState::default();
        state.visible = true;
        state.loading = false;

        state.handle_key(key(KeyCode::Left));
        assert_eq!(state.runtime.use_memory, Some(false));
        state.handle_key(key(KeyCode::Left));
        assert_eq!(state.runtime.use_memory, Some(true));
        state.handle_key(key(KeyCode::Left));
        assert_eq!(state.runtime.use_memory, None);
    }
}
