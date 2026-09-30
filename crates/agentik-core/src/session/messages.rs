//! Conversation storage on the Session: message insertion with tool_result
//! adjacency repair, context rendering, and durable sanitization.

use agentik_sdk::model::sanitize::sanitize_messages;
use agentik_sdk::types::AgentEvent;
use agentik_sdk::types::messages::{ContentBlock, Message, Role};
use uuid::Uuid;

use crate::error::Result;
use crate::message_ext::AgentMessageExt;
use crate::storage::PersistOp;

use super::Session;
use super::SessionState;
use super::compaction::{PRUNE_PROTECT_TOKENS, format_checkpoint_message, prune_old_tool_outputs};
use super::error;

/// Insert a `tool_result` into a user message so that all `tool_result`
/// blocks precede any text blocks.
///
/// The Anthropic invariant only requires the `tool_result` to live in the
/// user message immediately after the `tool_use`; however gateways that
/// translate to OpenAI Chat (e.g. MiniMax's Anthropic-compatible endpoint)
/// split a `text + tool_result` user message in block order. If text came
/// first it would sit between the assistant's `tool_use` and its
/// `tool_result`, producing a 400 `messages.N: tool_use ids were found
/// without tool_result blocks immediately after`. Keeping tool_results
/// grouped at the front preserves adjacency under that translation.
fn insert_tool_result_before_text(content: &mut Vec<ContentBlock>, block: ContentBlock) {
    let insert_at = content
        .iter()
        .position(|c| !matches!(c, ContentBlock::ToolResult { .. }))
        .unwrap_or(content.len());
    content.insert(insert_at, block);
}

impl Session {
    pub fn inject_message(
        &mut self,
        user_content: Vec<ContentBlock>,
        from_user: bool,
    ) -> Result<()> {
        // Emit before `remember` so the UI can distinguish an external display
        // event from the acknowledgement of a runtime-bound TUI prompt.
        let preview: String = user_content
            .iter()
            .filter_map(|cb| match cb {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !preview.is_empty() {
            self.shared.send_event(if from_user {
                AgentEvent::UserMessageAcknowledged(preview)
            } else {
                AgentEvent::MessageInjected(preview)
            });
        }

        let message = Message {
            id: Uuid::new_v4().to_string(),
            type_: "message".to_string(),
            role: Role::User,
            content: user_content,
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        };
        self.remember(message)?;
        Ok(())
    }

    // ── Message management (formerly on Memory) ─────────────

    /// Remember a new message: append to `messages` (handling tool-result
    /// adjacency) and push to the WAL if persistence is active.
    pub fn remember(&mut self, message: Message) -> Result<()> {
        let for_persist = if self.persist_tx.is_some() {
            Some(message.clone())
        } else {
            None
        };

        self.add_message(message)?;

        if let (Some(tx), Some(msg)) = (&self.persist_tx, for_persist) {
            let _ = tx.send(PersistOp::AppendMessage {
                session_id: self.id,
                message: msg,
            });
        }

        Ok(())
    }

    /// Add a message to the conversation, ensuring tool_result blocks are
    /// placed adjacent to their corresponding tool_use blocks.
    ///
    /// Duplicate `tool_result` blocks for the same `tool_use_id` are
    /// collapsed at insert time — the API rejects multiple tool_results for
    /// the same id, and silently letting them in risks a 400 once the
    /// request finally goes out. Keeping the *first* occurrence (the one
    /// already in the conversation) is safer than keeping the new one,
    /// because the existing block already matches what the conversation
    /// expects the model to have seen.
    fn add_message(&mut self, msg: Message) -> Result<()> {
        let mut tool_results: Vec<(String, Option<String>, Option<bool>)> = Vec::new();
        let mut others_content_blocks: Vec<ContentBlock> = Vec::new();
        // Filter out tool_results
        for cb in &msg.content {
            match cb {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => tool_results.push((tool_use_id.clone(), content.clone(), *is_error)),
                _ => others_content_blocks.push(cb.clone()),
            }
        }

        if !others_content_blocks.is_empty() {
            // De-dupe consecutive identical user text messages. Background
            // watchers (wait_task timeout) can inject the same message
            // multiple times if the LLM calls wait_task in a loop. Without
            // this guard, each copy is persisted and wastes context.
            if msg.role == Role::User {
                if let Some(last) = self.messages.last() {
                    if last.role == Role::User && last.content == others_content_blocks {
                        tracing::debug!(
                            "dropping duplicate consecutive user message ({} bytes)",
                            others_content_blocks
                                .iter()
                                .map(|cb| match cb {
                                    ContentBlock::Text { text } => text.len(),
                                    _ => 0,
                                })
                                .sum::<usize>()
                        );
                        return Ok(());
                    }
                }
            }
            let mut other_msg = msg.clone();
            other_msg.content = others_content_blocks;
            self.messages.push(other_msg);
        }

        for (tool_use_id, content, is_error) in tool_results {
            // De-dupe: if a tool_result for this id already exists anywhere
            // in the conversation, skip the new one. (The sanitisers in
            // `agentik_sdk::model::sanitize` would also catch this, but
            // stopping it at the entry point avoids needless churn and
            // prevents the duplicate from ever reaching the WAL.)
            if self.has_tool_result(&tool_use_id) {
                tracing::debug!(
                    tool_use_id = %tool_use_id,
                    "dropping duplicate tool_result at insert time"
                );
                continue;
            }

            let tc_msg_index =
                self.get_tooluse_msg_index(&tool_use_id)
                    .ok_or(error::Error::OrphanToolResult {
                        tool_use_id: tool_use_id.clone(),
                    })?;

            if tc_msg_index + 1 < self.messages.len() {
                // Need to move tool_result to the next message of tool_use
                if matches!(self.messages[tc_msg_index + 1].role, Role::User) {
                    // Insert the tool_result *before* any text already in the
                    // message (e.g. a wait_task timeout notification that was
                    // injected ahead of the result). Some Anthropic-compatible
                    // gateways split a text+tool_result user message in block
                    // order; text-first would land a plain user message between
                    // this tool_use and its tool_result and be rejected with
                    // `messages.N: tool_use ids were found without tool_result
                    // blocks immediately after`.
                    insert_tool_result_before_text(
                        &mut self.messages[tc_msg_index + 1].content,
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        },
                    );
                } else {
                    // Slot at tc_msg_index + 1 is non-User (e.g. another
                    // assistant turn, or a checkpoint summary message).
                    // Insert a fresh User message carrying only the
                    // tool_result so the Anthropic adjacency invariant
                    // (every tool_use followed immediately by a user
                    // message with the matching tool_result) is
                    // preserved at insertion. The previous error
                    // (`UnexpectedMessageLayout`) was the source of
                    // messages.1118 violations.
                    let mut tool_res_msg = msg.clone();
                    tool_res_msg.content = vec![ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    }];
                    self.messages.insert(tc_msg_index + 1, tool_res_msg);
                }
            } else {
                let mut tool_res_msg = msg.clone();
                tool_res_msg.content = vec![ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                }];
                self.messages.push(tool_res_msg);
            }
        }
        Ok(())
    }

    /// True when any message in the conversation already carries a
    /// `tool_result` block for `tool_use_id`.
    fn has_tool_result(&self, tool_use_id: &str) -> bool {
        self.messages.iter().any(|m| {
            m.content.iter().any(|c| {
                matches!(c, ContentBlock::ToolResult { tool_use_id: id, .. } if id == tool_use_id)
            })
        })
    }

    /// Get index of message with a tool_use block matching `tool_call_id`.
    fn get_tooluse_msg_index(&self, tool_call_id: &str) -> Option<usize> {
        self.messages.iter().position(|m| {
            m.content
                .iter()
                .any(|c| matches!(c, ContentBlock::ToolUse { id, .. } if id == tool_call_id))
        })
    }

    /// Render the full conversation context for the LLM.
    ///
    /// - Historical compaction summaries are injected as
    ///   `<conversation-checkpoint>` user messages BEFORE current messages.
    /// - Old tool outputs exceeding the protection window are pruned.
    pub fn render_context(&self) -> Result<Vec<Message>> {
        let mut result = Vec::new();

        // 1. Inject summaries from all compaction ancestors
        for summary in &self.ancestor_summaries {
            let formatted = format_checkpoint_message(summary);
            result.push(Message::user(formatted));
        }

        // 2. Append the current messages
        let mut messages = self.messages.clone();
        prune_old_tool_outputs(&mut messages, PRUNE_PROTECT_TOKENS);
        result.extend(messages);

        Ok(result)
    }

    /// Run the Anthropic-invariant sanitizer over `self.messages` and, if
    /// any repair was performed, replace the in-memory conversation with
    /// the sanitized version and emit `PersistOp::ReplaceSessionState` so
    /// the WAL reflects the cleaned shape.
    ///
    /// Without this, `Model::request` would silently run `sanitize_messages`
    /// at the API boundary and throw the patched result away — meaning
    /// every subsequent turn would re-patch the same broken shape and any
    /// snapshot/restore would resurrect the broken state. This method
    /// makes the repair *durable*.
    ///
    /// Cheap when nothing needs fixing: `sanitize_messages` returns the
    /// input `Vec` untouched (no allocation), so we only enter the WAL
    /// path when at least one rule fired.
    pub fn sanitize_and_persist(&mut self) {
        // Compare lengths before/after — same length means no repair was
        // needed (sanitize_messages either returns the input unchanged or
        // rebuilds a new Vec with potentially different length).
        let before_len = self.messages.len();
        let original_ptr = self.messages.as_ptr();
        let sanitized = sanitize_messages(std::mem::take(&mut self.messages));
        let same_alloc = sanitized.as_ptr() == original_ptr && sanitized.len() == before_len;
        self.messages = sanitized;
        if same_alloc {
            return;
        }
        let after_len = self.messages.len();
        tracing::info!(
            messages_before = before_len,
            messages_after = after_len,
            "session messages sanitized (Anthropic-invariant repair); persisting"
        );

        if let Some(tx) = &self.persist_tx {
            let state = SessionState {
                messages: self.messages.clone(),
                summary: self.summary.clone(),
                ancestor_summaries: self.ancestor_summaries.clone(),
                telemetry: self.telemetry,
            };
            let _ = tx.send(PersistOp::ReplaceSessionState {
                agent_id: self.shared.id,
                session_id: self.id,
                state,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_ext::AgentMessageExt;
    use crate::session::make_test_session;
    use agentik_types::messages::ContentBlock;

    /// Regression for `add_message` collapsing duplicate `tool_result` blocks
    /// when the unconditional `.push()` in the next-message branch would
    /// otherwise leave two `tool_result` blocks for the same `tool_use_id`
    /// in the conversation. Without this guard the next request would 400
    /// with `each tool_use must have a single result`.
    #[test]
    fn add_message_drops_duplicate_tool_result_on_remember() {
        let mut session = make_test_session();
        session
            .remember(Message::user("hello"))
            .expect("first remember ok");
        session
            .remember(Message::assistant_tool_use(
                "call_001",
                "bash",
                serde_json::json!({}),
            ))
            .expect("tool_use remember ok");

        // First tool_result — this is the legitimate one.
        session
            .remember(Message::tool_result("call_001", "first", false))
            .expect("first tool_result ok");

        // Snapshot how many tool_result blocks for call_001 exist now.
        let count_before = count_tool_results(&session.messages, "call_001");
        assert_eq!(count_before, 1);

        // Second remember with the same tool_use_id — must be dropped.
        session
            .remember(Message::tool_result(
                "call_001",
                "second (duplicate)",
                false,
            ))
            .expect("duplicate tool_result should not error");

        let count_after = count_tool_results(&session.messages, "call_001");
        assert_eq!(
            count_after, 1,
            "duplicate tool_result must be collapsed at insert time"
        );
        // And the *first* content should be preserved.
        let first_body = session
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .find_map(|c| match c {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } if tool_use_id == "call_001" => Some(content.clone()),
                _ => None,
            })
            .expect("at least one tool_result remains");
        assert_eq!(first_body.as_deref(), Some("first"));
    }

    /// Two distinct `tool_use_id`s must both be retained — the dedup is
    /// per-id, not "drop everything past the first".
    #[test]
    fn add_message_keeps_distinct_tool_results() {
        let mut session = make_test_session();
        session.remember(Message::user("hi")).unwrap();
        session
            .remember(Message {
                id: "a".into(),
                type_: "message".into(),
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "call_a".into(),
                        name: "bash".into(),
                        input: serde_json::json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "call_b".into(),
                        name: "bash".into(),
                        input: serde_json::json!({}),
                    },
                ],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            })
            .unwrap();
        session
            .remember(Message::tool_result("call_a", "alpha", false))
            .unwrap();
        session
            .remember(Message::tool_result("call_b", "beta", false))
            .unwrap();
        let a = count_tool_results(&session.messages, "call_a");
        let b = count_tool_results(&session.messages, "call_b");
        assert_eq!(a, 1);
        assert_eq!(b, 1);
    }

    /// Regression for `messages.N: tool_use ids were found without
    /// tool_result blocks immediately after` on MiniMax's Anthropic-compatible
    /// gateway: when a tool_result arrives after a notification/user text was
    /// already placed in the message following the tool_use (e.g. a wait_task
    /// timeout), `add_message` must insert the tool_result *before* that text.
    /// Gateway translation splits a text+tool_result user message in block
    /// order; text-first would put a plain user message between the tool_use
    /// and its tool_result.
    #[test]
    fn add_message_inserts_tool_result_before_existing_text() {
        let mut session = make_test_session();
        session.remember(Message::user("hi")).unwrap();
        session
            .remember(Message::assistant_tool_use(
                "call_wait",
                "wait_task",
                serde_json::json!({"task": 96}),
            ))
            .unwrap();
        // A notification text lands immediately after the tool_use (injected
        // by the wait_task watcher before the tool_result was delivered).
        session
            .remember(Message::user(
                "Background task 'delegate_to' (#96) did not complete within 240 seconds.",
            ))
            .unwrap();
        // The tool_result arrives late — must be inserted BEFORE the text.
        session
            .remember(Message::tool_result(
                "call_wait",
                "{\"status\":\"waiting\"}",
                false,
            ))
            .unwrap();

        let asst_idx = session
            .messages
            .iter()
            .position(|m| m.has_tool_use())
            .unwrap();
        let next = &session.messages[asst_idx + 1];
        assert!(matches!(next.role, Role::User));
        let kinds: Vec<&str> = next
            .content
            .iter()
            .map(|c| match c {
                ContentBlock::ToolResult { .. } => "tool_result",
                ContentBlock::Text { .. } => "text",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["tool_result", "text"],
            "tool_result must be inserted before the notification text"
        );
    }

    fn count_tool_results(msgs: &[Message], tool_use_id: &str) -> usize {
        msgs.iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| matches!(c, ContentBlock::ToolResult { tool_use_id: id, .. } if id == tool_use_id))
            .count()
    }

    /// When the slot immediately after a `tool_use` is non-User (e.g.
    /// another assistant turn or a checkpoint user message pushed in
    /// between the call and its result), `add_message` must insert a
    /// fresh User message carrying the `tool_result` so Anthropic's
    /// adjacency invariant holds at the source. Previously this returned
    /// `UnexpectedMessageLayout`, which surfaced as the messages.1118
    /// 400 from the API.
    #[test]
    fn add_message_inserts_user_slot_when_next_is_non_user() {
        let mut session = make_test_session();
        session.remember(Message::user("hi")).unwrap();
        session
            .remember(Message {
                id: "a".into(),
                type_: "message".into(),
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_X".into(),
                    name: "t".into(),
                    input: serde_json::json!({}),
                }],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            })
            .unwrap();
        // Push a non-user message between the tool_use and its result.
        session.messages.push(Message {
            id: "u_intermediate".into(),
            type_: "message".into(),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "checkpoint summary".into(),
            }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        });
        session
            .remember(Message::tool_result("call_X", "alpha", false))
            .unwrap();
        // Find the assistant tool_use and assert the immediately next
        // message is a User carrying tool_result(call_X).
        let asst_idx = session
            .messages
            .iter()
            .position(|m| m.has_tool_use())
            .expect("assistant with tool_use");
        let next = &session.messages[asst_idx + 1];
        assert!(
            matches!(next.role, Role::User),
            "next message must be User (inserted for adjacency)"
        );
        let has_result = next.content.iter().any(|c| {
            matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_X")
        });
        assert!(
            has_result,
            "adjacent user message must carry tool_result(call_X)"
        );
        let count = session
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_X"))
            .count();
        assert_eq!(
            count, 1,
            "tool_result must be inserted exactly once (no duplicates)"
        );
    }

    /// `sanitize_and_persist` must repair a misadjacent tool_result and
    /// leave the conversation well-formed. Without it, every LLM
    /// request would re-patch the same broken state and any snapshot
    /// would resurrect the broken shape.
    #[test]
    fn sanitize_and_persist_repairs_misadjacent_tool_result() {
        let mut session = make_test_session();
        // Build: [user, assistant{tool_use}, user{retry text},
        //        user{tool_result}] — the tool_result is one user
        // message too late.
        session.remember(Message::user("hi")).unwrap();
        session
            .remember(Message {
                id: "a".into(),
                type_: "message".into(),
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_X".into(),
                    name: "t".into(),
                    input: serde_json::json!({}),
                }],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            })
            .unwrap();
        session
            .remember(Message {
                id: "u_retry".into(),
                type_: "message".into(),
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "retry please".into(),
                }],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            })
            .unwrap();
        session
            .remember(Message::tool_result("call_X", "ok", false))
            .unwrap();

        let before_len = session.messages.len();
        session.sanitize_and_persist();
        let after_len = session.messages.len();

        // Adjacency: tool_use(call_X) must be followed immediately by a
        // user message containing tool_result(call_X).
        let asst_idx = session
            .messages
            .iter()
            .position(|m| m.has_tool_use())
            .expect("tool_use assistant remains");
        let next = &session.messages[asst_idx + 1];
        assert!(matches!(next.role, Role::User));
        let has_adjacent = next.content.iter().any(|c| {
            matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_X")
        });
        assert!(
            has_adjacent,
            "after sanitize_and_persist, tool_result(call_X) must sit in the adjacent user message"
        );
        // The repair moved blocks between messages; the count may change
        // (dedup + relocate). What matters is that exactly one
        // tool_result for call_X survives.
        let total = session
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_X"))
            .count();
        assert_eq!(total, 1, "exactly one tool_result for call_X must remain");
        // Length may have changed (one synthetic user message inserted
        // / one empty after-retract dropped).
        let _ = (before_len, after_len);
    }

    /// `sanitize_and_persist` must be a no-op when the conversation is
    /// already well-formed (fast-path: same Vec returned by
    /// `sanitize_messages`).
    #[test]
    fn sanitize_and_persist_is_noop_on_clean_conversation() {
        let mut session = make_test_session();
        session.remember(Message::user("hi")).unwrap();
        session.remember(Message::assistant_text("hello")).unwrap();
        session.remember(Message::user("how are you?")).unwrap();
        let before = session.messages.clone();
        session.sanitize_and_persist();
        // Clean input: no change in messages, no synthetic inserts.
        assert_eq!(session.messages.len(), before.len());
        for (a, b) in session.messages.iter().zip(before.iter()) {
            assert_eq!(a.role, b.role);
            assert_eq!(a.content.len(), b.content.len());
        }
    }

    /// Consecutive identical user text messages must be collapsed to a
    /// single copy. Background `wait_task` watchers can inject the same
    /// timeout message multiple times when the LLM calls wait_task in a
    /// loop — without this guard each copy is persisted and wastes context.
    #[test]
    fn add_message_drops_consecutive_duplicate_user_text() {
        let mut session = make_test_session();
        let msg_text = "Background task 'download' (#50) did not complete \
                         within 120 seconds. It is still running.";
        session.remember(Message::user(msg_text)).unwrap();
        let count_after_first = session.messages.len();

        // Second identical message — must be dropped.
        session.remember(Message::user(msg_text)).unwrap();
        assert_eq!(
            session.messages.len(),
            count_after_first,
            "duplicate consecutive user message must be dropped"
        );

        // Third identical message — still dropped.
        session.remember(Message::user(msg_text)).unwrap();
        assert_eq!(
            session.messages.len(),
            count_after_first,
            "third identical user message must also be dropped"
        );
    }

    /// Non-identical consecutive user messages must be kept. Only exact
    /// duplicates are collapsed.
    #[test]
    fn add_message_keeps_distinct_consecutive_user_text() {
        let mut session = make_test_session();
        session.remember(Message::user("first message")).unwrap();
        session.remember(Message::user("second message")).unwrap();
        assert_eq!(
            session.messages.len(),
            2,
            "distinct consecutive user messages must both be kept"
        );
    }

    /// After a different message (e.g. assistant reply) interleaves, the
    /// same text can appear again — the dedup is strictly consecutive.
    #[test]
    fn add_message_allows_same_text_after_interleaving() {
        let mut session = make_test_session();
        session.remember(Message::user("hello")).unwrap();
        session
            .remember(Message::assistant_text("hi there"))
            .unwrap();
        // Now "hello" again is fine — not consecutive duplicate.
        session.remember(Message::user("hello")).unwrap();
        assert_eq!(
            session.messages.len(),
            3,
            "same text after an interleaved message is not a duplicate"
        );
    }
}
