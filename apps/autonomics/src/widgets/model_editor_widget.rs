//! Editor widget for provider-scoped custom models.
//!
//! The widget owns form navigation, validation, and rendering. Its parent maps
//! the resulting [`ModelDraft`] to application persistence commands.

use agentik_sdk::model::ModelInfo;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::{StatefulWidget, Widget},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use crate::xai_textarea::{TextArea, TextAreaState};

/// A validated custom-model payload handed from the editor to its parent.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDraft {
    pub model_name: String,
    pub context_length: u64,
    pub max_output_tokens: u64,
    pub vision_ability: bool,
    pub supports_function_calling: bool,
    pub supports_streaming: bool,
    pub supports_thinking: bool,
    pub thinking_enabled: bool,
    pub input_token_price: f64,
    pub output_token_price: f64,
}

/// Fields in the add/edit model editor.
#[derive(Default, PartialEq, Eq, Copy, Clone)]
pub enum ModelField {
    #[default]
    ModelName,
    ContextLength,
    MaxOutput,
    InputPrice,
    OutputPrice,
    Vision,
    FunctionCalling,
    Streaming,
    Thinking,
    ThinkingEnabled,
}

/// The result of one key event handled by the editor.
#[derive(Debug, PartialEq)]
pub enum ModelEditorOutcome {
    Continue,
    Cancel,
    Save(ModelDraft),
}

/// TUI form state for custom models. Built-in presets never enter this state.
pub struct ModelEditorState {
    pub provider_name: String,
    pub original_model_name: Option<String>,
    model_name: TextArea,
    context_length: TextArea,
    max_output_tokens: TextArea,
    input_price: TextArea,
    output_price: TextArea,
    model_name_state: TextAreaState,
    context_length_state: TextAreaState,
    max_output_tokens_state: TextAreaState,
    input_price_state: TextAreaState,
    output_price_state: TextAreaState,
    vision: bool,
    function_calling: bool,
    streaming: bool,
    thinking: bool,
    thinking_enabled: bool,
    focused_field: ModelField,
    error: Option<String>,
    cursor_pos: Option<(u16, u16)>,
}

impl ModelEditorState {
    fn text_field(value: &str) -> (TextArea, TextAreaState) {
        let mut area = TextArea::new();
        area.show_scrollbar = false;
        area.set_text(value);
        (area, TextAreaState::default())
    }

    pub fn new_add(provider_name: String) -> Self {
        let (model_name, model_name_state) = Self::text_field("");
        let (context_length, context_length_state) = Self::text_field("131072");
        let (max_output_tokens, max_output_tokens_state) = Self::text_field("32768");
        let (input_price, input_price_state) = Self::text_field("0");
        let (output_price, output_price_state) = Self::text_field("0");
        Self {
            provider_name,
            original_model_name: None,
            model_name,
            context_length,
            max_output_tokens,
            input_price,
            output_price,
            model_name_state,
            context_length_state,
            max_output_tokens_state,
            input_price_state,
            output_price_state,
            vision: false,
            function_calling: true,
            streaming: true,
            thinking: false,
            thinking_enabled: false,
            focused_field: ModelField::ModelName,
            error: None,
            cursor_pos: None,
        }
    }

    pub fn new_edit(provider_name: String, model: ModelInfo) -> Self {
        let mut state = Self::new_add(provider_name);
        state.original_model_name = Some(model.model_name.clone());
        state.model_name.set_text(&model.model_name);
        state
            .context_length
            .set_text(&model.context_length.to_string());
        state
            .max_output_tokens
            .set_text(&model.max_output_tokens.to_string());
        state
            .input_price
            .set_text(&format!("{}", model.input_token_price));
        state
            .output_price
            .set_text(&format!("{}", model.output_token_price));
        state.vision = model.vision_ability;
        state.function_calling = model.supports_function_calling;
        state.streaming = model.supports_streaming;
        state.thinking = model.supports_thinking;
        state.thinking_enabled = model.thinking_enabled;
        state
    }

    pub fn cursor_pos(&self) -> Option<(u16, u16)> {
        self.cursor_pos
    }

    pub fn insert_paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        match self.focused_field {
            ModelField::ModelName => self.model_name.insert_str(text),
            ModelField::ContextLength => self.context_length.insert_str(text),
            ModelField::MaxOutput => self.max_output_tokens.insert_str(text),
            ModelField::InputPrice => self.input_price.insert_str(text),
            ModelField::OutputPrice => self.output_price.insert_str(text),
            _ => self.toggle_boolean(),
        }
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> ModelEditorOutcome {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Esc => ModelEditorOutcome::Cancel,
            KeyCode::Tab => {
                self.cycle_focus(false);
                ModelEditorOutcome::Continue
            }
            KeyCode::BackTab => {
                self.cycle_focus(true);
                ModelEditorOutcome::Continue
            }
            KeyCode::Up => {
                self.cycle_focus(true);
                ModelEditorOutcome::Continue
            }
            KeyCode::Down => {
                self.cycle_focus(false);
                ModelEditorOutcome::Continue
            }
            KeyCode::Char(' ') if self.is_boolean() => {
                self.toggle_boolean();
                ModelEditorOutcome::Continue
            }
            KeyCode::Enter if self.is_boolean() => {
                self.toggle_boolean();
                ModelEditorOutcome::Continue
            }
            KeyCode::Enter => self.save().map_or(ModelEditorOutcome::Continue, |draft| {
                ModelEditorOutcome::Save(draft)
            }),
            _ => {
                self.input(key);
                ModelEditorOutcome::Continue
            }
        }
    }

    fn cycle_focus(&mut self, reverse: bool) {
        use ModelField::*;
        let order = [
            ModelName,
            ContextLength,
            MaxOutput,
            InputPrice,
            OutputPrice,
            Vision,
            FunctionCalling,
            Streaming,
            Thinking,
            ThinkingEnabled,
        ];
        let current = order
            .iter()
            .position(|field| *field == self.focused_field)
            .unwrap_or(0);
        let next = if reverse {
            (current + order.len() - 1) % order.len()
        } else {
            (current + 1) % order.len()
        };
        self.focused_field = order[next];
    }

    fn is_boolean(&self) -> bool {
        matches!(
            self.focused_field,
            ModelField::Vision
                | ModelField::FunctionCalling
                | ModelField::Streaming
                | ModelField::Thinking
                | ModelField::ThinkingEnabled
        )
    }

    fn toggle_boolean(&mut self) {
        self.error = None;
        match self.focused_field {
            ModelField::Vision => self.vision = !self.vision,
            ModelField::FunctionCalling => self.function_calling = !self.function_calling,
            ModelField::Streaming => self.streaming = !self.streaming,
            ModelField::Thinking => {
                self.thinking = !self.thinking;
                if !self.thinking {
                    self.thinking_enabled = false;
                }
            }
            ModelField::ThinkingEnabled if self.thinking => {
                self.thinking_enabled = !self.thinking_enabled;
            }
            _ => {}
        }
    }

    fn input(&mut self, key: crossterm::event::KeyEvent) {
        self.error = None;
        match self.focused_field {
            ModelField::ModelName => self.model_name.input(key),
            ModelField::ContextLength => self.context_length.input(key),
            ModelField::MaxOutput => self.max_output_tokens.input(key),
            ModelField::InputPrice => self.input_price.input(key),
            ModelField::OutputPrice => self.output_price.input(key),
            _ => {}
        }
    }

    fn parse_u64(value: &str, label: &str) -> Result<u64, String> {
        value
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("{label} must be a non-negative integer"))
            .and_then(|parsed| {
                if parsed == 0 {
                    Err(format!("{label} must be greater than zero"))
                } else {
                    Ok(parsed)
                }
            })
    }

    fn parse_price(value: &str, label: &str) -> Result<f64, String> {
        let trimmed = value.trim();
        let parsed = if trimmed.is_empty() {
            0.0
        } else {
            trimmed
                .parse::<f64>()
                .map_err(|_| format!("{label} must be a number"))?
        };
        if !parsed.is_finite() || parsed < 0.0 {
            return Err(format!("{label} cannot be negative"));
        }
        Ok(parsed)
    }

    fn save(&mut self) -> Option<ModelDraft> {
        let parsed = (|| -> Result<ModelDraft, String> {
            let model_name = self.model_name.text().trim();
            if model_name.is_empty() {
                return Err("Model name is required".to_string());
            }
            Ok(ModelDraft {
                model_name: model_name.to_string(),
                context_length: Self::parse_u64(self.context_length.text(), "Context length")?,
                max_output_tokens: Self::parse_u64(self.max_output_tokens.text(), "Max output")?,
                vision_ability: self.vision,
                supports_function_calling: self.function_calling,
                supports_streaming: self.streaming,
                supports_thinking: self.thinking,
                thinking_enabled: self.thinking_enabled,
                input_token_price: Self::parse_price(self.input_price.text(), "Input price")?,
                output_token_price: Self::parse_price(self.output_price.text(), "Output price")?,
            })
        })();

        match parsed {
            Ok(draft) => Some(draft),
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }
}

/// Renders the add/edit custom-model form.
#[derive(Debug, Default)]
pub struct ModelEditorWidget;

impl StatefulWidget for ModelEditorWidget {
    type State = ModelEditorState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let title = if state.original_model_name.is_some() {
            " Edit Custom Model "
        } else {
            " Add Custom Model "
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::Yellow))
            .title(Line::from(vec![
                Span::styled(title, Style::default().fg(Color::Yellow)),
                Span::styled(
                    format!(" {} ", state.provider_name),
                    Style::default().fg(Color::White),
                ),
            ]))
            .title_bottom(
                Line::from("[Tab/↑↓] field  [Space] toggle  [Enter] save  [Esc] cancel")
                    .alignment(Alignment::Center),
            );
        let inner = block.inner(area);
        Clear.render(area, buf);
        block.render(area, buf);

        if inner.width < 36 || inner.height < 14 {
            Paragraph::new("  Increase terminal height to edit models.")
                .style(Style::default().fg(Color::DarkGray))
                .render(inner, buf);
            state.cursor_pos = None;
            return;
        }

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(inner);

        let label_style = Style::default().fg(Color::Cyan);
        let focused_label_style = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let dim_style = Style::default().fg(Color::DarkGray);
        let label = |name: &str| format!(" {name:<18}: ");
        let input_area = |row: Rect| Rect {
            x: inner.x + 20,
            width: inner.width.saturating_sub(20),
            ..row
        };
        let focused = |field: ModelField| state.focused_field == field;
        let text_style = |field: ModelField| {
            if focused(field) {
                Style::default()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            }
        };

        Paragraph::new(Line::from(vec![
            Span::styled(label("Provider"), label_style),
            Span::styled(
                state.provider_name.clone(),
                Style::default().fg(Color::White),
            ),
        ]))
        .render(rows[0], buf);

        let mut render_text = |row: usize,
                               field: ModelField,
                               name: &str,
                               area: &TextArea,
                               text_state: &mut TextAreaState| {
            Paragraph::new(Line::from(Span::styled(
                label(name),
                if focused(field) {
                    focused_label_style
                } else {
                    label_style
                },
            )))
            .render(rows[row], buf);
            let target = input_area(rows[row]);
            Paragraph::new(Span::styled(" ", text_style(field))).render(target, buf);
            let text_area = Rect {
                x: target.x + 1,
                width: target.width.saturating_sub(1),
                ..target
            };
            if text_area.width > 0 {
                ratatui::widgets::StatefulWidgetRef::render_ref(&area, text_area, buf, text_state);
            }
        };

        render_text(
            1,
            ModelField::ModelName,
            "Model",
            &state.model_name,
            &mut state.model_name_state,
        );
        render_text(
            2,
            ModelField::ContextLength,
            "Context Length",
            &state.context_length,
            &mut state.context_length_state,
        );
        render_text(
            3,
            ModelField::MaxOutput,
            "Max Output",
            &state.max_output_tokens,
            &mut state.max_output_tokens_state,
        );
        render_text(
            4,
            ModelField::InputPrice,
            "Input $/M Tok",
            &state.input_price,
            &mut state.input_price_state,
        );
        render_text(
            5,
            ModelField::OutputPrice,
            "Output $/M Tok",
            &state.output_price,
            &mut state.output_price_state,
        );

        let mut render_bool = |row: usize, field: ModelField, name: &str, value: bool| {
            let selected = focused(field);
            Paragraph::new(Line::from(Span::styled(
                label(name),
                if selected {
                    focused_label_style
                } else {
                    label_style
                },
            )))
            .render(rows[row], buf);
            let mark = if value { "[yes]" } else { "[no ]" };
            let style = if value {
                Style::default().fg(Color::Green)
            } else {
                dim_style
            };
            let target = Rect {
                x: inner.x + 20,
                width: inner.width.saturating_sub(20),
                ..rows[row]
            };
            Paragraph::new(Line::from(Span::styled(
                mark,
                if selected {
                    style.add_modifier(Modifier::BOLD)
                } else {
                    style
                },
            )))
            .render(target, buf);
        };

        render_bool(7, ModelField::Vision, "Vision", state.vision);
        render_bool(
            8,
            ModelField::FunctionCalling,
            "Function Calling",
            state.function_calling,
        );
        render_bool(9, ModelField::Streaming, "Streaming", state.streaming);
        render_bool(10, ModelField::Thinking, "Thinking", state.thinking);
        render_bool(
            11,
            ModelField::ThinkingEnabled,
            "Thinking Enabled",
            state.thinking_enabled,
        );

        match &state.error {
            Some(error) => {
                Paragraph::new(Line::from(Span::styled(
                    format!(" {error}"),
                    Style::default().fg(Color::Red),
                )))
                .render(rows[12], buf);
            }
            None => {
                Paragraph::new(Line::from(Span::styled(
                    " Custom models override built-in metadata.",
                    dim_style,
                )))
                .render(rows[12], buf);
            }
        }

        state.cursor_pos = match state.focused_field {
            ModelField::ModelName => state
                .model_name
                .cursor_pos_with_state(input_area(rows[1]), state.model_name_state.clone()),
            ModelField::ContextLength => state
                .context_length
                .cursor_pos_with_state(input_area(rows[2]), state.context_length_state.clone()),
            ModelField::MaxOutput => state
                .max_output_tokens
                .cursor_pos_with_state(input_area(rows[3]), state.max_output_tokens_state.clone()),
            ModelField::InputPrice => state
                .input_price
                .cursor_pos_with_state(input_area(rows[4]), state.input_price_state.clone()),
            ModelField::OutputPrice => state
                .output_price
                .cursor_pos_with_state(input_area(rows[5]), state.output_price_state.clone()),
            _ => None,
        };
    }
}
