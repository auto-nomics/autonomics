use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock};

use crate::config_db::{ModelInput, ProviderInput, ProviderRow};
use crate::widgets::input_area::{InputArea, InputState};
use agentik_sdk::model::Model;
use agentik_sdk::types::{AgentEvent, CompactEvent};
use arc_swap::ArcSwapOption;
use ratatui::text::Line;

pub const TABS: &[&str] = &["Agent", "Config"];

#[derive(Default)]
pub enum MainTabState {
    #[default]
    AgentTab,
    ConfigTab,
}

impl MainTabState {
    pub const fn index(&self) -> usize {
        match self {
            Self::AgentTab => 0,
            Self::ConfigTab => 1,
        }
    }

    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::AgentTab,
            _ => Self::ConfigTab,
        }
    }
}

// ── Tool task tracking ─────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolTaskStatus {
    Running,
    Done { ok: bool },
}

#[derive(Debug, Clone)]
pub struct ToolTaskInfo {
    pub id: String,
    pub name: String,
    pub status: ToolTaskStatus,
}

// ── Chat data model ─────────────────────────────────────

/// Token usage for a single LLM turn, attached to the assistant message it
/// produced. `input_tokens` is `Option` because the streaming protocol only
/// reports it on the final delta of a turn.
#[derive(Debug, Clone, Copy)]
pub struct TurnUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum ChatLine {
    User(String),
    Assistant {
        text: String,
        usage: Option<TurnUsage>,
    },
    Thinking(String),
    ToolCall {
        name: String,
        input: String,
    },
    ToolResult {
        ok: bool,
        content: String,
    },
    /// Tool is running in the background (sync phase expired).
    ToolBackground {
        id: String,
        name: String,
    },
    Error(String),
    Separator,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatus {
    Idle,
    Requesting,
    Streaming,
    Error,
}

/// Modal state of the agent tab's input surface.
///
/// `Browse` scrolls the transcript; `Input` is the composing mode where
/// keystrokes insert text and Enter sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Browse,
    /// Insert mode — keystrokes insert text; Enter sends.
    Input,
}

/// Mutable state for the Agent tab.
pub struct AgentTabState {
    pub messages: Vec<ChatLine>,
    pub tool_tasks: Vec<ToolTaskInfo>,
    pub scroll_offset: usize,
    pub status: AgentStatus,
    pub input: InputArea,
    /// History of submitted prompts, newest at the back. Used by
    /// `Up`/`Down` to recall previous messages.
    pub input_history: VecDeque<String>,
    /// Maximum number of retained history entries. Oldest get evicted
    /// when capacity is reached.
    pub input_history_capacity: usize,
    /// Buffer snapshot saved when the user first presses `Up` so the
    /// in-progress edit is restored on `Down` past the oldest recall.
    pub input_draft: Option<String>,
    /// `None` = editing the user's draft (no recall in progress).
    /// `Some(idx)` = displaying `input_history[idx]`. The first non-`Up`/
    /// `Down` keystroke collapses back to draft mode.
    pub input_recall: Option<usize>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub input_mode: InputMode,
    /// When true, `clamp_scroll` forces offset to the bottom each frame.
    pub auto_scroll: bool,
    /// True while an incremental Ctrl+R history search is in progress.
    pub in_history_search: bool,
    /// The current Ctrl+R search query (typed by the user).
    pub history_search_query: String,
    /// Indices into `input_history` that match `history_search_query`,
    /// newest-first. Recomputed whenever the query changes.
    pub history_search_matches: Vec<usize>,
    /// Selected position within `history_search_matches` (0 = newest match).
    pub history_search_selected: usize,
    /// Snapshot of the input buffer saved when Ctrl+R starts, restored on Esc.
    pub history_search_draft: Option<String>,
    /// Cached total rendered line count (updated every frame).
    pub content_line_count: usize,
    /// Per-message content version (parallel to `messages`). Bumped whenever
    /// the rendered output of that message would change (text append, usage
    /// update, etc.). The render cache compares these to decide which messages
    /// need re-rendering.
    pub msg_versions: Vec<u64>,
    /// Monotonic counter backing `msg_versions`; each push/mutation assigns the
    /// next value.
    msg_version_counter: u64,
    /// Per-message rendered-line cache (parallel to `messages`). Entry *i*
    /// holds the last rendered `Vec<Line>` for `messages[i]`.
    pub cached_msg_lines: Vec<Vec<Line<'static>>>,
    /// The `msg_versions[i]` value at which `cached_msg_lines[i]` was rendered.
    /// Zero means stale (never rendered).
    pub cached_msg_versions: Vec<u64>,
    /// Terminal width at which the per-message cache was built.
    pub cached_msg_width: u16,
    /// Index of the assistant line currently being streamed, so incoming
    /// `UsageUpdate` events can be attributed to the right turn. Reset on each
    /// `Requesting` (one per LLM call); `None` for tool-only turns.
    pub streaming_assistant: Option<usize>,
    /// Monotonic frame counter, incremented each render tick while the agent
    /// is active (Requesting / Streaming). Drives loading animations.
    pub frame: u64,
    pub compact_state: CompactState,
}

#[derive(Debug, Default)]
pub struct CompactState {
    pub is_compacting: bool,
}

impl Default for AgentTabState {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            tool_tasks: Vec::new(),
            scroll_offset: 0,
            status: AgentStatus::Idle,
            input: InputArea::new(),
            input_history: VecDeque::new(),
            input_history_capacity: 200,
            input_draft: None,
            input_recall: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            input_mode: InputMode::Browse,
            auto_scroll: true,
            in_history_search: false,
            history_search_query: String::new(),
            history_search_matches: Vec::new(),
            history_search_selected: 0,
            history_search_draft: None,
            content_line_count: 0,
            msg_versions: Vec::new(),
            msg_version_counter: 0,
            cached_msg_lines: Vec::new(),
            cached_msg_versions: Vec::new(),
            cached_msg_width: 0,
            streaming_assistant: None,
            frame: 0,
            compact_state: CompactState::default(),
        }
    }
}

impl AgentTabState {
    /// Returns true when the user can type and send messages.
    pub fn can_send(&self) -> bool {
        self.status == AgentStatus::Idle && !self.input.is_empty()
    }

    /// Take the current input text and clear the input field.
    pub fn take_input(&mut self) -> String {
        let text = self.input.value();
        self.input.clear();
        text
    }

    /// Push a user message and a separator after the previous assistant response.
    pub fn push_user_message(&mut self, text: String) {
        self.push_line(ChatLine::User(text));
    }

    // ── Per-message versioning helpers ──
    //
    // These keep `msg_versions` in lockstep with `messages` so the render
    // cache in `AgentTabWidget` can skip re-rendering unchanged messages.

    /// Push a new chat line, assigning it a fresh content version.
    fn push_line(&mut self, line: ChatLine) {
        self.msg_version_counter = self.msg_version_counter.wrapping_add(1);
        self.messages.push(line);
        self.msg_versions.push(self.msg_version_counter);
    }

    /// Bump the content version of the last message (its content mutated in place).
    fn bump_last_version(&mut self) {
        self.msg_version_counter = self.msg_version_counter.wrapping_add(1);
        if let Some(v) = self.msg_versions.last_mut() {
            *v = self.msg_version_counter;
        }
    }

    /// Bump the content version of the message at `idx`.
    fn bump_version_at(&mut self, idx: usize) {
        self.msg_version_counter = self.msg_version_counter.wrapping_add(1);
        if let Some(v) = self.msg_versions.get_mut(idx) {
            *v = self.msg_version_counter;
        }
    }

    /// Re-enable auto-scroll so the next `clamp_scroll` call pins to the bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.auto_scroll = true;
    }

    /// Clamp scroll_offset based on content and viewport.
    /// When `auto_scroll` is true, forces offset to the maximum (bottom).
    pub fn clamp_scroll(&mut self, viewport_height: u16) {
        let total = self.content_line_count;
        let max_offset = total.saturating_sub(viewport_height as usize);
        if self.auto_scroll {
            self.scroll_offset = max_offset;
        } else {
            self.scroll_offset = self.scroll_offset.min(max_offset);
        }
    }

    /// Returns true when the viewport is showing the bottom of the content.
    pub fn is_at_bottom(&self, viewport_height: u16) -> bool {
        let max_offset = self
            .content_line_count
            .saturating_sub(viewport_height as usize);
        self.scroll_offset >= max_offset
    }
}

// ── Event → State mapping ──────────────────────────────

/// Apply an `AgentEvent` to `AgentTabState`, mutating the conversation view.
pub fn apply_event(state: &mut AgentTabState, event: AgentEvent) {
    match event {
        AgentEvent::Requesting => {
            state.status = AgentStatus::Requesting;
            // A new LLM call begins — clear the streaming-assistant handle so
            // usage from this call isn't attributed to a prior turn's line.
            state.streaming_assistant = None;
        }
        AgentEvent::TextDelta(text) => {
            state.status = AgentStatus::Streaming;
            // Append to last Assistant line, or create a new one
            let last_is_assistant = state
                .messages
                .last()
                .is_some_and(|l| matches!(l, ChatLine::Assistant { .. }));
            if last_is_assistant {
                if let Some(ChatLine::Assistant { text: s, .. }) = state.messages.last_mut() {
                    s.push_str(&text);
                }
                state.bump_last_version();
            } else {
                state.push_line(ChatLine::Assistant { text, usage: None });
                state.streaming_assistant = Some(state.messages.len() - 1);
            }
            state.scroll_to_bottom();
        }
        AgentEvent::ThinkingDelta(text) => {
            let last_is_thinking = state
                .messages
                .last()
                .is_some_and(|l| matches!(l, ChatLine::Thinking(_)));
            if last_is_thinking {
                if let Some(ChatLine::Thinking(s)) = state.messages.last_mut() {
                    s.push_str(&text);
                }
                state.bump_last_version();
            } else {
                state.push_line(ChatLine::Thinking(text));
            }
            state.scroll_to_bottom();
        }
        AgentEvent::UsageUpdate {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
        } => {
            // Attribute this turn's usage to the assistant line being streamed.
            // Overwriting handles multiple deltas within one stream (output_tokens
            // accumulates); tool-only turns have no assistant line and are skipped.
            if let Some(idx) = state.streaming_assistant {
                if let Some(ChatLine::Assistant { usage, .. }) = state.messages.get_mut(idx) {
                    *usage = Some(TurnUsage {
                        input_tokens,
                        output_tokens,
                        cache_creation_input_tokens,
                        cache_read_input_tokens,
                    });
                }
                state.bump_version_at(idx);
            }
            // Cumulative totals for the status bar.
            // `input_tokens` is `Some` only on the final delta of a turn — at that
            // point output_tokens and cache_read_input_tokens also hold the complete
            // per-turn totals, so we accumulate once per turn.
            if let Some(t) = input_tokens {
                state.input_tokens += t;
                state.output_tokens += output_tokens;
                if let Some(c) = cache_read_input_tokens {
                    state.cache_read_tokens += c;
                }
            }
        }
        AgentEvent::LlmResponse(_text) => {
            // The full aggregated response — we already have it via TextDelta.
            // No-op here; the assistant line was built incrementally.
        }
        AgentEvent::Thinking(_text) => {
            // Aggregated thinking block — already streamed via ThinkingDelta.
        }
        AgentEvent::ToolCall { name, input } => {
            state.push_line(ChatLine::ToolCall {
                name,
                input: input.to_string(),
            });
            state.scroll_to_bottom();
        }
        AgentEvent::ToolResult { ok, content } => {
            state.push_line(ChatLine::ToolResult { ok, content });
            state.scroll_to_bottom();
        }
        AgentEvent::ToolCallBackground { id, name } => {
            state.push_line(ChatLine::ToolBackground {
                id: id.clone(),
                name: name.clone(),
            });
            state.tool_tasks.push(ToolTaskInfo {
                id,
                name,
                status: ToolTaskStatus::Running,
            });
            state.scroll_to_bottom();
        }
        AgentEvent::ToolBackgroundComplete { id, ok } => {
            state.push_line(ChatLine::ToolResult {
                ok,
                content: format!("Background task `{id}` has completed"),
            });
            if let Some(task) = state.tool_tasks.iter_mut().find(|t| t.id == id) {
                task.status = ToolTaskStatus::Done { ok };
            }
            state.scroll_to_bottom();
        }
        AgentEvent::Done => {
            state.status = AgentStatus::Idle;
            // Keep still-running background tasks visible across the IDLE
            // window — the agent intentionally goes IDLE while they execute
            // and is woken later by `ToolBackgroundComplete`. Only drop tasks
            // that have already finished.
            state
                .tool_tasks
                .retain(|t| matches!(t.status, ToolTaskStatus::Running));
            state.push_line(ChatLine::Separator);
            state.scroll_to_bottom();
        }
        AgentEvent::Error(msg) => {
            state.push_line(ChatLine::Error(msg));
            state.scroll_to_bottom();
            state.status = AgentStatus::Idle;
        }
        // Streaming protocol events — not surfaced directly to the chat view
        AgentEvent::StreamStart { .. }
        | AgentEvent::ContentBlockStart { .. }
        | AgentEvent::ContentBlockStop { .. }
        | AgentEvent::StreamDelta { .. } => {}
        AgentEvent::Compact { event } => match event {
            CompactEvent::CompactStart { .. } => state.compact_state.is_compacting = true,
            CompactEvent::CompactFinish { .. } => state.compact_state.is_compacting = false,
        },
    }
}

// ── Config tab data model ──────────────────────────────

use crate::config_db::ModelRow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPane {
    Providers,
    Models,
}

/// Which form (if any) is open in the Config tab.
pub enum ConfigMode {
    Browsing,
    EditProvider(ProviderForm),
    EditModel(ModelForm),
}

/// Editable provider record. `id == None` means a new row.
pub struct ProviderForm {
    pub id: Option<i64>,
    pub name: InputState,
    pub provider_type: InputState,
    pub base_url: InputState,
    pub api_key: InputState,
    pub auth_method: InputState,
    pub focus: usize,
}

impl ProviderForm {
    pub const FIELDS: &'static [&'static str] = &["Name", "Type", "Base URL", "API Key", "Auth"];

    pub fn new(input: ProviderInput, id: Option<i64>) -> Self {
        let mk = |v: String| {
            let mut s = InputState::new();
            for ch in v.chars() {
                s.insert_char(ch);
            }
            s
        };
        Self {
            id,
            name: mk(input.name),
            provider_type: mk(input.provider_type),
            base_url: mk(input.base_url),
            api_key: mk(input.api_key),
            auth_method: mk(input.auth_method),
            focus: 0,
        }
    }

    pub fn fields_mut(&mut self) -> [&mut InputState; 5] {
        [
            &mut self.name,
            &mut self.provider_type,
            &mut self.base_url,
            &mut self.api_key,
            &mut self.auth_method,
        ]
    }

    pub fn fields(&self) -> [&InputState; 5] {
        [
            &self.name,
            &self.provider_type,
            &self.base_url,
            &self.api_key,
            &self.auth_method,
        ]
    }

    /// Validate and collect the form into a `ProviderInput`.
    pub fn collect(&self) -> Result<ProviderInput, String> {
        let name = self.name.value().trim().to_string();
        if name.is_empty() {
            return Err("name must not be empty".into());
        }
        Ok(ProviderInput {
            name,
            provider_type: self.provider_type.value().trim().to_string(),
            base_url: self.base_url.value().trim().to_string(),
            api_key: self.api_key.value().trim().to_string(),
            auth_method: self.auth_method.value().trim().to_string(),
        })
    }
}

/// Editable model record. `id == None` means a new row.
pub struct ModelForm {
    pub id: Option<i64>,
    pub model_name: InputState,
    pub provider_index: usize,
    pub context_length: InputState,
    pub max_output_tokens: InputState,
    pub input_token_price: InputState,
    pub output_token_price: InputState,
    pub supports_function_calling: bool,
    pub supports_streaming: bool,
    pub supports_thinking: bool,
    pub vision_ability: bool,
    pub focus: usize,
}

impl ModelForm {
    /// Indices of text-entry fields in the form (used by the render/keys).
    pub const TEXT_FIELD_COUNT: usize = 5;

    pub fn new(input: ModelInput, id: Option<i64>, providers: &[ProviderRow]) -> Self {
        let mk = |v: String| {
            let mut s = InputState::new();
            for ch in v.chars() {
                s.insert_char(ch);
            }
            s
        };
        let provider_index = providers
            .iter()
            .position(|p| p.id == input.provider_id)
            .unwrap_or(0);
        Self {
            id,
            model_name: mk(input.model_name),
            provider_index,
            context_length: mk(input.context_length.to_string()),
            max_output_tokens: mk(input.max_output_tokens.to_string()),
            input_token_price: mk(format!("{}", input.input_token_price)),
            output_token_price: mk(format!("{}", input.output_token_price)),
            supports_function_calling: input.supports_function_calling,
            supports_streaming: input.supports_streaming,
            supports_thinking: input.supports_thinking,
            vision_ability: input.vision_ability,
            focus: 0,
        }
    }

    pub fn text_fields_mut(&mut self) -> [&mut InputState; 5] {
        [
            &mut self.model_name,
            &mut self.context_length,
            &mut self.max_output_tokens,
            &mut self.input_token_price,
            &mut self.output_token_price,
        ]
    }

    pub fn text_fields(&self) -> [&InputState; 5] {
        [
            &self.model_name,
            &self.context_length,
            &self.max_output_tokens,
            &self.input_token_price,
            &self.output_token_price,
        ]
    }

    /// Validate and collect the form into a `ModelInput`.
    pub fn collect(&self, providers: &[ProviderRow]) -> Result<ModelInput, String> {
        let model_name = self.model_name.value().trim().to_string();
        if model_name.is_empty() {
            return Err("model name must not be empty".into());
        }
        let provider_id = providers
            .get(self.provider_index)
            .map(|p| p.id)
            .ok_or_else(|| "no provider selected".to_string())?;
        let parse_i = |s: &str, field: &str| -> Result<i64, String> {
            s.trim()
                .parse()
                .map_err(|_| format!("{field} must be an integer"))
        };
        let parse_f = |s: &str, field: &str| -> Result<f64, String> {
            s.trim()
                .parse()
                .map_err(|_| format!("{field} must be a number"))
        };
        Ok(ModelInput {
            model_name,
            provider_id,
            context_length: parse_i(self.context_length.value(), "context length")?,
            max_output_tokens: parse_i(self.max_output_tokens.value(), "max output tokens")?,
            input_token_price: parse_f(self.input_token_price.value(), "input price")?,
            output_token_price: parse_f(self.output_token_price.value(), "output price")?,
            vision_ability: self.vision_ability,
            supports_function_calling: self.supports_function_calling,
            supports_streaming: self.supports_streaming,
            supports_thinking: self.supports_thinking,
        })
    }
}

/// Mutable state for the Config tab.
pub struct ConfigTabState {
    pub providers: Vec<ProviderRow>,
    pub models: Vec<ModelRow>,
    pub pane: ConfigPane,
    pub selected_provider: usize,
    pub selected_model: usize,
    pub mode: ConfigMode,
    /// Transient status line message (e.g. "saved", "deleted", validation error).
    pub message: String,
}

impl Default for ConfigTabState {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            models: Vec::new(),
            pane: ConfigPane::Providers,
            selected_provider: 0,
            selected_model: 0,
            mode: ConfigMode::Browsing,
            message: String::new(),
        }
    }
}

impl ConfigTabState {
    pub fn selected_provider_row(&self) -> Option<&ProviderRow> {
        self.providers.get(self.selected_provider)
    }

    pub fn selected_model_row(&self) -> Option<&ModelRow> {
        self.models.get(self.selected_model)
    }

    pub fn move_selection(&mut self, delta: i32) {
        match self.pane {
            ConfigPane::Providers => {
                if self.providers.is_empty() {
                    return;
                }
                let len = self.providers.len() as i32;
                let mut i = self.selected_provider as i32 + delta;
                if i < 0 {
                    i = len - 1;
                } else if i >= len {
                    i = 0;
                }
                self.selected_provider = i as usize;
            }
            ConfigPane::Models => {
                if self.models.is_empty() {
                    return;
                }
                let len = self.models.len() as i32;
                let mut i = self.selected_model as i32 + delta;
                if i < 0 {
                    i = len - 1;
                } else if i >= len {
                    i = 0;
                }
                self.selected_model = i as usize;
            }
        }
    }
}

// ── AppState ───────────────────────────────────────────

/// State container for the TUI.
#[derive(Default)]
pub struct AppState {
    pub main_tab_state: MainTabState,
    pub agent_tab_state: AgentTabState,
    pub model_config_state: crate::widgets::model_config_widget::ModelConfigState,
    pub active_model: Arc<ArcSwapOption<Model>>,
}
