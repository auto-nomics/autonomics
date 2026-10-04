//! Re-attaching images returned by tools.
//!
//! Tool results are remembered as text-only `ContentBlock::ToolResult`
//! messages, which drops any image blocks the tool produced. For vision
//! models (or when `AGENTIK_TOOL_IMAGES=always`) the images are re-attached
//! as companion user messages placed right after the tool result — the one
//! place every wire protocol accepts images.

use std::sync::OnceLock;

use agentik_sdk::types::messages::{ContentBlock, ImageSource, Message};
use agentik_sdk::types::tools::{
    ImageSource as ToolImageSource, ToolResult, ToolResultBlock, ToolResultContent, ToolUse,
};

use crate::error::Result;
use crate::message_ext::AgentMessageExt;

use super::Session;

/// Maximum images carried into one companion message per tool result.
const TOOL_IMAGE_MAX_COUNT: usize = 4;
/// Maximum base64 characters per image in a companion message. Larger
/// images are dropped with a warning — a truncated base64 payload would
/// be invalid input for every provider, so skipping is the only safe cut.
const TOOL_IMAGE_MAX_BASE64_CHARS: usize = 6 * 1024 * 1024;

/// Parsed form of the `AGENTIK_TOOL_IMAGES` env var.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolImagesMode {
    /// Enabled iff the active model reports vision support (default).
    Auto,
    /// Enabled regardless of vision support.
    Always,
    /// Disabled entirely.
    Never,
}

fn parse_tool_images_mode(raw: &str) -> ToolImagesMode {
    match raw.trim().to_ascii_lowercase().as_str() {
        "always" => ToolImagesMode::Always,
        "never" => ToolImagesMode::Never,
        _ => ToolImagesMode::Auto,
    }
}

/// Whether tool-returned images should be delivered to the model.
///
/// Controlled by `AGENTIK_TOOL_IMAGES` (`auto` | `always` | `never`,
/// default `auto`): in `auto` mode images are attached only when the
/// active model reports `vision_ability`, so text-only models never
/// receive image blocks they would reject.
fn tool_images_enabled(vision: bool) -> bool {
    static MODE: OnceLock<ToolImagesMode> = OnceLock::new();
    let mode = *MODE.get_or_init(|| {
        parse_tool_images_mode(&std::env::var("AGENTIK_TOOL_IMAGES").unwrap_or_default())
    });
    match mode {
        ToolImagesMode::Always => true,
        ToolImagesMode::Never => false,
        ToolImagesMode::Auto => vision,
    }
}

/// Build the companion user message carrying a tool result's image blocks.
///
/// Tool results are remembered as `ContentBlock::ToolResult`, whose content
/// is plain text — image blocks returned by tools such as `bib_read_figure`
/// are silently dropped by `text_content()`. This companion re-attaches them
/// where every wire protocol accepts images: a user message right after the
/// tool result. Returns `None` when the result carries no usable images.
fn tool_result_image_companion(tr: &ToolResult, tool_name: &str) -> Option<Message> {
    let ToolResultContent::Blocks(blocks) = &tr.content else {
        return None;
    };
    let mut images = Vec::new();
    let mut skipped = 0usize;
    for block in blocks {
        let ToolResultBlock::Image { source } = block else {
            continue;
        };
        let ToolImageSource::Base64 { media_type, data } = source;
        if data.len() > TOOL_IMAGE_MAX_BASE64_CHARS {
            skipped += 1;
            tracing::warn!(
                tool = tool_name,
                media_type = media_type.as_str(),
                chars = data.len(),
                "skipping oversize tool-result image"
            );
            continue;
        }
        if images.len() >= TOOL_IMAGE_MAX_COUNT {
            skipped += 1;
            tracing::warn!(
                tool = tool_name,
                limit = TOOL_IMAGE_MAX_COUNT,
                "skipping tool-result image beyond per-call limit"
            );
            continue;
        }
        images.push(ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: media_type.clone(),
                data: data.clone(),
            },
        });
    }
    if images.is_empty() {
        return None;
    }
    let short_id: String = tr.tool_use_id.chars().take(8).collect();
    let mut note = format!(
        "[tool-result-images] {tool_name} (call {short_id}…) returned {} image(s), attached \
         below. They are the figures the tool produced; analyze them directly.",
        images.len()
    );
    if skipped > 0 {
        note.push_str(&format!(
            " {skipped} image(s) were skipped (per-call limit {TOOL_IMAGE_MAX_COUNT} or over \
             the 6 MiB size cap)."
        ));
    }
    let mut content = vec![ContentBlock::Text { text: note }];
    content.append(&mut images);
    Some(Message::user_blocks(content))
}

impl Session {
    /// Core agent workflow: build context → request API → execute tools → remember.
    /// Re-attach images returned by tools as companion user messages.
    ///
    /// The `Message::tool_result` values remembered by the workflow flatten
    /// their content to text, so image blocks would never reach the model.
    /// No-op for text-only results and — in the default `auto` mode of
    /// `AGENTIK_TOOL_IMAGES` — for models without vision support.
    pub(super) fn remember_tool_result_images(
        &mut self,
        tool_results: &[ToolResult],
        toolcalls: &[ToolUse],
    ) -> Result<()> {
        let vision = self
            .shared
            .model
            .load()
            .as_ref()
            .is_some_and(|m| m.model_info.vision_ability);
        if !tool_images_enabled(vision) {
            return Ok(());
        }
        for tr in tool_results {
            let Some(tool_name) = toolcalls
                .iter()
                .find(|tc| tc.id == tr.tool_use_id)
                .map(|tc| tc.name.as_str())
            else {
                continue;
            };
            let Some(companion) = tool_result_image_companion(tr, tool_name) else {
                continue;
            };
            self.remember(companion.clone())?;
            self.token_budget.add_pending_message(&companion);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentConfig;
    use crate::message_ext::AgentMessageExt;
    use crate::session::{AgentShared, make_test_session};
    use crate::testing::dummy_model_info;
    use crate::tools::error::ToolError;
    use crate::tools::function::{ToolContext, ToolFunction};
    use crate::tools::toolset::ToolRegistration;
    use agentik_proc::tool;
    use agentik_sdk::model::Model;
    use agentik_sdk::model::sanitize::sanitize_messages;
    use agentik_sdk::provider::client::MockApiClient;
    use agentik_sdk::streaming::MessageStream;
    use agentik_sdk::types::messages::Role;
    use agentik_types::tools::ToolResultContent;
    use std::sync::Arc;
    use uuid::Uuid;

    fn figure_result(id: &str) -> ToolResult {
        ToolResult {
            tool_use_id: id.to_string(),
            content: ToolResultContent::Blocks(vec![
                agentik_types::tools::ToolResultBlock::text("figure summary"),
                agentik_types::tools::ToolResultBlock::image_base64(
                    "image/png",
                    "aGk=", // "hi"
                ),
            ]),
            is_error: None,
        }
    }

    #[test]
    fn companion_is_user_text_then_images() {
        let companion =
            tool_result_image_companion(&figure_result("call_fig_12345678"), "bib_read_figure")
                .expect("blocks result with an image yields a companion");
        assert!(matches!(companion.role, Role::User));
        assert_eq!(companion.content.len(), 2);
        assert!(matches!(&companion.content[0], ContentBlock::Text { text }
                if text.contains("[tool-result-images]")
                    && text.contains("bib_read_figure")
                    && text.contains("(call call_fig…)")));
        assert!(matches!(&companion.content[1], ContentBlock::Image {
                source: agentik_types::messages::ImageSource::Base64 { media_type, data }
            } if media_type == "image/png" && data == "aGk="));
    }

    #[test]
    fn companion_is_none_without_images() {
        let text_only = ToolResult {
            tool_use_id: "call_t1".into(),
            content: ToolResultContent::Text("plain".into()),
            is_error: None,
        };
        assert!(tool_result_image_companion(&text_only, "t").is_none());

        let empty_blocks = ToolResult {
            tool_use_id: "call_t2".into(),
            content: ToolResultContent::Blocks(vec![agentik_types::tools::ToolResultBlock::text(
                "only text",
            )]),
            is_error: None,
        };
        assert!(tool_result_image_companion(&empty_blocks, "t").is_none());
    }

    #[test]
    fn companion_caps_image_count_and_notes_skips() {
        let blocks: Vec<_> = (0..6)
            .map(|i| {
                agentik_types::tools::ToolResultBlock::image_base64("image/png", "x".repeat(10 + i))
            })
            .collect();
        let result = ToolResult {
            tool_use_id: "call_many".into(),
            content: ToolResultContent::Blocks(blocks),
            is_error: None,
        };
        let companion = tool_result_image_companion(&result, "net_image").unwrap();
        let images = companion.content.iter().filter(|c| c.is_image()).count();
        assert_eq!(images, TOOL_IMAGE_MAX_COUNT);
        let note = companion.text().join("");
        assert!(note.contains("2 image(s) were skipped"), "note was: {note}");
    }

    #[test]
    fn companion_drops_oversize_images() {
        let oversize = ToolResult {
            tool_use_id: "call_big".into(),
            content: ToolResultContent::Blocks(vec![
                agentik_types::tools::ToolResultBlock::image_base64(
                    "image/png",
                    "x".repeat(TOOL_IMAGE_MAX_BASE64_CHARS + 1),
                ),
            ]),
            is_error: None,
        };
        assert!(tool_result_image_companion(&oversize, "t").is_none());
    }

    #[test]
    fn mode_parsing_covers_aliases() {
        assert_eq!(parse_tool_images_mode(""), ToolImagesMode::Auto);
        assert_eq!(parse_tool_images_mode("auto"), ToolImagesMode::Auto);
        assert_eq!(parse_tool_images_mode("junk"), ToolImagesMode::Auto);
        assert_eq!(parse_tool_images_mode(" ALWAYS "), ToolImagesMode::Always);
        assert_eq!(parse_tool_images_mode("Never"), ToolImagesMode::Never);
    }

    /// The real remember flow produces a conversation that sanitize
    /// renders as one canonical user message: `[ToolResult, Text,
    /// Image]` — tool result first, then the companion marker and the
    /// image itself.
    #[test]
    fn companion_survives_sanitize_as_canonical_layout() {
        let mut session = make_test_session();
        session.remember(Message::user("read figure 1")).unwrap();
        session
            .remember(Message::assistant_tool_use(
                "call_fig_1",
                "bib_read_figure",
                serde_json::json!({}),
            ))
            .unwrap();
        session
            .remember(Message::tool_result("call_fig_1", "figure summary", false))
            .unwrap();
        let companion = tool_result_image_companion(
            &ToolResult {
                tool_use_id: "call_fig_1".into(),
                content: ToolResultContent::Blocks(vec![
                    agentik_types::tools::ToolResultBlock::text("figure summary"),
                    agentik_types::tools::ToolResultBlock::image_base64("image/png", "aGk="),
                ]),
                is_error: None,
            },
            "bib_read_figure",
        )
        .unwrap();
        session.remember(companion).unwrap();

        // The conversation itself must now contain the image block
        // somewhere (merged or standalone — that is sanitize's call).
        assert!(
            session
                .messages
                .iter()
                .flat_map(|m| m.content.iter())
                .any(|c| c.is_image()),
            "companion image must be remembered"
        );

        let sanitized = sanitize_messages(session.messages.clone());
        let carrier = sanitized
            .iter()
            .find(|m| {
                m.content.iter().any(|c| {
                    matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_fig_1")
                })
            })
            .expect("tool_result message survives sanitize");
        let kinds: Vec<&str> = carrier
            .content
            .iter()
            .map(|c| match c {
                ContentBlock::ToolResult { .. } => "tool_result",
                ContentBlock::Text { .. } => "text",
                ContentBlock::Image { .. } => "image",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["tool_result", "text", "image"]);
        // Exactly one tool_result block for the call across the whole
        // sanitized conversation.
        let total_tool_results = sanitized
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| {
                matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_fig_1")
            })
            .count();
        assert_eq!(total_tool_results, 1);
    }

    // ── Workflow integration (mock-backed) ──────────────────────

    #[tool(name = "return_figure", description = "Returns one figure image")]
    struct ReturnFigureInput {}

    struct ReturnFigureTool;

    #[async_trait::async_trait]
    impl ToolFunction for ReturnFigureTool {
        type Input = ReturnFigureInput;

        async fn run(
            &self,
            _input: ReturnFigureInput,
        ) -> std::result::Result<ToolResult, ToolError> {
            Ok(ToolResult::with_blocks(vec![
                agentik_types::tools::ToolResultBlock::text("figure summary"),
                agentik_types::tools::ToolResultBlock::image_base64("image/png", "aGk="),
            ]))
        }
    }

    /// `AgentShared::new_for_tests` ships an empty model slot and an
    /// empty (frozen) registry, so the workflow tests build their own
    /// with a mock-backed model and the blocks-returning tool above.
    fn make_shared_with(model: Model) -> Arc<AgentShared> {
        let mut registry = crate::tools::ToolRegistry::new();
        registry
            .register(ToolRegistration::from(ReturnFigureTool))
            .unwrap();
        Arc::new(AgentShared {
            id: Uuid::new_v4(),
            path: agentik_types::AgentPath::root(),
            config_json: serde_json::json!({}),
            model: Arc::new(arc_swap::ArcSwapOption::from_pointee(Some(model))),
            config: AgentConfig::default(),
            storage: None,
            context_provider: None,
            system_prompt_section: None,
            system_prompt_identity: None,
            memory: None,
            tool_registry: Arc::new(registry),
            tasks: Arc::new(tokio::sync::RwLock::new(
                crate::tools::task_runtime::TaskStore::new(),
            )),
            event_tx: arc_swap::ArcSwapOption::empty(),
            persist_tx: std::sync::OnceLock::new(),
            plan: Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                agentik_types::AgentPlan::new(),
            ))),
        })
    }

    async fn run_workflow_with_model(vision: bool) -> Vec<Vec<Message>> {
        // The mode is cached in a process-wide OnceLock; an ambient
        // override would make the assertions below meaningless.
        if std::env::var_os("AGENTIK_TOOL_IMAGES").is_some() {
            panic!("unset AGENTIK_TOOL_IMAGES before running this test");
        }

        let captured: Arc<std::sync::Mutex<Vec<Vec<Message>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut mock = MockApiClient::new();

        let cap_first = Arc::clone(&captured);
        mock.expect_request_stream_with_system().times(1).returning(
            move |messages, _tools, _info, _system| {
                cap_first.lock().unwrap().push(messages);
                Ok(MessageStream::from_events(
                    Vec::new(),
                    Message::assistant_tool_use(
                        "call_fig_1",
                        "return_figure",
                        serde_json::json!({}),
                    ),
                ))
            },
        );
        let cap_second = Arc::clone(&captured);
        mock.expect_request_stream_with_system().times(1).returning(
            move |messages, _tools, _info, _system| {
                cap_second.lock().unwrap().push(messages);
                Ok(MessageStream::from_events(
                    Vec::new(),
                    Message::assistant_text("figure described"),
                ))
            },
        );

        let mut info = dummy_model_info("img-test");
        info.vision_ability = vision;
        let model = Model::with_client(info, mock);
        let shared = make_shared_with(model);

        let (internal_tx, _internal_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut session = Session::new_for_tests(shared, agentik_types::AgentPath::root());
        session.remember(Message::user("read the figure")).unwrap();
        // Two iterations = the outer run_session loop: first produces
        // the tool call, second requests again with the results (this
        // is where the companion must be present or absent).
        session.agent_workflow(&internal_tx, None).await.unwrap();
        session.agent_workflow(&internal_tx, None).await.unwrap();

        let mut guard = captured.lock().unwrap();
        std::mem::take(&mut *guard)
    }

    #[tokio::test]
    async fn workflow_sends_tool_images_to_vision_model() {
        let requests = run_workflow_with_model(true).await;
        assert_eq!(requests.len(), 2, "one request per workflow iteration");
        let second = &requests[1];
        assert!(
            second
                .iter()
                .flat_map(|m| m.content.iter())
                .any(|c| matches!(c, ContentBlock::Image { .. })),
            "second request must carry the companion image for a vision model"
        );
        assert!(
            second
                .iter()
                .any(|m| m.text().iter().any(|t| t.contains("[tool-result-images]"))),
            "second request must carry the companion marker text"
        );
    }

    #[tokio::test]
    async fn workflow_omits_tool_images_for_text_model() {
        let requests = run_workflow_with_model(false).await;
        assert_eq!(requests.len(), 2);
        let second = &requests[1];
        assert!(
            !second
                .iter()
                .flat_map(|m| m.content.iter())
                .any(|c| matches!(c, ContentBlock::Image { .. })),
            "text-only model must not receive image blocks"
        );
        // The text-flattened tool result itself is still there.
        assert!(
            second.iter().flat_map(|m| m.content.iter()).any(|c| {
                matches!(c, ContentBlock::ToolResult { content, .. }
                    if content.as_deref().is_some_and(|t| t.contains("figure summary")))
            }),
            "flattened tool-result text must still be delivered"
        );
    }
}
