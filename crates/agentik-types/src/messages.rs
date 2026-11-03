use crate::path::AgentPath;
use crate::reasoning::{ReasoningConfig, ReasoningEffort, ThinkingConfig};
use crate::shared::{RequestId, Usage};
use crate::tools::{ToolChoice, ToolDefinition};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: String,
    #[serde(rename = "type", default = "default_type")]
    pub type_: String,
    pub role: Role,
    /// Who authored this message. The wire protocol only distinguishes
    /// user/assistant, so system-injected content (delegations, task
    /// notices, runtime feedback) rides in `Role::User` messages; `origin`
    /// records the actual source. Never serialized onto the model wire —
    /// requests are built from [`MessageParam`], which carries only
    /// role + content. Defaults to [`MessageOrigin::User`] so transcripts
    /// persisted before this field existed load unchanged.
    #[serde(default)]
    pub origin: MessageOrigin,
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub model: Option<String>,
    pub stop_reason: Option<StopReason>,
    pub stop_sequence: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(skip)]
    pub request_id: Option<RequestId>,
}

/// Maximum number of characters of text/thinking/tool content included in
/// [`Message::log_summary`] previews. Anything longer is truncated with `…`.
const LOG_PREVIEW_LEN: usize = 120;

/// Truncate `s` to at most `max` characters, appending `…` when truncated.
fn truncate_preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

impl Message {
    pub fn has_tool_use(&self) -> bool {
        self.content.iter().any(ContentBlock::is_tool_use)
    }

    pub fn has_tool_result(&self) -> bool {
        self.content.iter().any(ContentBlock::is_tool_result)
    }

    pub fn tool_uses(&self) -> Vec<&ContentBlock> {
        self.content.iter().filter(|c| c.is_tool_use()).collect()
    }

    pub fn tool_results(&self) -> Vec<&ContentBlock> {
        self.content.iter().filter(|c| c.is_tool_result()).collect()
    }

    /// Produce a compact, single-line summary of this message for logs.
    ///
    /// Format: `[role] block1 | block2 | ... [stop=<reason>] [tokens=in+out]`,
    /// where each block preview is produced by [`ContentBlock::log_summary`].
    /// Long text/JSON payloads are truncated to [`LOG_PREVIEW_LEN`] chars so
    /// the whole line fits comfortably on a single log line. This is intended
    /// for `tracing`/log output — not a faithful serialization.
    #[must_use]
    pub fn log_summary(&self) -> String {
        let role = match self.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let blocks: Vec<String> = self.content.iter().map(ContentBlock::log_summary).collect();
        let mut s = format!("[{role}] {}", blocks.join(" | "));
        if self.origin != MessageOrigin::default() {
            match self.origin.agent_path() {
                Some(path) => s.push_str(&format!(" origin=agent:{path}")),
                None => s.push_str(&format!(" origin={}", self.origin.tag())),
            }
        }
        if let Some(stop) = &self.stop_reason {
            s.push_str(&format!(" stop={stop:?}"));
        }
        if let Some(u) = &self.usage {
            s.push_str(&format!(" tokens={}+{}", u.input_tokens, u.output_tokens));
        }
        s
    }
}

fn default_type() -> String {
    "message".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// Provenance of a [`Message`] — who actually authored it.
///
/// The Anthropic-style wire only has user/assistant roles, so every
/// non-model contribution enters the conversation as `Role::User`.
/// `origin` keeps the distinction durable (persisted with the transcript)
/// without leaking onto the model wire.
///
/// Serialization: `User`/`System` serialize as bare strings (`"user"` /
/// `"system"`); `Agent` serializes as `{"agent": {"path": "…"}}`. The
/// deserializer also accepts the pre-`Agent` legacy string forms —
/// `"delegate"` / `"background"` degrade to [`MessageOrigin::System`]
/// because their authoring path was never recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrigin {
    /// A human typed it (gateway `DeliverMessage`, TUI prompt).
    #[default]
    User,
    /// Content authored by another agent, identified by its [`AgentPath`]
    /// (a `delegate_to` task delivery).
    Agent { path: AgentPath },
    /// Runtime feedback authored by the harness itself: interruption
    /// markers, retry notices, background-task notices, plan/state
    /// notices, compaction checkpoints.
    System,
}

impl MessageOrigin {
    /// [`MessageOrigin::Agent`] from a raw path string. Falls back to
    /// [`MessageOrigin::System`] when `path` is not a valid [`AgentPath`] —
    /// callers with pre-validated paths never hit that branch.
    #[must_use]
    pub fn agent(path: impl AsRef<str>) -> Self {
        AgentPath::try_from(path.as_ref())
            .map(|path| Self::Agent { path })
            .unwrap_or(Self::System)
    }

    /// Short lowercase kind tag for logs and UI badges.
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent { .. } => "agent",
            Self::System => "system",
        }
    }

    /// The authoring agent path, if any.
    #[must_use]
    pub fn agent_path(&self) -> Option<&AgentPath> {
        match self {
            Self::Agent { path } => Some(path),
            _ => None,
        }
    }
}

/// Legacy-compatible deserialization: bare strings (`"user"`, `"system"`,
/// and the pre-`Agent` `"delegate"` / `"background"` tags) alongside the
/// current externally-tagged map form. See [`MessageOrigin`].
impl<'de> Deserialize<'de> for MessageOrigin {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            /// `"user"` / `"system"`, or a legacy `"delegate"` /
            /// `"background"` tag.
            Tag(String),
            /// `{"agent": {"path": "…"}}`.
            Agent { agent: AgentFields },
        }
        #[derive(Deserialize)]
        struct AgentFields {
            path: AgentPath,
        }

        match Raw::deserialize(deserializer)? {
            Raw::Tag(tag) => match tag.as_str() {
                "user" => Ok(Self::User),
                // Legacy tags predate path attribution — degrade honestly.
                "delegate" | "background" => Ok(Self::System),
                "system" => Ok(Self::System),
                other => Err(serde::de::Error::unknown_field(
                    other,
                    &["user", "system", "agent"],
                )),
            },
            Raw::Agent { agent: AgentFields { path } } => Ok(Self::Agent { path }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: String,
    },

    #[serde(rename = "image")]
    Image { source: ImageSource },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: Option<String>,
        is_error: Option<bool>,
    },
}

impl ContentBlock {
    pub fn is_text(&self) -> bool {
        matches!(self, ContentBlock::Text { .. })
    }

    pub fn is_thinking(&self) -> bool {
        matches!(self, ContentBlock::Thinking { .. })
    }

    pub fn is_image(&self) -> bool {
        matches!(self, ContentBlock::Image { .. })
    }

    pub fn is_tool_use(&self) -> bool {
        matches!(self, ContentBlock::ToolUse { .. })
    }

    pub fn is_tool_result(&self) -> bool {
        matches!(self, ContentBlock::ToolResult { .. })
    }

    pub fn get_tool_call_id(&self) -> Option<String> {
        match self {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        }
    }

    /// Compact one-line preview of this block for logs (see
    /// [`Message::log_summary`]). Reports the block kind, a short payload
    /// preview, and identifying ids where applicable.
    #[must_use]
    pub fn log_summary(&self) -> String {
        match self {
            ContentBlock::Text { text } => {
                let n = text.chars().count();
                format!("text({n}): {}", truncate_preview(text, LOG_PREVIEW_LEN))
            }
            ContentBlock::Thinking { thinking, .. } => {
                let n = thinking.chars().count();
                format!(
                    "thinking({n}): {}",
                    truncate_preview(thinking, LOG_PREVIEW_LEN)
                )
            }
            ContentBlock::Image { source } => match source {
                ImageSource::Base64 { media_type, .. } => format!("image[{media_type}]"),
                ImageSource::Url { url } => format!("image<{url}>"),
            },
            ContentBlock::ToolUse { id, name, input } => {
                let payload = truncate_preview(&input.to_string(), LOG_PREVIEW_LEN);
                format!("tool_use {name}#{id} {payload}")
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let err = if is_error.unwrap_or(false) {
                    " ERROR"
                } else {
                    ""
                };
                let body = content.as_deref().unwrap_or("");
                format!(
                    "tool_result#{tool_use_id}{err}: {}",
                    truncate_preview(body, LOG_PREVIEW_LEN)
                )
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(tag = "type")]
pub enum ImageSource {
    #[serde(rename = "base64")]
    Base64 { media_type: String, data: String },

    #[serde(rename = "url")]
    Url { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageCreateParams {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<MessageParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<std::collections::HashMap<String, String>>,
    /// Anthropic-style extended-thinking configuration
    /// (`thinking: { type: "enabled", budget_tokens: N }`). Emitted verbatim
    /// on the Anthropic Messages wire; translated or dropped by other wire
    /// protocols. See [`crate::reasoning`] for cross-protocol guidance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    /// Protocol-neutral reasoning configuration
    /// (budget or effort). Emitted as `reasoning_effort`/`reasoning` on
    /// OpenAI wires; translated to a thinking budget on Anthropic wires when
    /// no `thinking` is also set. See [`crate::reasoning`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageParam {
    pub role: Role,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlockParam>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockParam {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: String,
    },

    #[serde(rename = "image")]
    Image { source: ImageSource },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: Option<String>,
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone)]
pub struct MessageCreateBuilder {
    params: MessageCreateParams,
}

impl MessageCreateBuilder {
    #[must_use]
    pub fn new(model: impl Into<String>, max_tokens: u32) -> Self {
        Self {
            params: MessageCreateParams {
                model: model.into(),
                max_tokens,
                messages: Vec::new(),
                system: None,
                temperature: None,
                top_p: None,
                top_k: None,
                stop_sequences: None,
                stream: None,
                tools: None,
                tool_choice: None,
                metadata: None,
                thinking: None,
                reasoning: None,
            },
        }
    }

    #[must_use]
    pub fn message(mut self, role: Role, content: impl Into<MessageContent>) -> Self {
        self.params.messages.push(MessageParam {
            role,
            content: content.into(),
        });
        self
    }

    #[must_use]
    pub fn user(self, content: impl Into<MessageContent>) -> Self {
        self.message(Role::User, content)
    }

    #[must_use]
    pub fn assistant(self, content: impl Into<MessageContent>) -> Self {
        self.message(Role::Assistant, content)
    }

    #[must_use]
    pub fn system(mut self, system: impl Into<String>) -> Self {
        self.params.system = Some(system.into());
        self
    }

    #[must_use]
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.params.temperature = Some(temperature);
        self
    }

    #[must_use]
    pub fn top_p(mut self, top_p: f32) -> Self {
        self.params.top_p = Some(top_p);
        self
    }

    #[must_use]
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.params.top_k = Some(top_k);
        self
    }

    #[must_use]
    pub fn stop_sequences(mut self, stop_sequences: Vec<String>) -> Self {
        self.params.stop_sequences = Some(stop_sequences);
        self
    }

    #[must_use]
    pub fn stream(mut self, stream: bool) -> Self {
        self.params.stream = Some(stream);
        self
    }

    #[must_use]
    pub fn tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.params.tools = Some(tools);
        self
    }

    #[must_use]
    pub fn tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.params.tool_choice = Some(tool_choice);
        self
    }

    #[must_use]
    pub fn metadata(mut self, metadata: std::collections::HashMap<String, String>) -> Self {
        self.params.metadata = Some(metadata);
        self
    }

    /// Configure extended thinking with an Anthropic-style token budget.
    ///
    /// On the Anthropic wire this emits `thinking: { type: "enabled",
    /// budget_tokens: N }`; other wires translate it as best they can.
    /// See [`crate::reasoning`] for cross-protocol semantics.
    #[must_use]
    pub fn thinking(mut self, thinking: ThinkingConfig) -> Self {
        self.params.thinking = Some(thinking);
        self
    }

    /// Configure reasoning with a protocol-neutral [`ReasoningConfig`].
    ///
    /// The active wire protocol picks the appropriate on-the-wire shape.
    #[must_use]
    pub fn reasoning(mut self, reasoning: ReasoningConfig) -> Self {
        self.params.reasoning = Some(reasoning);
        self
    }

    /// Convenience: set [`ReasoningConfig::Effort`] at the given level.
    ///
    /// Equivalent to `.reasoning(ReasoningConfig::from_effort(effort))`.
    #[must_use]
    pub fn reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.params.reasoning = Some(ReasoningConfig::from_effort(effort));
        self
    }

    #[must_use]
    pub fn build(self) -> MessageCreateParams {
        self.params
    }
}

impl From<String> for MessageContent {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for MessageContent {
    fn from(text: &str) -> Self {
        Self::Text(text.to_string())
    }
}

impl From<Vec<ContentBlockParam>> for MessageContent {
    fn from(blocks: Vec<ContentBlockParam>) -> Self {
        Self::Blocks(blocks)
    }
}

impl ContentBlockParam {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    #[must_use]
    pub fn image_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self::Image {
            source: ImageSource::Base64 {
                media_type: media_type.into(),
                data: data.into(),
            },
        }
    }

    #[must_use]
    pub fn image_url(url: impl Into<String>) -> Self {
        Self::Image {
            source: ImageSource::Url { url: url.into() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_builder() {
        let params = MessageCreateBuilder::new("claude-3-5-sonnet-latest", 1024)
            .user("Hello, Claude!")
            .system("You are a helpful assistant.")
            .temperature(0.7)
            .build();

        assert_eq!(params.model, "claude-3-5-sonnet-latest");
        assert_eq!(params.max_tokens, 1024);
        assert_eq!(params.messages.len(), 1);
        assert_eq!(params.messages[0].role, Role::User);
        assert_eq!(
            params.system,
            Some("You are a helpful assistant.".to_string())
        );
        assert_eq!(params.temperature, Some(0.7));
    }

    #[test]
    fn test_content_block_creation() {
        let text_block = ContentBlockParam::text("Hello world");
        match text_block {
            ContentBlockParam::Text { text } => assert_eq!(text, "Hello world"),
            _ => panic!("Expected text block"),
        }

        let image_block = ContentBlockParam::image_base64("image/jpeg", "base64data");
        match image_block {
            ContentBlockParam::Image { source } => match source {
                ImageSource::Base64 { media_type, data } => {
                    assert_eq!(media_type, "image/jpeg");
                    assert_eq!(data, "base64data");
                }
                _ => panic!("Expected base64 image source"),
            },
            _ => panic!("Expected image block"),
        }
    }

    #[test]
    fn test_message_content_from_string() {
        let content: MessageContent = "Hello".into();
        match content {
            MessageContent::Text(text) => assert_eq!(text, "Hello"),
            _ => panic!("Expected text content"),
        }
    }

    #[test]
    fn test_log_summary_text_message() {
        let msg = Message {
            id: "m1".into(),
            type_: "message".into(),
            role: Role::Assistant,
            origin: MessageOrigin::default(),
            content: vec![ContentBlock::Text {
                text: "Hello world".into(),
            }],
            model: None,
            stop_reason: Some(StopReason::EndTurn),
            stop_sequence: None,
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            }),
            request_id: None,
        };
        let s = msg.log_summary();
        assert!(s.starts_with("[assistant] text(11): Hello world"));
        assert!(s.contains("stop=EndTurn"));
        assert!(s.contains("tokens=10+5"));
    }

    #[test]
    fn test_log_summary_mixed_blocks() {
        let msg = Message {
            id: "m2".into(),
            type_: "message".into(),
            role: Role::User,
            origin: MessageOrigin::default(),
            content: vec![
                ContentBlock::ToolUse {
                    id: "tu_1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"cmd": "ls"}),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "tu_1".into(),
                    content: Some("file_a\nfile_b".into()),
                    is_error: Some(false),
                },
            ],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        };
        let s = msg.log_summary();
        assert!(s.starts_with("[user] tool_use bash#tu_1"));
        assert!(s.contains("tool_result#tu_1: file_a"));
        // should not append stop/tokens when absent
        assert!(!s.contains("stop="));
        assert!(!s.contains("tokens="));
    }

    #[test]
    fn test_log_summary_truncation() {
        let long = "x".repeat(LOG_PREVIEW_LEN + 50);
        let block = ContentBlock::Text { text: long.clone() };
        let s = block.log_summary();
        assert!(s.ends_with('…'));
        // char count preview = LOG_PREVIEW_LEN, plus the ellipsis
        let preview: String = s.split(": ").nth(1).unwrap().to_string();
        assert_eq!(preview.chars().count(), LOG_PREVIEW_LEN + 1);
    }

    #[test]
    fn test_log_summary_error_result() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "tu_9".into(),
            content: Some("boom".into()),
            is_error: Some(true),
        };
        assert!(block.log_summary().contains("ERROR"));
    }

    /// Transcripts persisted before `origin` existed must deserialize as
    /// plain user messages.
    #[test]
    fn legacy_message_without_origin_deserializes_as_user() {
        let json = r#"{
            "id": "m_old",
            "type": "message",
            "role": "user",
            "content": [{"type": "text", "text": "hello"}]
        }"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert_eq!(msg.origin, MessageOrigin::User);
        assert_eq!(msg.role, Role::User);
    }

    /// `origin` survives a JSON round trip in the current format and never
    /// reaches wire params.
    #[test]
    fn origin_round_trips_and_never_reaches_wire_params() {
        let cases = [
            (
                MessageOrigin::User,
                serde_json::json!("user"),
            ),
            (
                MessageOrigin::System,
                serde_json::json!("system"),
            ),
            (
                MessageOrigin::agent("/root/researcher"),
                serde_json::json!({"agent": {"path": "/root/researcher"}}),
            ),
        ];
        for (origin, expected) in cases {
            let msg = Message {
                id: "m1".into(),
                type_: "message".into(),
                role: Role::User,
                origin: origin.clone(),
                content: vec![ContentBlock::Text { text: "hi".into() }],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            };
            let json = serde_json::to_value(&msg).unwrap();
            assert_eq!(json["origin"], expected);
            let back: Message = serde_json::from_value(json).unwrap();
            assert_eq!(back.origin, origin);

            // Wire params carry only role + content — no origin leakage.
            let param = MessageParam {
                role: msg.role.clone(),
                content: MessageContent::Text("hi".into()),
            };
            let param_json = serde_json::to_value(&param).unwrap();
            assert!(param_json.get("origin").is_none());
        }
    }

    /// Transcript rows persisted between the origin field's introduction
    /// (string tags) and the `Agent { path }` upgrade must still load.
    /// `"delegate"` / `"background"` degrade to [`MessageOrigin::System`]
    /// — their authoring path was never recorded.
    #[test]
    fn legacy_string_origin_tags_deserialize() {
        assert_eq!(
            serde_json::from_value::<MessageOrigin>(serde_json::json!("user")).unwrap(),
            MessageOrigin::User
        );
        assert_eq!(
            serde_json::from_value::<MessageOrigin>(serde_json::json!("system")).unwrap(),
            MessageOrigin::System
        );
        assert_eq!(
            serde_json::from_value::<MessageOrigin>(serde_json::json!("delegate")).unwrap(),
            MessageOrigin::System
        );
        assert_eq!(
            serde_json::from_value::<MessageOrigin>(serde_json::json!("background")).unwrap(),
            MessageOrigin::System
        );
        assert!(
            serde_json::from_value::<MessageOrigin>(serde_json::json!("nonsense")).is_err(),
            "unknown tags must be rejected"
        );
    }

    /// Invalid paths fall back to System; valid ones keep the typed path.
    #[test]
    fn agent_constructor_validates_path() {
        assert_eq!(
            MessageOrigin::agent("/root/researcher"),
            MessageOrigin::Agent {
                path: "/root/researcher".parse().unwrap(),
            }
        );
        assert_eq!(MessageOrigin::agent("not a path"), MessageOrigin::System);
    }

    /// Non-default origins show up in log summaries.
    #[test]
    fn log_summary_reports_non_default_origin() {
        let base = Message {
            id: "m1".into(),
            type_: "message".into(),
            role: Role::User,
            origin: MessageOrigin::agent("/root/researcher"),
            content: vec![ContentBlock::Text {
                text: "task".into(),
            }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        };
        assert!(base.log_summary().contains("origin=agent:/root/researcher"));

        let system = Message {
            origin: MessageOrigin::System,
            ..base.clone()
        };
        assert!(system.log_summary().contains("origin=system"));

        let plain = Message {
            origin: MessageOrigin::User,
            ..base
        };
        assert!(!plain.log_summary().contains("origin="));
    }
}
