//! Model selection widget: a **tree view** of built-in provider catalogues.
//!
//! Providers and models are **built-in** (from `agentik_sdk::provider::registry`).
//! The database stores credentials (api_key) and the chosen base_url override.
//! If a provider has a row in the `providers` table with a non-empty `api_key`,
//! it is "configured" and its models become selectable.
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
//! **Config panel**: when the user presses `e` on a provider row, a
//! credential editor opens on the right pane with two fields —
//! **API Key** and **Base URL** — switchable via `Tab`. Within the Base URL
//! field, `Up`/`Down` cycle through the provider's preset endpoints (leaving
//! the textarea free for custom URL entry).
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
    /// Persist the api_key and base_url for `provider_name` to the database.
    SaveProvider {
        provider_name: String,
        api_key: String,
        base_url: String,
    },
    /// Persist and activate the selected model.
    SelectModel {
        provider_name: String,
        model_name: String,
    },
    /// Close the model config popup.
    Close,
    /// Nothing to do.
    None,
}

// ── Data model ─────────────────────────────────────────

/// One built-in provider entry, augmented with the user's configuration state.
pub struct CatalogProvider {
    pub provider_type: ProviderType,
    /// Display name derived from the provider type (e.g. `"deepseek"`).
    pub name: String,
    /// All known base URL endpoints for this provider (default first).
    /// Source: [`registry::known_base_urls`].
    pub base_urls: Vec<String>,
    /// Currently selected base URL — either a DB-stored override or the
    /// provider default. Drawn from the DB on `build_catalog`.
    pub selected_base_url: String,
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

/// Which input field in the config panel currently has focus.
#[derive(Default, PartialEq, Eq, Copy, Clone)]
pub enum ConfigField {
    #[default]
    ApiKey,
    BaseUrl,
}

#[derive(Default)]
pub enum ProviderPanelState {
    #[default]
    Preview,
    Config {
        provider_name: String,
        api_key: TextArea,
        api_key_state: TextAreaState,
        base_url: TextArea,
        base_url_state: TextAreaState,
        /// All preset URLs for this provider — cycled with `Up`/`Down` while
        /// the Base URL field is focused.
        base_url_presets: Vec<String>,
        /// Which of the two fields is currently being typed into.
        focused_field: ConfigField,
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
    /// Quick-filter search query. When non-empty, the tree auto-expands to
    /// show only providers/models whose names match (case-insensitive
    /// substring).  Typing any character pushes to the query; Backspace
    /// pops; Esc clears (press Esc again to close).
    pub query: String,
}

impl ModelConfigState {
    /// Build the flat list of visible rows.
    ///
    /// **No filter**: provider headers + indented models for
    /// configured/expanded providers.
    ///
    /// **With filter** (query non-empty): providers whose name matches show
    /// all their models (configured only); configured providers with matching
    /// model names show only those models; unconfigured providers whose name
    /// matches show as a header with no children.  Expansion state is
    /// ignored while filtering — matched content is always visible.
    fn flat_items(&self) -> Vec<FlatItem> {
        let needle = self.query.trim().to_lowercase();
        let filtering = !needle.is_empty();
        let mut items = Vec::new();
        for (pi, p) in self.providers.iter().enumerate() {
            if filtering {
                let provider_name_match = p.name.to_lowercase().contains(&needle);
                // Only show models from configured providers while filtering.
                let matching_models: Vec<usize> = if !p.configured {
                    Vec::new()
                } else if provider_name_match {
                    (0..p.models.len()).collect()
                } else {
                    p.models
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.model_name.to_lowercase().contains(&needle))
                        .map(|(mi, _)| mi)
                        .collect()
                };
                if provider_name_match || !matching_models.is_empty() {
                    items.push(FlatItem::Provider(pi));
                    for mi in matching_models {
                        items.push(FlatItem::Model(pi, mi));
                    }
                }
            } else {
                items.push(FlatItem::Provider(pi));
                if p.configured && p.expanded {
                    for (mi, _) in p.models.iter().enumerate() {
                        items.push(FlatItem::Model(pi, mi));
                    }
                }
            }
        }
        items
    }

    /// Push a character onto the search query and re-clamp the cursor.
    fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.clamp_cursor();
    }

    /// Pop the last character from the search query.
    fn pop_char(&mut self) {
        self.query.pop();
        self.clamp_cursor();
    }

    /// Clamp cursor into the valid range of the current flat list.
    fn clamp_cursor(&mut self) {
        let len = self.flat_items().len();
        if len == 0 {
            self.cursor = 0;
        } else if self.cursor >= len {
            self.cursor = len - 1;
        }
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
    /// - **Config mode**: typing into the focused field (API Key or Base URL),
    ///   `Tab` to switch focus, `Up`/`Down` on the Base URL field to cycle
    ///   through preset endpoints, Esc to cancel, Enter to confirm (returns
    ///   `SaveProvider` command for DB persistence).
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

        // ── Config mode ──
        if let ProviderPanelState::Config {
            api_key,
            base_url,
            base_url_presets,
            focused_field,
            ..
        } = &mut self.provider_panel_state
        {
            return match key.code {
                KeyCode::Esc => {
                    self.provider_panel_state = ProviderPanelState::Preview;
                    consumed(ConfigCommand::None)
                }
                // Tab / BackTab: switch focus between the two fields.
                KeyCode::Tab | KeyCode::BackTab => {
                    *focused_field = match *focused_field {
                        ConfigField::ApiKey => ConfigField::BaseUrl,
                        ConfigField::BaseUrl => ConfigField::ApiKey,
                    };
                    consumed(ConfigCommand::None)
                }
                // Up/Down on Base URL field: cycle preset endpoints.
                KeyCode::Up
                    if *focused_field == ConfigField::BaseUrl && !base_url_presets.is_empty() =>
                {
                    let cur = base_url.text();
                    let idx = base_url_presets
                        .iter()
                        .position(|u| u == cur)
                        .map(|i| {
                            if i == 0 {
                                base_url_presets.len() - 1
                            } else {
                                i - 1
                            }
                        })
                        .unwrap_or(0);
                    base_url.set_text(&base_url_presets[idx]);
                    consumed(ConfigCommand::None)
                }
                KeyCode::Down
                    if *focused_field == ConfigField::BaseUrl && !base_url_presets.is_empty() =>
                {
                    let cur = base_url.text();
                    let idx = base_url_presets
                        .iter()
                        .position(|u| u == cur)
                        .map(|i| (i + 1) % base_url_presets.len())
                        .unwrap_or(0);
                    base_url.set_text(&base_url_presets[idx]);
                    consumed(ConfigCommand::None)
                }
                KeyCode::Enter => {
                    // Confirm: write the entered values back to the provider entry.
                    let mut command = ConfigCommand::None;
                    if let ProviderPanelState::Config {
                        provider_name,
                        api_key,
                        base_url,
                        ..
                    } = std::mem::replace(
                        &mut self.provider_panel_state,
                        ProviderPanelState::Preview,
                    ) {
                        let key_val = api_key.text().trim().to_string();
                        let url_val = base_url.text().trim().to_string();
                        if let Some(p) = self.providers.iter_mut().find(|p| p.name == provider_name)
                        {
                            p.api_key = if key_val.is_empty() {
                                None
                            } else {
                                Some(key_val.clone())
                            };
                            p.configured = p.api_key.is_some();
                            p.expanded = p.configured;
                            if !url_val.is_empty() {
                                p.selected_base_url = url_val.clone();
                            }
                        }
                        // Ask App to persist to DB.
                        command = ConfigCommand::SaveProvider {
                            provider_name,
                            api_key: key_val,
                            base_url: url_val,
                        };
                    }
                    consumed(command)
                }
                _ => {
                    // Forward to the focused textarea.
                    match *focused_field {
                        ConfigField::ApiKey => api_key.input(key),
                        ConfigField::BaseUrl => base_url.input(key),
                    }
                    consumed(ConfigCommand::None)
                }
            };
        }

        // ── Preview mode ──
        match key.code {
            KeyCode::Esc => {
                if !self.query.is_empty() {
                    self.query.clear();
                    self.clamp_cursor();
                    consumed(ConfigCommand::None)
                } else {
                    consumed(ConfigCommand::Close)
                }
            }
            // Arrow keys always navigate, even while filtering.
            KeyCode::Down => {
                self.move_cursor(1);
                consumed(ConfigCommand::None)
            }
            KeyCode::Up => {
                self.move_cursor(-1);
                consumed(ConfigCommand::None)
            }
            // Vim-style navigation only when not filtering.
            KeyCode::Char('j') if self.query.is_empty() => {
                self.move_cursor(1);
                consumed(ConfigCommand::None)
            }
            KeyCode::Char('k') if self.query.is_empty() => {
                self.move_cursor(-1);
                consumed(ConfigCommand::None)
            }
            KeyCode::Right | KeyCode::Tab if self.query.is_empty() => {
                self.toggle_expand_at_cursor();
                consumed(ConfigCommand::None)
            }
            KeyCode::Left | KeyCode::BackTab if self.query.is_empty() => {
                self.toggle_expand_at_cursor();
                consumed(ConfigCommand::None)
            }
            KeyCode::Char('l') if self.query.is_empty() => {
                self.toggle_expand_at_cursor();
                consumed(ConfigCommand::None)
            }
            KeyCode::Char('h') if self.query.is_empty() => {
                self.toggle_expand_at_cursor();
                consumed(ConfigCommand::None)
            }
            // Backspace pops from the search query.
            KeyCode::Backspace => {
                self.pop_char();
                consumed(ConfigCommand::None)
            }
            KeyCode::Enter => {
                let flat = self.flat_items();
                if let Some(FlatItem::Model(pi, mi)) = flat.get(self.cursor).copied() {
                    let provider = &self.providers[pi];
                    // Only allow selecting models from configured providers.
                    if !provider.configured {
                        return consumed(ConfigCommand::None);
                    }
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
            // Enter provider config mode when cursor is on a provider row
            // (only when not filtering).
            KeyCode::Char('e') if self.query.is_empty() => {
                let flat = self.flat_items();
                if let Some(FlatItem::Provider(pi)) = flat.get(self.cursor) {
                    let pi = *pi;
                    let provider = &self.providers[pi];
                    let mut api_key_ta = TextArea::new();
                    if let Some(ref key) = provider.api_key {
                        api_key_ta.set_text(key);
                    }
                    let mut base_url_ta = TextArea::new();
                    base_url_ta.set_text(&provider.selected_base_url);
                    self.provider_panel_state = ProviderPanelState::Config {
                        provider_name: provider.name.clone(),
                        api_key: api_key_ta,
                        api_key_state: TextAreaState::default(),
                        base_url: base_url_ta,
                        base_url_state: TextAreaState::default(),
                        base_url_presets: provider.base_urls.clone(),
                        focused_field: ConfigField::ApiKey,
                    };
                }
                consumed(ConfigCommand::None)
            }
            // Any other printable character is appended to the search query.
            KeyCode::Char(c) => {
                self.push_char(c);
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
        // Vertical: search bar (1) + separator (1) + content (rest).
        let v_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // search input
                Constraint::Length(1), // separator
                Constraint::Min(3),    // content
            ])
            .split(area);

        render_search_bar(v_chunks[0], buf, state);

        // Separator line.
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray))
            .render(v_chunks[1], buf);

        // Content area: tree (left) + detail/config (right).
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(v_chunks[2]);

        // Tree is always rendered on the left.
        render_tree(chunks[0], buf, state);

        match &mut state.provider_panel_state {
            ProviderPanelState::Preview => {
                render_detail(chunks[1], buf, state);
            }
            ProviderPanelState::Config {
                provider_name,
                api_key,
                api_key_state,
                base_url,
                base_url_state,
                base_url_presets,
                focused_field,
            } => {
                render_config_panel(
                    chunks[1],
                    buf,
                    &state.providers,
                    provider_name,
                    api_key,
                    api_key_state,
                    base_url,
                    base_url_state,
                    base_url_presets,
                    *focused_field,
                );
            }
        }
    }
}

// ── Search bar ─────────────────────────────────────────

fn render_search_bar(area: Rect, buf: &mut Buffer, state: &ModelConfigState) {
    let line = if state.query.is_empty() {
        Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                " search models…  (type to filter, Enter to select, Esc to clear)",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
        ])
    } else {
        let filtered_count = state.flat_items().len();
        Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Magenta)),
            Span::styled(
                state.query.clone(),
                Style::default().fg(Color::White),
            ),
            Span::styled("▏", Style::default().fg(Color::Magenta)),
            Span::styled(
                format!("  {} items", filtered_count),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    };
    Widget::render(Paragraph::new(line), area, buf);
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
                Line::from(vec![label("Base URL"), dim(p.selected_base_url.clone())]),
                Line::from(vec![
                    label("Endpoints"),
                    dim(format!("{}", p.base_urls.len())),
                ]),
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
                Line::from(vec![
                    label("Thinking"),
                    val(format!(
                        "{}{}",
                        yn(m.supports_thinking),
                        if m.thinking_enabled { " (on)" } else { "" }
                    )),
                ]),
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

#[allow(clippy::too_many_arguments)]
fn render_config_panel(
    area: Rect,
    buf: &mut Buffer,
    providers: &[CatalogProvider],
    provider_name: &str,
    api_key: &TextArea,
    api_key_state: &mut TextAreaState,
    base_url: &TextArea,
    base_url_state: &mut TextAreaState,
    base_url_presets: &[String],
    focused_field: ConfigField,
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
            Line::from("[Tab] switch field  [↑/↓] cycle URL preset  [Enter] save  [Esc] cancel")
                .alignment(Alignment::Center),
        );
    let inner = block.inner(area);
    block.render(area, buf);

    let provider = providers.iter().find(|p| p.name == provider_name);

    let label_style = Style::default().fg(Color::Cyan);
    let focused_label_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let value_style = Style::default().fg(Color::White);
    let dim_style = Style::default().fg(Color::DarkGray);

    // Rows: Provider + Type + blank + API Key label + api_key textarea +
    //       blank + Base URL label + base_url textarea + spacer.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Provider
            Constraint::Length(1), // Type
            Constraint::Length(1), // blank
            Constraint::Length(1), // API Key label
            Constraint::Length(3), // api_key textarea (bordered box)
            Constraint::Length(1), // blank
            Constraint::Length(1), // Base URL label
            Constraint::Length(3), // base_url textarea (bordered box)
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

    // ── API Key field ──
    let api_label_style = if focused_field == ConfigField::ApiKey {
        focused_label_style
    } else {
        label_style
    };
    Paragraph::new(Line::from(vec![
        Span::styled(" API Key        :", api_label_style),
        Span::raw(" "),
        Span::styled("(required)", dim_style),
    ]))
    .render(rows[3], buf);

    let api_block = Block::default()
        .borders(Borders::ALL)
        .border_style(
            Style::default().fg(if focused_field == ConfigField::ApiKey {
                Color::Yellow
            } else {
                Color::DarkGray
            }),
        );
    let api_inner = api_block.inner(rows[4]);
    api_block.render(rows[4], buf);
    if api_inner.width > 0 && api_inner.height > 0 {
        api_key.render_ref(api_inner, buf, api_key_state);
    }

    // ── Base URL field ──
    // Compose a preset indicator: "[i/N preset]" if the textarea text matches
    // a known preset, otherwise "[custom]".
    let cur_url = base_url.text();
    let preset_tag = if cur_url.is_empty() {
        "(empty)".to_string()
    } else if let Some(idx) = base_url_presets.iter().position(|u| u == cur_url) {
        format!("[{}/{} preset]", idx + 1, base_url_presets.len())
    } else {
        "[custom]".to_string()
    };
    let url_label_style = if focused_field == ConfigField::BaseUrl {
        focused_label_style
    } else {
        label_style
    };
    Paragraph::new(Line::from(vec![
        Span::styled(" Base URL       :", url_label_style),
        Span::raw(" "),
        Span::styled(preset_tag, dim_style),
    ]))
    .render(rows[6], buf);

    let url_block = Block::default()
        .borders(Borders::ALL)
        .border_style(
            Style::default().fg(if focused_field == ConfigField::BaseUrl {
                Color::Yellow
            } else {
                Color::DarkGray
            }),
        );
    let url_inner = url_block.inner(rows[7]);
    url_block.render(rows[7], buf);
    if url_inner.width > 0 && url_inner.height > 0 {
        base_url.render_ref(url_inner, buf, base_url_state);
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
/// `db_providers` is a map of `provider_type` string →
/// `(api_key, base_url_override)`. Providers present in this map with a
/// non-empty `api_key` are "configured".
///
/// The selected base URL is the DB-stored override when present, otherwise
/// the provider default. The full list of preset endpoints
/// ([`registry::known_base_urls`]) is exposed for switching in the editor.
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
            let preset_urls: Vec<String> = registry::known_base_urls(&provider_type)
                .into_iter()
                .map(|s| s.to_string())
                .collect();
            let default_url = preset_urls.first().cloned().unwrap_or_else(|| {
                registry::default_base_url(&provider_type)
                    .unwrap_or("")
                    .to_string()
            });

            // Match this provider against the DB rows.
            let db_match = db_providers
                .iter()
                .find(|(pt, _, _)| pt.eq_ignore_ascii_case(type_str));
            let (configured, api_key) = match db_match {
                Some((_, key, _)) if !key.is_empty() => (true, Some(key.clone())),
                _ => (false, None),
            };
            // Selected base URL: DB override if non-empty, else the default.
            let selected_base_url = db_match
                .and_then(|(_, _, url)| {
                    if url.is_empty() {
                        None
                    } else {
                        Some(url.clone())
                    }
                })
                .unwrap_or(default_url);

            CatalogProvider {
                name: type_str.to_string(),
                provider_type,
                base_urls: preset_urls,
                selected_base_url,
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
        query: String::new(),
    }
}
