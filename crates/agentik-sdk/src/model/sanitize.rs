//! Message sanitisation before sending to Anthropic-compatible APIs.
//!
//! Anthropic's Messages API enforces several invariants on the `messages`
//! list. If the agent loop is interrupted (cancel, crash, retry, snapshot
//! replay) between recording a `tool_use` and its `tool_result`, or if
//! `tool_result` blocks are appended twice for the same `tool_use_id`, the
//! request fails with a 400 `invalid_request_error` like:
//!
//! > messages.N.content.M: each tool_use must have a single result. Found
//! > multiple `tool_result` blocks with id: call_00_xxx.
//!
//! [`sanitize_messages`] repairs such damage in one pass, applying these
//! fixes:
//!
//! 1. **Missing `tool_result`** for a trailing `tool_use` → insert a stub
//!    user message with `is_error: Some(true)` content
//!    `"Tool execution has been interrupted"`, so the message list ends with
//!    a user turn.
//! 2. **Duplicate `tool_result`** blocks with the same `tool_use_id` in the
//!    same message (or across the immediately-following user message) → keep
//!    the first, drop the rest. (Anthropic rejects `tool_result` duplicates.)
//! 3. **Orphan `tool_result`** whose `tool_use_id` does not match any
//!    `tool_use` block in the immediately preceding assistant message → drop
//!    (or stub if no preceding assistant message exists).
//! 4. **Empty messages** with no content blocks → drop.
//! 5. **Consecutive same-role messages** → the API requires strictly
//!    alternating user / assistant roles. Coalesce same-role neighbours into
//!    one message.
//! 6. **First message must be a user turn** → if the head is assistant, merge
//!    with the next user message; if the list starts with assistant only,
//!    prepend a placeholder user message.
//!
//! In the common case (no repairs needed) the original `Vec` is returned
//! untouched — zero allocation, one read-only pass.

use std::collections::{HashMap, HashSet};

use agentik_types::messages::{ContentBlock, Message, Role};
use uuid::Uuid;

/// Apply all sanitisation rules and return a clean `messages` list.
///
/// The returned Vec is the input when no rule fired — callers can use
/// `Arc::ptr_eq` (or simply compare `Vec::as_ptr` + `len`) to detect a no-op
/// if needed. Otherwise it is a fresh allocation.
pub fn sanitize_messages(messages: Vec<Message>) -> Vec<Message> {
    if !needs_sanitize(&messages) {
        return messages;
    }
    sanitize_inner(messages)
}

/// Read-only preflight: do any rules need to fire?
fn needs_sanitize(messages: &[Message]) -> bool {
    if messages.is_empty() {
        return false;
    }

    // Rule 6 — head must be user.
    if !matches!(messages[0].role, Role::User) {
        return true;
    }

    // Rule 4 — empty messages.
    if messages.iter().any(|m| m.content.is_empty()) {
        return true;
    }

    // Rule 5 — same-role neighbours.
    if messages
        .windows(2)
        .any(|w| matches_same_role(&w[0].role, &w[1].role))
    {
        return true;
    }

    // Rule 2 — duplicate tool_result for the same tool_use_id, either within
    // one message or split between an assistant message and its immediately
    // following user message (which can happen when tool results are appended
    // before compaction rebuilds adjacency).
    let mut prev_assistant_tool_ids: HashSet<&str> = HashSet::new();
    for w in messages.windows(2) {
        if matches!(w[0].role, Role::Assistant) && matches!(w[1].role, Role::User) {
            collect_tool_use_ids(&w[0], &mut prev_assistant_tool_ids);
            let mut seen: HashSet<&str> = HashSet::new();
            for c in &w[1].content {
                if let ContentBlock::ToolResult { tool_use_id, .. } = c {
                    if !seen.insert(tool_use_id.as_str()) {
                        return true;
                    }
                }
            }
        } else {
            prev_assistant_tool_ids.clear();
        }
    }

    // Also check for duplicate tool_result inside the same user message
    // (e.g. an assistant emitted two tool_uses in one turn and the user
    // message ended up with two tool_results for the same id).
    for m in messages {
        if !matches!(m.role, Role::User) {
            continue;
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for c in &m.content {
            if let ContentBlock::ToolResult { tool_use_id, .. } = c {
                if !seen.insert(tool_use_id.as_str()) {
                    return true;
                }
            }
        }
    }

    // Rule 1 / Rule 3 — orphan tool_use (no following tool_result) or
    // orphan tool_result (no preceding tool_use in the immediately previous
    // assistant message).
    if has_orphan_tool_use(messages) || has_orphan_tool_result(messages) {
        return true;
    }

    false
}

fn matches_same_role(a: &Role, b: &Role) -> bool {
    matches!(
        (a, b),
        (Role::User, Role::User) | (Role::Assistant, Role::Assistant)
    )
}

fn collect_tool_use_ids<'a>(m: &'a Message, out: &mut HashSet<&'a str>) {
    for c in &m.content {
        if let ContentBlock::ToolUse { id, .. } = c {
            out.insert(id.as_str());
        }
    }
}

fn has_orphan_tool_use(messages: &[Message]) -> bool {
    // True when the final message contains a tool_use and is not followed by
    // a user message containing a matching tool_result.
    if let Some((last_idx, _)) = messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, m)| !m.content.is_empty())
    {
        let last = &messages[last_idx];
        if !last.has_tool_use() {
            return false;
        }
        // Last non-empty message holds tool_use(s). Are they all matched by
        // tool_results in the next message (if any)?
        let needed: HashSet<&str> = last
            .content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let provided: HashSet<&str> = match messages.get(last_idx + 1) {
            Some(next) if matches!(next.role, Role::User) => next
                .content
                .iter()
                .filter_map(|c| match c {
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        Some(tool_use_id.as_str())
                    }
                    _ => None,
                })
                .collect(),
            _ => HashSet::new(),
        };
        !needed.is_subset(&provided)
    } else {
        false
    }
}

fn has_orphan_tool_result(messages: &[Message]) -> bool {
    // True when a user message contains a tool_result whose tool_use_id is
    // absent from the immediately-preceding assistant message.
    let mut prev_tool_ids: HashSet<&str> = HashSet::new();
    let mut prev_is_assistant = false;
    for m in messages {
        if matches!(m.role, Role::Assistant) {
            prev_tool_ids.clear();
            collect_tool_use_ids(m, &mut prev_tool_ids);
            prev_is_assistant = true;
        } else if matches!(m.role, Role::User) {
            for c in &m.content {
                if let ContentBlock::ToolResult { tool_use_id, .. } = c {
                    if !prev_is_assistant || !prev_tool_ids.contains(tool_use_id.as_str()) {
                        return true;
                    }
                }
            }
            prev_is_assistant = false;
        }
    }
    false
}

fn sanitize_inner(messages: Vec<Message>) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::with_capacity(messages.len() + 1);

    for msg in messages.into_iter() {
        // Drop empty messages (Rule 4).
        if msg.content.is_empty() {
            continue;
        }

        // Coalesce into the previous message if the role matches (Rule 5/6).
        if let Some(prev) = out.last_mut() {
            if matches_same_role(&prev.role, &msg.role) {
                let mut merged_blocks = std::mem::take(&mut prev.content);
                merged_blocks.extend(msg.content);
                prev.content = dedup_tool_results(merged_blocks);
                continue;
            }
        }

        // Otherwise normalise this message: dedup tool_results within it,
        // then maybe insert an orphan-stub for the *previous* tool_use
        // message before we push it.
        let normalised = Message {
            content: dedup_tool_results(msg.content),
            ..msg
        };

        // Rule 1: if the previous message contains a tool_use whose
        // tool_use_ids are not all covered by `normalised`'s tool_results,
        // insert a stub user message between them.
        if let Some(prev) = out.last() {
            if prev.has_tool_use() && matches!(normalised.role, Role::User) {
                let missing = unmatched_tool_use_ids(prev, &normalised);
                if !missing.is_empty() {
                    out.push(stub_user_message(missing));
                }
            }
        }

        out.push(normalised);
    }

    // Rule 1 trailing pass: if the final message still has unmatched
    // tool_use blocks (no following user message with the matching
    // tool_results), append a synthetic user message carrying stub
    // tool_results so the conversation always ends with a user turn.
    if let Some(last) = out.last() {
        if last.has_tool_use() {
            let last_ids: HashSet<String> = last
                .tool_uses()
                .iter()
                .filter_map(|c| c.get_tool_call_id())
                .collect();
            let provided: HashSet<String> = out
                .iter()
                .rev()
                .nth(1)
                .map(|prev| {
                    prev.tool_results()
                        .iter()
                        .filter_map(|c| c.get_tool_call_id())
                        .collect()
                })
                .unwrap_or_default();
            let missing: Vec<String> = last_ids.difference(&provided).cloned().collect();
            if !missing.is_empty() {
                out.push(stub_user_message(missing));
            }
        }
    }

    // Rule 3 fallback — orphan tool_results at the head (no preceding
    // assistant message). Drop them; without a tool_use there's no slot to
    // attach to.
    let mut prev_had_tool_use = false;
    for m in out.iter_mut() {
        match m.role {
            Role::Assistant => {
                prev_had_tool_use = m.has_tool_use();
            }
            Role::User => {
                if !prev_had_tool_use {
                    m.content.retain(|c| !matches!(c, ContentBlock::ToolResult { .. }));
                }
                prev_had_tool_use = false;
            }
        }
    }

    // Final pass: drop any messages that became empty after the orphan
    // cleanup above.
    out.retain(|m| !m.content.is_empty());

    // Rule 6 — head must be user. If the first surviving message is
    // assistant, prepend a placeholder.
    if let Some(first) = out.first() {
        if !matches!(first.role, Role::User) {
            let placeholder = Message {
                id: Uuid::new_v4().to_string(),
                type_: "message".to_string(),
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "[conversation resumed]".to_string(),
                }],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            };
            out.insert(0, placeholder);
        }
    }

    out
}

/// Compute the `tool_use_id`s in `prev` that are *not* satisfied by
/// `tool_result` blocks in `next`. Used to decide whether to insert a stub
/// user message between two adjacent messages.
fn unmatched_tool_use_ids(prev: &Message, next: &Message) -> Vec<String> {
    let prev_ids: HashSet<String> = prev
        .tool_uses()
        .iter()
        .filter_map(|c| c.get_tool_call_id())
        .collect();
    let provided: HashSet<String> = next
        .tool_results()
        .iter()
        .filter_map(|c| c.get_tool_call_id())
        .collect();
    prev_ids.difference(&provided).cloned().collect()
}

/// Build a synthetic user message whose content is one stub `tool_result`
/// per provided `tool_use_id`, marking the call as interrupted.
fn stub_user_message(tool_use_ids: Vec<String>) -> Message {
    Message {
        id: Uuid::new_v4().to_string(),
        type_: "message".to_string(),
        role: Role::User,
        content: tool_use_ids
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
    }
}

/// Drop duplicate `tool_result` blocks within a single message's content,
/// keeping the first occurrence of each `tool_use_id`.
fn dedup_tool_results(blocks: Vec<ContentBlock>) -> Vec<ContentBlock> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(blocks.len());
    for c in blocks {
        if let ContentBlock::ToolResult { tool_use_id, .. } = &c {
            if !seen.insert(tool_use_id.clone()) {
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Convenience: produce a stub `tool_result` message for the given
/// `tool_use_id`s, useful for tests and as a helper for callers that want to
/// patch a single orphan `tool_use` without rebuilding the entire list.
#[allow(dead_code)]
pub fn stub_tool_result_message(tool_use_ids: impl IntoIterator<Item = String>) -> Message {
    let mut content: Vec<ContentBlock> = tool_use_ids
        .into_iter()
        .map(|tc_id| ContentBlock::ToolResult {
            tool_use_id: tc_id,
            content: Some("Tool execution has been interrupted".to_string()),
            is_error: Some(true),
        })
        .collect();
    if content.is_empty() {
        content.push(ContentBlock::Text {
            text: "[conversation resumed]".to_string(),
        });
    }
    Message {
        id: Uuid::new_v4().to_string(),
        type_: "message".to_string(),
        role: Role::User,
        content,
        model: None,
        stop_reason: None,
        stop_sequence: None,
        usage: None,
        request_id: None,
    }
}

/// Diagnostic: count rule violations without modifying the messages.
///
/// Returns a map of rule name → violation count. Useful for telemetry and
/// tests; the main `sanitize_messages` path uses `needs_sanitize` for
/// performance and only runs the full rebuild when at least one rule fires.
#[allow(dead_code)]
pub fn diagnose(messages: &[Message]) -> HashMap<&'static str, usize> {
    let mut out: HashMap<&'static str, usize> = HashMap::new();

    if !matches!(messages.first().map(|m| &m.role), Some(Role::User)) {
        *out.entry("head_not_user").or_insert(0) += 1;
    }
    *out.entry("empty_messages").or_insert(0) +=
        messages.iter().filter(|m| m.content.is_empty()).count();

    let mut seen: HashSet<&str> = HashSet::new();
    let mut dups = 0usize;
    for m in messages {
        if !matches!(m.role, Role::User) {
            continue;
        }
        seen.clear();
        for c in &m.content {
            if let ContentBlock::ToolResult { tool_use_id, .. } = c {
                if !seen.insert(tool_use_id.as_str()) {
                    dups += 1;
                }
            }
        }
    }
    if dups > 0 {
        *out.entry("duplicate_tool_result").or_insert(0) += dups;
    }

    if has_orphan_tool_use(messages) {
        *out.entry("orphan_tool_use").or_insert(0) += 1;
    }
    if has_orphan_tool_result(messages) {
        *out.entry("orphan_tool_result").or_insert(0) += 1;
    }
    *out.entry("same_role_neighbors").or_insert(0) += messages
        .windows(2)
        .filter(|w| matches_same_role(&w[0].role, &w[1].role))
        .count();

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_types::messages::{ContentBlock, Message, Role};

    fn text_user(s: &str) -> Message {
        Message {
            id: "u".into(),
            type_: "message".into(),
            role: Role::User,
            content: vec![ContentBlock::Text { text: s.into() }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        }
    }

    fn text_assistant(s: &str) -> Message {
        Message {
            id: "a".into(),
            type_: "message".into(),
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: s.into() }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        }
    }

    fn assistant_tool_use(id: &str) -> Message {
        Message {
            id: "a".into(),
            type_: "message".into(),
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: "t".into(),
                input: serde_json::json!({}),
            }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        }
    }

    fn tool_result(id: &str, content: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: Some(content.into()),
            is_error: Some(false),
        }
    }

    fn user_with_tool_results(ids: &[&str]) -> Message {
        Message {
            id: "u".into(),
            type_: "message".into(),
            role: Role::User,
            content: ids.iter().map(|i| tool_result(i, "ok")).collect(),
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        }
    }

    #[test]
    fn clean_passes_through_unchanged() {
        let msgs = vec![
            text_user("hi"),
            text_assistant("hello"),
            text_user("how are you?"),
            text_assistant("fine"),
        ];
        let before_ptr = msgs.as_ptr();
        let out = sanitize_messages(msgs);
        // No allocation: same Vec identity (preserved by `return messages`).
        assert_eq!(out.len(), 4);
        assert!(out[0].content.iter().any(|c| matches!(c, ContentBlock::Text { .. })));
        // Best-effort identity check — pointer comparison is valid since we
        // returned the original Vec untouched.
        assert_eq!(out.as_ptr(), before_ptr as *const Message);
    }

    #[test]
    fn empty_messages_dropped() {
        let empty_user = Message {
            id: "u".into(),
            type_: "message".into(),
            role: Role::User,
            content: vec![],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        };
        let msgs = vec![text_user("hi"), empty_user, text_assistant("ok")];
        let out = sanitize_messages(msgs);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0].role, Role::User));
        assert!(matches!(out[1].role, Role::Assistant));
    }

    #[test]
    fn consecutive_user_messages_coalesced() {
        let msgs = vec![
            text_user("hello"),
            text_user("world"),
            text_assistant("ok"),
        ];
        let out = sanitize_messages(msgs);
        assert_eq!(out.len(), 2);
        // Coalesced content carries both texts.
        let combined = out[0]
            .content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("|");
        assert_eq!(combined, "hello|world");
    }

    #[test]
    fn consecutive_assistant_messages_coalesced() {
        let msgs = vec![
            text_user("hi"),
            text_assistant("part 1"),
            text_assistant("part 2"),
            text_user("done"),
        ];
        let out = sanitize_messages(msgs);
        assert_eq!(out.len(), 3);
        let combined = out[1]
            .content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("|");
        assert_eq!(combined, "part 1|part 2");
    }

    #[test]
    fn head_must_be_user_prepends_placeholder() {
        let msgs = vec![text_assistant("hi"), text_user("hello")];
        let out = sanitize_messages(msgs);
        assert!(matches!(out[0].role, Role::User));
        assert!(matches!(out[1].role, Role::Assistant));
        assert!(matches!(out[2].role, Role::User));
    }

    #[test]
    fn orphan_trailing_tool_use_gets_stub() {
        let msgs = vec![text_user("hi"), assistant_tool_use("call_001")];
        let out = sanitize_messages(msgs);
        // Three messages: user prompt, assistant tool_use, stub user
        // message carrying the synthetic tool_result.
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0].role, Role::User));
        assert!(matches!(out[1].role, Role::Assistant));
        assert!(matches!(out[2].role, Role::User));
        let last = out.last().expect("non-empty");
        let has_stub = last.content.iter().any(|c| {
            matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_001")
        });
        assert!(has_stub, "expected stub tool_result for call_001");
        // And the stub should be marked as an error.
        let stub_is_error = last.content.iter().any(|c| {
            matches!(
                c,
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error: Some(true),
                    ..
                } if tool_use_id == "call_001"
            )
        });
        assert!(stub_is_error, "stub tool_result should have is_error=true");
    }

    #[test]
    fn duplicate_tool_result_in_single_message_collapsed() {
        // Same tool_use_id appears twice in the same user message — the
        // payload that triggered the original API bug.
        let msgs = vec![
            text_user("hi"),
            Message {
                id: "a".into(),
                type_: "message".into(),
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_dup".into(),
                    name: "t".into(),
                    input: serde_json::json!({}),
                }],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            },
            Message {
                id: "u".into(),
                type_: "message".into(),
                role: Role::User,
                content: vec![
                    tool_result("call_dup", "first result"),
                    tool_result("call_dup", "second result"),
                ],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            },
        ];
        let out = sanitize_messages(msgs);
        // Collect all tool_result blocks for call_dup across the whole
        // conversation — there should be exactly one.
        let dupes: Vec<_> = out
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_dup"))
            .collect();
        assert_eq!(dupes.len(), 1, "duplicate tool_result must be collapsed");
        // First occurrence is kept.
        if let ContentBlock::ToolResult { content, .. } = &dupes[0] {
            assert_eq!(content.as_deref(), Some("first result"));
        } else {
            panic!("expected ToolResult");
        }
    }

    #[test]
    fn duplicate_tool_result_split_across_messages_collapsed() {
        // user[2] holds the first tool_result for "call_X", and user[3] gets
        // a second one appended — the exact bug shape from add_message()'s
        // unconditional .push() path. Sanitiser must collapse.
        let msgs = vec![
            text_user("hi"),
            assistant_tool_use("call_X"),
            Message {
                id: "u1".into(),
                type_: "message".into(),
                role: Role::User,
                content: vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "call_X".into(),
                        content: Some("first".into()),
                        is_error: Some(false),
                    },
                    ContentBlock::Text {
                        text: "between".into(),
                    },
                ],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            },
            Message {
                id: "u2".into(),
                type_: "message".into(),
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call_X".into(),
                    content: Some("second".into()),
                    is_error: Some(false),
                }],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            },
        ];
        let out = sanitize_messages(msgs);
        let total_dupes: usize = out
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_X"))
            .count();
        assert_eq!(total_dupes, 1, "exactly one tool_result for call_X must remain");
    }

    #[test]
    fn orphan_tool_result_dropped() {
        // tool_result with no preceding tool_use.
        let msgs = vec![
            text_user("hi"),
            Message {
                id: "u".into(),
                type_: "message".into(),
                role: Role::User,
                content: vec![tool_result("phantom", "where did this come from?")],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            },
        ];
        let out = sanitize_messages(msgs);
        let orphan_count = out
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "phantom"))
            .count();
        assert_eq!(orphan_count, 0, "orphan tool_result must be dropped");
    }

    #[test]
    fn multiple_tool_uses_all_get_results() {
        let msgs = vec![
            text_user("hi"),
            Message {
                id: "a".into(),
                type_: "message".into(),
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "a".into(),
                        name: "t".into(),
                        input: serde_json::json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "b".into(),
                        name: "t".into(),
                        input: serde_json::json!({}),
                    },
                ],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            },
            user_with_tool_results(&["a", "b"]),
        ];
        let out = sanitize_messages(msgs);
        let total = out.iter().flat_map(|m| m.content.iter()).count();
        assert!(total >= 4); // user text + 2 tool_uses + 2 tool_results
    }

    #[test]
    fn diagnose_finds_violations() {
        let msgs = vec![
            Message {
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
            },
            Message {
                id: "u".into(),
                type_: "message".into(),
                role: Role::User,
                content: vec![
                    tool_result("call_X", "ok"),
                    tool_result("call_X", "ok again"),
                ],
                model: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                request_id: None,
            },
        ];
        let diag = diagnose(&msgs);
        assert!(diag.get("duplicate_tool_result").copied().unwrap_or(0) >= 1);
        assert!(diag.get("head_not_user").copied().unwrap_or(0) >= 1);
    }

    #[test]
    fn fast_path_zero_messages_returns_input() {
        let msgs: Vec<Message> = vec![];
        let out = sanitize_messages(msgs);
        assert!(out.is_empty());
    }

    #[test]
    fn stub_tool_result_message_helper() {
        let m = stub_tool_result_message(vec!["a".to_string(), "b".to_string()]);
        assert!(matches!(m.role, Role::User));
        assert_eq!(m.content.len(), 2);
    }

    #[test]
    fn empty_stub_falls_back_to_text() {
        let m = stub_tool_result_message(Vec::<String>::new());
        assert!(matches!(m.role, Role::User));
        assert_eq!(m.content.len(), 1);
        assert!(matches!(m.content[0], ContentBlock::Text { .. }));
    }
}
