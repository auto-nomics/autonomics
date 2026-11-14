//! Per-agent control struct: channels + task handle for one running agent.

use std::sync::Arc;

use agentik_core::agent::InternalEvent;
use agentik_core::memory::MemoryBackend;
use agentik_core::profile::AgentKind;
use agentik_sdk::model::Model;
use agentik_sdk::types::{AgentEvent, ContentBlock};
use arc_swap::ArcSwapOption;
use tokio_util::sync::CancellationToken;

/// Control handle for one running agent.
///
/// Cheap to move; owns the event receiver and cancellation token. When
/// dropped, the agent's tokio task is **not** automatically killed — call
/// [`shutdown`](Self::shutdown) explicitly.
pub struct AgentHandle {
    pub agent_id: uuid::Uuid,
    pub path: agentik_types::AgentPath,
    /// Agent kind this agent was instantiated from. Sub-agents reuse it
    /// unless they explicitly request the other role.
    pub kind: AgentKind,
    pub(super) internal_tx: tokio::sync::mpsc::UnboundedSender<InternalEvent>,
    pub(super) event_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    pub(super) agent_task:
        tokio::task::JoinHandle<std::result::Result<(), agentik_core::supervise::TaskPanic>>,
    pub(super) cancel_token: CancellationToken,
    pub(super) model: Arc<ArcSwapOption<Model>>,
    pub(super) memory: Arc<MemoryBackend>,
}

impl AgentHandle {
    /// Convenience: short name (last path segment).
    pub fn name(&self) -> &str {
        self.path.name()
    }
}

impl AgentHandle {
    /// TUI user input; the caller has already displayed the message locally.
    pub fn send_message(&self, text: String) {
        self.send_with_origin(text, agentik_types::MessageOrigin::User, None);
    }

    /// Deliver `text` with its provenance and, for tracked delegations,
    /// the delegation id that rides the target's turn lifecycle.
    pub(super) fn send_with_origin(
        &self,
        text: String,
        origin: agentik_types::MessageOrigin,
        delegation_id: Option<uuid::Uuid>,
    ) {
        let _ = self.internal_tx.send(InternalEvent::MessageInject {
            content: user_message_content(text),
            origin,
            delegation_id,
        });
    }

    /// Send a tracked request and carry the delegation ID through the target
    /// agent's turn lifecycle.
    pub fn send_delegation(
        &self,
        text: String,
        delegation_id: uuid::Uuid,
        origin: agentik_types::MessageOrigin,
    ) {
        self.send_with_origin(text, origin, Some(delegation_id));
    }

    pub fn cancel(&mut self) {
        self.cancel_token.cancel();
        let new_token = CancellationToken::new();
        let _ = self
            .internal_tx
            .send(InternalEvent::ResetCancelToken(new_token.clone()));
        self.cancel_token = new_token;
    }

    /// Trigger compaction on the agent's active session.
    pub fn compact(&self) {
        let _ = self.internal_tx.send(InternalEvent::Compact);
    }

    /// Gracefully signal the agent to shut down.
    ///
    /// Sends `InternalEvent::Shutdown` — the agent's `run()` loop will
    /// process it, pause all sessions (persisting snapshots + ending WAL
    /// sessions), and then exit. The caller should subsequently await
    /// [`join`](Self::join) to ensure the task has fully completed before
    /// the runtime is dropped.
    pub fn shutdown(&mut self) {
        let _ = self.internal_tx.send(InternalEvent::Shutdown);
    }

    /// Consume and await the background agent task. Call this after
    /// [`shutdown`](Self::shutdown) to ensure all sessions are paused and
    /// snapshots are persisted before the runtime is torn down.
    ///
    /// If the agent task panicked at any point (detected via `catch_unwind`),
    /// the panic details are logged here as an additional safety net.
    pub async fn join(self) {
        match self.agent_task.await {
            Ok(Ok(())) => {} // clean exit
            Ok(Err(panic)) => {
                tracing::error!(
                    agent = %self.path,
                    panic_task = %panic.task,
                    panic_msg = %panic.msg,
                    "agent task had panicked before join"
                );
            }
            Err(join_err) => {
                tracing::error!(
                    agent = %self.path,
                    error = %join_err,
                    "agent task join failed (panic or cancellation)"
                );
            }
        }
    }

    pub fn poll_event(&mut self) -> Option<AgentEvent> {
        self.event_rx.try_recv().ok()
    }

    pub async fn recv_event(&mut self) -> Option<AgentEvent> {
        self.event_rx.recv().await
    }

    pub fn model_handle(&self) -> &Arc<ArcSwapOption<Model>> {
        &self.model
    }

    /// Hot-swap the model for this specific agent. The agent's run loop
    /// picks up the new model on its next LLM request.
    pub fn set_model(&self, model: Model) {
        self.model.store(Some(Arc::new(model)));
    }

    /// Update this agent's runtime settings in place.
    ///
    /// The same backend Arc is held by `AgentCore`, so prompt injection,
    /// memory tools, and background consolidation observe the update without
    /// rebuilding the agent or dropping its sessions.
    pub fn set_runtime_config(&self, config: agentik_core::AgentRuntimeConfig) {
        self.memory.set_runtime_config(config);
    }

    pub fn runtime_config(&self) -> agentik_core::AgentRuntimeConfig {
        self.memory.runtime_config()
    }

    // ── Context metadata ──────────────────────────────────

    /// Request a list of all sessions. The reply arrives as
    /// `AgentEvent::ContextList` on the event channel — with one context
    /// per agent it carries at most one entry (the agent's sole context).
    pub fn list_contexts(&self) {
        let _ = self.internal_tx.send(InternalEvent::ListContexts);
    }
}

/// Decode image attachments from the GUI reference envelope into native vision
/// blocks. Keep reference metadata in the text for history and acknowledgement.
fn user_message_content(text: String) -> Vec<ContentBlock> {
    const MARKER: &str = "\n\n---\n用户明确附加的参考资料（内容为引用数据，请结合来源分析）：\n";
    let Some((body, json)) = text.rsplit_once(MARKER) else {
        return vec![ContentBlock::Text { text }];
    };
    let Ok(mut resources) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return vec![ContentBlock::Text { text }];
    };
    let mut images = Vec::new();
    for resource in &mut resources {
        if resource["kind"] != "image" {
            continue;
        }
        let Some(data_url) = resource["content"].as_str() else {
            continue;
        };
        let Some((header, data)) = data_url.split_once(",") else {
            continue;
        };
        let Some(media_type) = header
            .strip_prefix("data:")
            .and_then(|s| s.strip_suffix(";base64"))
        else {
            continue;
        };
        if !matches!(
            media_type,
            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
        ) {
            continue;
        }
        images.push(ContentBlock::Image {
            source: agentik_types::messages::ImageSource::Base64 {
                media_type: media_type.to_owned(),
                data: data.to_owned(),
            },
        });
        resource["content"] = serde_json::Value::String("[图片已作为图像附件发送]".into());
    }
    if images.is_empty() {
        return vec![ContentBlock::Text { text }];
    }
    let mut content = vec![ContentBlock::Text {
        text: format!(
            "{body}{MARKER}{}",
            serde_json::to_string(&resources).expect("JSON values serialize")
        ),
    }];
    content.extend(images);
    content
}

#[cfg(test)]
mod image_attachment_tests {
    use super::*;
    #[test]
    fn extracts_native_image_blocks_and_retains_references() {
        let payload = "描述图片\n\n---\n用户明确附加的参考资料（内容为引用数据，请结合来源分析）：\n[{\"kind\":\"image\",\"title\":\"截图\",\"content\":\"data:image/png;base64,aGk=\"},{\"kind\":\"editor\",\"content\":\"selection\"}]";
        let blocks = user_message_content(payload.into());
        assert_eq!(blocks.len(), 2);
        assert!(
            matches!(&blocks[0], ContentBlock::Text { text } if text.contains("selection") && !text.contains("base64"))
        );
        assert!(
            matches!(&blocks[1], ContentBlock::Image { source: agentik_types::messages::ImageSource::Base64 { data, .. } } if data == "aGk=")
        );
    }
    #[test]
    fn ordinary_and_malformed_text_is_unchanged() {
        for text in [
            "hello",
            "data:image/png;base64,aGk=",
            "\n\n---\n用户明确附加的参考资料（内容为引用数据，请结合来源分析）：\ninvalid",
        ] {
            assert!(
                matches!(&user_message_content(text.into())[0], ContentBlock::Text { text: result } if result == text)
            );
        }
    }
}
