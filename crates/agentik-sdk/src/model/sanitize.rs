//! Message sanitisation before sending to Anthropic-compatible APIs.
//!
//! Anthropic requires every `tool_use` block to have a corresponding
//! `tool_result` block in the *immediately following* message. If the agent
//! loop is interrupted (user cancel, crash, etc.) between recording a
//! `tool_use` and its `tool_result`, the orphaned `tool_use` causes a 400
//! `invalid_request_error`. [`sanitize_messages`] patches such gaps with
//! stub `tool_result` blocks so the request always satisfies the API contract.
//!
//! In the common case (no orphans) the original `Vec` is returned untouched —
//! zero allocation, one read-only pass.

use std::collections::HashSet;

use agentik_types::messages::{ContentBlock, Message, Role};
use uuid::Uuid;

/// Insert stub `tool_result` messages for any `tool_use` that is not followed
/// (immediately) by a matching `tool_result`.
///
/// The stub is placed *between* the `tool_use` message and the next message,
/// so that the `tool_result` truly is "immediately after" as the API requires.
///
/// Returns the original `Vec` without allocation when no patches are needed.
pub fn sanitize_messages(messages: Vec<Message>) -> Vec<Message> {
    // Fast path: read-only scan for the first orphan. If none found, return
    // the original Vec — zero allocation.
    if !has_orphan(&messages) {
        return messages;
    }

    // Slow path: rebuild with stubs inserted.
    sanitize_inner(messages)
}

/// Read-only check: does any `tool_use` lack an immediately-following
/// `tool_result`?
fn has_orphan(messages: &[Message]) -> bool {
    for window in messages.windows(2) {
        if window[0].has_tool_use() && !window[1].has_tool_result() {
            return true;
        }
    }
    false
}

fn sanitize_inner(messages: Vec<Message>) -> Vec<Message> {
    let mut sanitized: Vec<Message> = Vec::with_capacity(messages.len() + 1);

    for msg in messages {
        let needs_stub = sanitized
            .last()
            .is_some_and(|prev| prev.has_tool_use() && !msg.has_tool_result());

        if needs_stub {
            let prev = sanitized.last().expect("checked by is_some_and");
            let prev_ids: HashSet<String> = prev
                .tool_uses()
                .iter()
                .filter_map(|c| c.get_tool_call_id())
                .collect();

            if !prev_ids.is_empty() {
                sanitized.push(Message {
                    id: Uuid::new_v4().to_string(),
                    type_: "message".to_string(),
                    role: Role::User,
                    content: prev_ids
                        .into_iter()
                        .map(|tc_id| ContentBlock::ToolResult {
                            tool_use_id: tc_id,
                            content: Some("Tool execution has been interrupted".to_string()),
                            is_error: Some(true),
                        })
                        .collect(),
                    model: None,
                    stop_reason: None,
                    stop_sequence: None,
                    usage: None,
                    request_id: None,
                });
            }
        }

        sanitized.push(msg);
    }

    sanitized
}
