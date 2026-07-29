//! Model selection widget: a **tree view** of built-in provider catalogues.
//!
//! Providers and models are **built-in** (from `agentik_sdk::provider::registry`).
//! The database only stores credentials (api_key / base_url override) — if a
//! provider has a row in the `providers` table with a non-empty `api_key`, it
//! is "configured" and its models become selectable.
//!
//! **Tree layout**:
//! ```text
//! ▼ deepseek ✓                ← configured, expanded (top)
//!   ● deepseek-v4-pro         ← model (indented)
//!     deepseek-v4-flash
//! ▶ mimo ✓                    ← configured, collapsed
//! ── minimax ✗                ← unconfigured (bottom, not expandable)
//! ```
//!
//! **Data flow**: App builds [`ModelConfigState`] from the SDK registry +
//! DB credentials. The widget only reads this state.

use agentik_sdk::model::{ModelInfo, ProviderType};
use agentik_sdk::provider::registry;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph,
        StatefulWidgetRef,
    },
};

use crate::xai_textarea::{TextArea, TextAreaState};

/// Commands that the widget asks the App to execute (for operations requiring
/// external resources like DB access).
#[derive(Debug)]
pub enum ConfigCommand {
    /// Persist the api_key for `provider_name` to the database.
    SaveProvider {
        provider_name: String,
        api_key: String,
    },
    /// Persist and activate the selected model.
    SelectModel {
        provider_name: String,
        model_name: String,
    },
    /// Nothing to do.
    None,
}

// ── Data model ─────────────────────────────────────────

/// One built-in provider entry, augmented with the user's configuration state.
pub struct CatalogProvider {
    pub provider_type: ProviderType,
    /// Display name derived from the provider type (e.g. `"deepseek"`).
    pub name: String,
    /// Default base URL from the built-in preset.
    pub base_url: String,
    /// Built-in model catalogue for this provider.
    pub models: Vec<ModelInfo>,
    /// Whether the user has configured credentials for this provider.
    pub configured: bool,
    /// The api_key stored in the DB (if configured).
    pub api_key: Option<String>,
    /// Whether the tree node is currently expanded (only meaningful if
    /// `configured` is true).
    pub expanded: bool,
}

/// A row in the flattened tree — used for cursor navigation and rendering.
#[derive(Clone, Copy)]
enum FlatItem {
    /// Provider header row. Index into `ModelConfigState::providers`.
    Provider(usize),
    /// Model row under a configured + expanded provider.
    /// Indices: (provider index, model index within that provider).
    Model(usize, usize),
}

#[derive(Default)]
pub enum ProviderPanelState {
    #[default]
    Preview,
    Config {
        provider_name: String,
        textarea: TextArea,
        textarea_state: TextAreaState,
    },
}

/// In-memory state for the model config widget.
#[derive(Default)]
pub struct ModelConfigState {
    /// Providers sorted: configured first, then alphabetical.
    pub providers: Vec<CatalogProvider>,
    /// Cursor position in the flattened visible list.
    pub cursor: usize,
    /// The `model_name` of the model the agent is currently using, if any.
    pub active_model_name: Option<String>,
    pub provider_panel_state: ProviderPanelState,
}

impl ModelConfigState {
    /// Build the flat list of visible rows (provider headers + indented models
    /// for configured/expanded providers).
    fn flat_items(&self) -> Vec<FlatItem> {
        let mut items = Vec::new();
        for (pi, p) in self.providers.iter().enumerate() {
            items.push(FlatItem::Provider(pi));
            if p.configured && p.expanded {
                for (mi, _) in p.models.iter().enumerate() {
                    items.push(FlatItem::Model(pi, mi));
                }
            }
        }
        items
    }

    /// Move cursor by `delta`, wrapping around.
    pub fn move_cursor(&mut self, delta: i32) {
        let items = self.flat_items();
        if items.is_empty() {
            return;
        }
        let len = items.len() as i32;
        let mut i = self.cursor as i32 + delta;
        if i < 0 {
            i = len - 1;
        } else if i >= len {
            i = 0;
        }
        self.cursor = i as usize;
    }

    /// Toggle expansion of the provider at the cursor (no-op if unconfigured).
    pub fn toggle_expand_at_cursor(&mut self) {
        let items = self.flat_items();
        if let Some(FlatItem::Provider(pi)) = items.get(self.cursor) {
            let pi = *pi;
            if self.providers[pi].configured {
                self.providers[pi].expanded = !self.providers[pi].expanded;
            }
        }
    }

    /// The model info at the cursor position, if the cursor is on a model row.
    pub fn model_at_cursor(&self) -> Option<&ModelInfo> {
        let items = self.flat_items();
        match items.get(self.cursor)? {
            FlatItem::Model(pi, mi) => self.providers.get(*pi)?.models.get(*mi),
            _ => None,
        }
    }

    /// Handle keys and return any command the App should execute.
    ///
    /// - **Preview mode**: navigation, expand/collapse, select model, `e` to edit.
    /// - **Config mode**: typing into the api_key field, Esc to cancel, Enter
    ///   to confirm (returns `SaveProvider` command for DB persistence).
    ///
    /// Returns `ConfigCommand::None` for unrecognized keys so the caller
    /// (App) can handle keys like `r` (reload) that need DB access.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> ConfigCommand {
        use crossterm::event::{KeyCode, KeyEvent};
        let key: KeyEvent = key;

        let consumed = |cc: ConfigCommand| {
            // Mark as consumed by not returning None.
            cc
        };

        // ── Config mode: all keys go to the textarea (except Esc/Enter) ──
        if let ProviderPanelState::Config { textarea, .. } = &mut self.provider_panel_state {
            return match key.code {
                KeyCode::Esc => {
                    self.provider_panel_state = ProviderPanelState::Preview;
                    consumed(ConfigCommand::None)
                }
                KeyCode::Enter => {
                    // Confirm: write the entered api_key back to the provider entry.
                    let mut command = ConfigCommand::None;
                    if let ProviderPanelState::Config { provider_name, textarea, .. } =
                        std::mem::replace(&mut self.provider_panel_state, ProviderPanelState::Preview)
                    {
                        let key_val = textarea.text().trim().to_string();
                        if let Some(p) = self
                            .providers
                            .iter_mut()
                            .find(|p| p.name == provider_name)
                        {
                            p.api_key = if key_val.is_empty() { None } else { Some(key_val.clone()) };
                            p.configured = p.api_key.is_some();
                            p.expanded = p.configured;
                        }
                        // Ask App to persist to DB.
                        command = ConfigCommand::SaveProvider {
                            provider_name,
                            api_key: key_val,
                        };
                    }
                    consumed(command)
                }
                _ => {
                    textarea.input(key);
                    consumed(ConfigCommand::None)
                }
            };
        }

        // ── Preview mode ──
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_cursor(1);
                consumed(ConfigCommand::None)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_cursor(-1);
                consumed(ConfigCommand::None)
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                self.toggle_expand_at_cursor();
                consumed(ConfigCommand::None)
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                self.toggle_expand_at_cursor();
                consumed(ConfigCommand::None)
            }
            KeyCode::Enter => {
                let flat = self.flat_items();
                if let Some(FlatItem::Model(pi, mi)) = flat.get(self.cursor).copied() {
                    let provider = &self.providers[pi];
                    let model = &provider.models[mi];
                    self.active_model_name = Some(model.model_name.clone());
                    consumed(ConfigCommand::SelectModel {
                        provider_name: provider.name.clone(),
                        model_name: model.model_name.clone(),
                    })
                } else {
                    consumed(ConfigCommand::None)
                }
            }
            // Enter provider config mode when cursor is on a provider row.
            KeyCode::Char('e') => {
                let flat = self.flat_items();
                if let Some(FlatItem::Provider(pi)) = flat.get(self.cursor) {
                    let pi = *pi;
                    let provider = &self.providers[pi];
                    let mut textarea = TextArea::new();
                    if let Some(ref key) = provider.api_key {
                        textarea.set_text(key);
                    }
                    self.provider_panel_state = ProviderPanelState::Config {
                        provider_name: provider.name.clone(),
                        textarea,
                        textarea_state: TextAreaState::default(),
                    };
                }
                consumed(ConfigCommand::None)
            }
            _ => ConfigCommand::None,
        }
    }
}

// ── Widget ─────────────────────────────────────────────

pub struct ModelConfigWidget;

impl StatefulWidgetRef for ModelConfigWidget {
    type State = ModelConfigState;

    fn render_ref(&self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        // Tree is always rendered on the left.
        render_tree(chunks[0], buf, state);

        match &mut state.provider_panel_state {
            ProviderPanelState::Preview => {
                render_detail(chunks[1], buf, state);
            }
            ProviderPanelState::Config { provider_name, textarea, textarea_state } => {
                render_config_panel(
                    chunks[1],
                    buf,
                    &state.providers,
                    provider_name,
                    textarea,
                    textarea_state,
                );
            }
        }
    }
}

// ── Tree list ──────────────────────────────────────────

fn render_tree(area: Rect, buf: &mut Buffer, state: &ModelConfigState) {
    let flat = state.flat_items();

    let items: Vec<ListItem> = flat
        .iter()
        .map(|&item| match item {
            FlatItem::Provider(pi) => {
                let p = &state.providers[pi];
                let (arrow, style) = if !p.configured {
                    ("──", Style::default().fg(Color::DarkGray))
                } else if p.expanded {
                    ("▼", Style::default().fg(Color::Yellow))
                } else {
                    ("▶", Style::default().fg(Color::Yellow))
                };
                let check = if p.configured { "✓" } else { "✗" };
                let check_color = if p.configured {
                    Color::Green
                } else {
                    Color::DarkGray
                };

                ListItem::new(Line::from(vec![
                    Span::styled(format!("{arrow} "), style),
                    Span::styled(
                        p.name.clone(),
                        if p.configured {
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        },
                    ),
                    Span::styled(format!(" {check}"), Style::default().fg(check_color)),
                ]))
            }
            FlatItem::Model(pi, mi) => {
                let p = &state.providers[pi];
                let m = &p.models[mi];
                let active = state.active_model_name.as_deref() == Some(m.model_name.as_str());
                let icon = if active { "●" } else { " " };
                let icon_color = if active {
                    Color::Green
                } else {
                    Color::DarkGray
                };
                let indent = "    "; // 4 spaces under provider

                ListItem::new(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(format!("{icon} "), Style::default().fg(icon_color)),
                    Span::styled(
                        m.model_name.clone(),
                        Style::default().fg(if active { Color::White } else { Color::Gray }),
                    ),
                ]))
            }
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Line::from(vec![
                    Span::styled(" Providers ", Style::default().fg(Color::Yellow)),
                    Span::styled(
                        format!(
                            "({} configured) ",
                            state.providers.iter().filter(|p| p.configured).count()
                        ),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut list_state = ListState::default();
    list_state.select(if flat.is_empty() {
        None
    } else {
        Some(state.cursor)
    });
    ratatui::widgets::StatefulWidget::render(list, area, buf, &mut list_state);
}

// ── Detail panel ───────────────────────────────────────

fn render_detail(area: Rect, buf: &mut Buffer, state: &ModelConfigState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            " Details ",
            Style::default().fg(Color::Yellow),
        ));
    let inner = block.inner(area);
    block.render(area, buf);

    let flat = state.flat_items();
    let Some(&item) = flat.get(state.cursor) else {
        let p = Paragraph::new("  (nothing selected)").style(Style::default().fg(Color::DarkGray));
        ratatui::widgets::Widget::render(p, inner, buf);
        return;
    };

    let lines = match item {
        FlatItem::Provider(pi) => {
            let p = &state.providers[pi];
            let label =
                |k: &str| Span::styled(format!(" {:<14}: ", k), Style::default().fg(Color::Cyan));
            let val = |v: String| Span::styled(v, Style::default().fg(Color::White));
            let dim = |v: String| Span::styled(v, Style::default().fg(Color::DarkGray));
            let green = |v: String| Span::styled(v, Style::default().fg(Color::Green));
            let red = |v: String| Span::styled(v, Style::default().fg(Color::Red));

            vec![
                Line::from(vec![label("Provider"), val(p.name.clone())]),
                Line::from(vec![label("Type"), dim(format!("{:?}", p.provider_type))]),
                Line::from(vec![label("Base URL"), dim(p.base_url.clone())]),
                Line::from(vec![
                    label("Status"),
                    if p.configured {
                        green("configured".to_string())
                    } else {
                        red("not configured".to_string())
                    },
                ]),
                Line::from(vec![
                    label("Models"),
                    val(format!("{} (built-in)", p.models.len())),
                ]),
            ]
        }
        FlatItem::Model(pi, mi) => {
            let p = &state.providers[pi];
            let m = &p.models[mi];
            let active = state.active_model_name.as_deref() == Some(m.model_name.as_str());
            let label =
                |k: &str| Span::styled(format!(" {:<14}: ", k), Style::default().fg(Color::Cyan));
            let val = |v: String| Span::styled(v, Style::default().fg(Color::White));
            let dim = |v: String| Span::styled(v, Style::default().fg(Color::DarkGray));

            vec![
                Line::from(vec![
                    label("Model"),
                    val(m.model_name.clone()),
                    if active {
                        Span::styled("  ● active", Style::default().fg(Color::Green))
                    } else {
                        Span::raw("")
                    },
                ]),
                Line::from(vec![label("Provider"), val(p.name.clone())]),
                Line::from(vec![
                    label("Context Len"),
                    val(format!("{}", m.context_length)),
                ]),
                Line::from(vec![
                    label("Max Output"),
                    val(format!("{}", m.max_output_tokens)),
                ]),
                Line::from(vec![label("FnCall"), val(yn(m.supports_function_calling))]),
                Line::from(vec![label("Streaming"), val(yn(m.supports_streaming))]),
                Line::from(vec![label("Thinking"), val(yn(m.supports_thinking))]),
                Line::from(vec![label("Vision"), val(yn(m.vision_ability))]),
                Line::from(vec![
                    label("In Price"),
                    dim(format!("${:.2}/M tok", m.input_token_price)),
                ]),
                Line::from(vec![
                    label("Out Price"),
                    dim(format!("${:.2}/M tok", m.output_token_price)),
                ]),
            ]
        }
    };

    let p = Paragraph::new(lines).block(Block::default().padding(Padding::vertical(1)));
    ratatui::widgets::Widget::render(p, inner, buf);
}

// ── Config panel (provider credential editor) ──────────

fn render_config_panel(
    area: Rect,
    buf: &mut Buffer,
    providers: &[CatalogProvider],
    provider_name: &str,
    textarea: &TextArea,
    textarea_state: &mut TextAreaState,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Line::from(vec![
            Span::styled(" Configure ", Style::default().fg(Color::Yellow)),
            Span::raw(" "),
            Span::styled(provider_name, Style::default().fg(Color::White)),
        ]))
        .title_bottom(
            Line::from("[Enter] save  [Esc] cancel").alignment(Alignment::Center),
        );
    let inner = block.inner(area);
    block.render(area, buf);

    let provider = providers.iter().find(|p| p.name == provider_name);

    let label_style = Style::default().fg(Color::Cyan);
    let value_style = Style::default().fg(Color::White);
    let dim_style = Style::default().fg(Color::DarkGray);

    // Rows: read-only info (3) + gap + API Key label + textarea input + spacer
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Provider
            Constraint::Length(1), // Type
            Constraint::Length(1), // Base URL
            Constraint::Length(1), // blank
            Constraint::Length(1), // API Key label
            Constraint::Length(3), // textarea (bordered box)
            Constraint::Min(0),    // spacer
        ])
        .split(inner);

    Paragraph::new(Line::from(vec![
        Span::styled(" Provider       : ", label_style),
        Span::styled(provider_name.to_string(), value_style),
    ]))
    .render(rows[0], buf);

    let type_str = provider
        .map(|p| format!("{:?}", p.provider_type))
        .unwrap_or_default();
    Paragraph::new(Line::from(vec![
        Span::styled(" Type           : ", label_style),
        Span::styled(type_str, dim_style),
    ]))
    .render(rows[1], buf);

    let base_url = provider.map(|p| p.base_url.as_str()).unwrap_or("");
    Paragraph::new(Line::from(vec![
        Span::styled(" Base URL       : ", label_style),
        Span::styled(base_url.to_string(), dim_style),
    ]))
    .render(rows[2], buf);

    // API Key label
    Paragraph::new(Line::from(vec![
        Span::styled(" API Key        :", label_style),
    ]))
    .render(rows[4], buf);

    // Textarea: render into a bordered sub-area inside rows[5].
    let ta_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let ta_inner = ta_block.inner(rows[5]);
    ta_block.render(rows[5], buf);

    if ta_inner.width > 0 && ta_inner.height > 0 {
        let ta_ref: &TextArea = textarea;
        ta_ref.render_ref(ta_inner, buf, textarea_state);
    }
}

fn yn(b: bool) -> String {
    if b {
        "Yes".to_string()
    } else {
        "No".to_string()
    }
}

// ── Catalog builder (called by App) ────────────────────

/// Build the full provider catalogue from the SDK registry, marking each
/// provider's configured state based on the DB rows passed in.
///
/// `db_providers` is a map of `provider_type` string → `(api_key, base_url_override)`.
/// Providers present in this map with a non-empty `api_key` are "configured".
///
/// The result is sorted: configured providers first (alphabetical), then
/// unconfigured (alphabetical). Configured providers start expanded.
pub fn build_catalog(
    db_providers: &[(String, String, String)], // (provider_type, api_key, base_url)
) -> ModelConfigState {
    let types = registry::known_provider_types();

    let mut providers: Vec<CatalogProvider> = types
        .into_iter()
        .map(|type_str| {
            let provider_type = ProviderType::from(type_str);
            let models = registry::preset_models(&provider_type).unwrap_or_default();
            let base_url = registry::default_base_url(&provider_type)
                .unwrap_or("")
                .to_string();

            // Check DB for configuration (only api_key matters — base_url
            // always comes from the registry so code updates take effect
            // without needing to re-write the DB).
            let db_match = db_providers
                .iter()
                .find(|(pt, _, _)| pt.eq_ignore_ascii_case(type_str));
            let (configured, api_key) = match db_match {
                Some((_, key, _)) if !key.is_empty() => {
                    (true, Some(key.clone()))
                }
                _ => (false, None),
            };

            CatalogProvider {
                name: type_str.to_string(),
                provider_type,
                base_url,
                models,
                configured,
                api_key,
                // Configured providers start expanded.
                expanded: configured,
            }
        })
        .collect();

    // Sort: configured first, then alphabetical by name.
    providers.sort_by(|a, b| match (a.configured, b.configured) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    ModelConfigState {
        providers,
        cursor: 0,
        active_model_name: None,
        provider_panel_state: Default::default(),
    }
}
