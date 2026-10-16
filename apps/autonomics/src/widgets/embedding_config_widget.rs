//! Embedding model configuration surface.

use crate::xai_textarea::{TextArea, TextAreaState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gateway::proto::{EmbeddingConfigView, SaveEmbeddingConfigRequest};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    prelude::{Buffer, Widget},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, StatefulWidgetRef},
};

use crate::widgets::popup::{Popup, PopupControls};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingConfigField {
    Enabled,
    BaseUrl,
    Model,
    BatchSize,
    TimeoutSecs,
    Save,
}

const FIELDS: [EmbeddingConfigField; 6] = [
    EmbeddingConfigField::Enabled,
    EmbeddingConfigField::BaseUrl,
    EmbeddingConfigField::Model,
    EmbeddingConfigField::BatchSize,
    EmbeddingConfigField::TimeoutSecs,
    EmbeddingConfigField::Save,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingConfigAction {
    None,
    Close,
    Save,
}

pub struct EmbeddingConfigState {
    pub config: EmbeddingConfigView,
    pub focused: EmbeddingConfigField,
    pub base_url: TextArea,
    pub base_url_state: TextAreaState,
    pub model: TextArea,
    pub model_state: TextAreaState,
    pub batch_size: TextArea,
    pub batch_size_state: TextAreaState,
    pub timeout_secs: TextArea,
    pub timeout_secs_state: TextAreaState,
    pub cursor_pos: Option<(u16, u16)>,
}

impl Default for EmbeddingConfigState {
    fn default() -> Self {
        let mut state = Self {
            config: EmbeddingConfigView {
                enabled: true,
                base_url: "http://127.0.0.1:11434".into(),
                model: "qwen3-embedding:0.6b".into(),
                batch_size: 32,
                timeout_secs: 30,
            },
            focused: EmbeddingConfigField::BaseUrl,
            base_url: TextArea::new(),
            base_url_state: Default::default(),
            model: TextArea::new(),
            model_state: Default::default(),
            batch_size: TextArea::new(),
            batch_size_state: Default::default(),
            timeout_secs: TextArea::new(),
            timeout_secs_state: Default::default(),
            cursor_pos: None,
        };
        state.sync_inputs();
        state
    }
}

impl EmbeddingConfigState {
    pub fn load(&mut self, config: EmbeddingConfigView) {
        self.config = config;
        self.sync_inputs();
    }

    fn sync_inputs(&mut self) {
        self.base_url = TextArea::new();
        self.base_url.insert_str(&self.config.base_url);
        self.model = TextArea::new();
        self.model.insert_str(&self.config.model);
        self.batch_size = TextArea::new();
        self.batch_size
            .insert_str(&self.config.batch_size.to_string());
        self.timeout_secs = TextArea::new();
        self.timeout_secs
            .insert_str(&self.config.timeout_secs.to_string());
    }

    pub fn insert_paste(&mut self, text: &str) {
        match self.focused {
            EmbeddingConfigField::BaseUrl => self.base_url.insert_str(text),
            EmbeddingConfigField::Model => self.model.insert_str(text),
            EmbeddingConfigField::BatchSize => {
                let digits: String = text.chars().filter(char::is_ascii_digit).collect();
                self.batch_size.insert_str(&digits);
            }
            EmbeddingConfigField::TimeoutSecs => {
                let digits: String = text.chars().filter(char::is_ascii_digit).collect();
                self.timeout_secs.insert_str(&digits);
            }
            _ => {}
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> EmbeddingConfigAction {
        match key.code {
            KeyCode::Esc => return EmbeddingConfigAction::Close,
            KeyCode::Tab | KeyCode::Down => self.focus_next(),
            KeyCode::BackTab | KeyCode::Up => self.focus_previous(),
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return EmbeddingConfigAction::Save;
            }
            _ => match self.focused {
                EmbeddingConfigField::Enabled if key.code == KeyCode::Enter => {
                    self.config.enabled = !self.config.enabled;
                }
                EmbeddingConfigField::BaseUrl => {
                    self.base_url.input(*key);
                    self.config.base_url = self.base_url.text().trim().to_string();
                }
                EmbeddingConfigField::Model => {
                    self.model.input(*key);
                    self.config.model = self.model.text().trim().to_string();
                }
                EmbeddingConfigField::BatchSize => {
                    match key.code {
                        KeyCode::Char(c) if c.is_ascii_digit() => self.batch_size.input(*key),
                        KeyCode::Backspace => self.batch_size.input(*key),
                        _ => {}
                    }
                    self.config.batch_size = self
                        .batch_size
                        .text()
                        .trim()
                        .parse()
                        .unwrap_or(self.config.batch_size);
                }
                EmbeddingConfigField::TimeoutSecs => {
                    match key.code {
                        KeyCode::Char(c) if c.is_ascii_digit() => self.timeout_secs.input(*key),
                        KeyCode::Backspace => self.timeout_secs.input(*key),
                        _ => {}
                    }
                    self.config.timeout_secs = self
                        .timeout_secs
                        .text()
                        .trim()
                        .parse()
                        .unwrap_or(self.config.timeout_secs);
                }
                EmbeddingConfigField::Save if key.code == KeyCode::Enter => {
                    return EmbeddingConfigAction::Save;
                }
                _ => {}
            },
        }
        EmbeddingConfigAction::None
    }

    pub fn request(&self) -> SaveEmbeddingConfigRequest {
        SaveEmbeddingConfigRequest {
            config: self.config.clone(),
        }
    }

    fn focus_next(&mut self) {
        let index = FIELDS
            .iter()
            .position(|field| field == &self.focused)
            .unwrap_or(0);
        self.focused = FIELDS[(index + 1) % FIELDS.len()];
    }

    fn focus_previous(&mut self) {
        let index = FIELDS
            .iter()
            .position(|field| field == &self.focused)
            .unwrap_or(0);
        self.focused = FIELDS[(index + FIELDS.len() - 1) % FIELDS.len()];
    }
}

pub struct EmbeddingConfigWidget;

impl StatefulWidgetRef for EmbeddingConfigWidget {
    type State = EmbeddingConfigState;

    fn render_ref(&self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        Clear.render(area, buf);
        let rows = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(area);

        render_toggle(rows[0], buf, state);
        render_input(
            rows[1],
            buf,
            state,
            EmbeddingConfigField::BaseUrl,
            "Base URL",
        );
        render_input(rows[2], buf, state, EmbeddingConfigField::Model, "Model");
        render_input(
            rows[3],
            buf,
            state,
            EmbeddingConfigField::BatchSize,
            "Batch size",
        );
        render_input(
            rows[4],
            buf,
            state,
            EmbeddingConfigField::TimeoutSecs,
            "Timeout (seconds)",
        );
        render_save(rows[5], buf, state);
        state.cursor_pos = match state.focused {
            EmbeddingConfigField::BaseUrl => state
                .base_url
                .cursor_pos_with_state(inner(rows[1]), state.base_url_state),
            EmbeddingConfigField::Model => state
                .model
                .cursor_pos_with_state(inner(rows[2]), state.model_state),
            EmbeddingConfigField::BatchSize => state
                .batch_size
                .cursor_pos_with_state(inner(rows[3]), state.batch_size_state),
            EmbeddingConfigField::TimeoutSecs => state
                .timeout_secs
                .cursor_pos_with_state(inner(rows[4]), state.timeout_secs_state),
            _ => None,
        };
    }
}

fn focused_style(active: bool) -> Style {
    if active {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn render_toggle(area: Rect, buf: &mut Buffer, state: &EmbeddingConfigState) {
    let style = focused_style(state.focused == EmbeddingConfigField::Enabled);
    let marker = if state.config.enabled { "[x]" } else { "[ ]" };
    Paragraph::new(Line::from(format!("{marker} Enabled")))
        .style(style)
        .render(area, buf);
}

fn render_input(
    area: Rect,
    buf: &mut Buffer,
    state: &mut EmbeddingConfigState,
    field: EmbeddingConfigField,
    label: &str,
) {
    let focused = state.focused == field;
    let (text_area, text_state) = match field {
        EmbeddingConfigField::BaseUrl => (&mut state.base_url, &mut state.base_url_state),
        EmbeddingConfigField::Model => (&mut state.model, &mut state.model_state),
        EmbeddingConfigField::BatchSize => (&mut state.batch_size, &mut state.batch_size_state),
        _ => (&mut state.timeout_secs, &mut state.timeout_secs_state),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(label)
        .border_style(focused_style(focused));
    let inner_area = inner(area);
    block.render(area, buf);
    ratatui::widgets::StatefulWidgetRef::render_ref(&&*text_area, inner_area, buf, text_state);
}

fn render_save(area: Rect, buf: &mut Buffer, state: &EmbeddingConfigState) {
    let style = focused_style(state.focused == EmbeddingConfigField::Save);
    Paragraph::new(Line::from("Save and apply (Enter)"))
        .style(style)
        .render(area, buf);
}

fn inner(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}
