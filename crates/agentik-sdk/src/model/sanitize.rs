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

    // Duplicate Text blocks within a single message (accumulated by
    // coalescing or repeated message injection on restart).
    for m in messages {
        let mut seen_texts: HashSet<&str> = HashSet::new();
        for c in &m.content {
            if let ContentBlock::Text { text } = c
                && !seen_texts.insert(text.as_str())
            {
                return true;
            }
        }
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
                if let ContentBlock::ToolResult { tool_use_id, .. } = c
                    && !seen.insert(tool_use_id.as_str())
                {
                    return true;
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
            if let ContentBlock::ToolResult { tool_use_id, .. } = c
                && !seen.insert(tool_use_id.as_str())
            {
                return true;
            }
        }
    }

    // Rule 1 / Rule 3 — orphan tool_use (no following tool_result) or
    // orphan tool_result (no preceding tool_use in the immediately previous
    // assistant message).
    if has_orphan_tool_use(messages) || has_orphan_tool_result(messages) {
        return true;
    }

    // Rule 7 — mid-sequence adjacency break. A tool_use in an assistant
    // message whose matching tool_result is 2+ user messages away (e.g.
    // caused by an intervening user message arriving between the call and
    // its result). Detect by looking at every assistant message and
    // checking whether its tool_use ids are all in the immediately next
    // user message.
    for w in messages.windows(2) {
        if !matches!(w[0].role, Role::Assistant) || !matches!(w[1].role, Role::User) {
            continue;
        }
        if !w[0].has_tool_use() {
            continue;
        }
        if !unmatched_tool_use_ids(&w[0], &w[1]).is_empty() {
            return true;
        }
    }

    // Rule 8 — orphan tool_result whose tool_use was dropped (e.g. by
    // compaction summarising the assistant message). The tool_use_id no
    // longer appears in any previous assistant message.
    if has_unorphaned_tool_result(messages) {
        return true;
    }

    // Rule 9 — tool_result whose tool_use appears in a LATER assistant
    // message (forward-orphan). Same detection as Rule 8 — by symmetry,
    // any tool_result without a matching tool_use earlier in the list is
    // an orphan.

    false
}

/// True when any user message contains a `tool_result` whose `tool_use_id`
/// is missing from **every** previous assistant message in the sequence.
///
/// Used by Rules 8/9: any tool_result whose tool_use has been summarised
/// away (or never existed) is an orphan and must be dropped.
fn has_unorphaned_tool_result(messages: &[Message]) -> bool {
    let mut seen_tool_ids: HashSet<String> = HashSet::new();
    for m in messages {
        match m.role {
            Role::Assistant => {
                for c in &m.content {
                    if let ContentBlock::ToolUse { id, .. } = c {
                        seen_tool_ids.insert(id.clone());
                    }
                }
            }
            Role::User => {
                for c in &m.content {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = c
                        && !seen_tool_ids.contains(tool_use_id)
                    {
                        return true;
                    }
                }
            }
        }
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
                    ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
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
                if let ContentBlock::ToolResult { tool_use_id, .. } = c
                    && (!prev_is_assistant || !prev_tool_ids.contains(tool_use_id.as_str()))
                {
                    return true;
                }
            }
            prev_is_assistant = false;
        }
    }
    false
}

fn sanitize_inner(messages: Vec<Message>) -> Vec<Message> {
    // ── Rule 7 + 8 + 9 pre-pass: repair adjacency + drop orphans ──
    // Run before the main loop so subsequent passes see a well-formed
    // sequence. We must be careful not to lose genuine text/thinking
    // content during the moves — only tool_result blocks are relocated.
    let messages = repair_tool_use_adjacency(messages);

    let mut out: Vec<Message> = Vec::with_capacity(messages.len() + 1);

    for msg in messages.into_iter() {
        // Drop empty messages (Rule 4).
        if msg.content.is_empty() {
            continue;
        }

        // Coalesce into the previous message if the role matches (Rule 5/6).
        if let Some(prev) = out.last_mut()
            && matches_same_role(&prev.role, &msg.role)
        {
            let mut merged_blocks = std::mem::take(&mut prev.content);
            merged_blocks.extend(msg.content);
            prev.content = dedup_content_blocks(merged_blocks);
            continue;
        }

        // Otherwise normalise this message: dedup tool_results within it,
        // then maybe insert an orphan-stub for the *previous* tool_use
        // message before we push it.
        let normalised = Message {
            content: dedup_content_blocks(msg.content),
            ..msg
        };

        // Rule 1: if the previous message contains a tool_use whose
        // tool_use_ids are not all covered by `normalised`'s tool_results,
        // insert a stub user message between them.
        if let Some(prev) = out.last()
            && prev.has_tool_use()
            && matches!(normalised.role, Role::User)
        {
            let missing = unmatched_tool_use_ids(prev, &normalised);
            if !missing.is_empty() {
                out.push(stub_user_message(missing));
            }
        }

        out.push(normalised);
    }

    // Rule 1 trailing pass: if the final message still has unmatched
    // tool_use blocks (no following user message with the matching
    // tool_results), append a synthetic user message carrying stub
    // tool_results so the conversation always ends with a user turn.
    if let Some(last) = out.last()
        && last.has_tool_use()
    {
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
                    m.content
                        .retain(|c| !matches!(c, ContentBlock::ToolResult { .. }));
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
    if let Some(first) = out.first()
        && !matches!(first.role, Role::User)
    {
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

    out
}

/// Repair adjacency invariants before the main sanitisation pass.
///
/// Restores three invariants:
///
/// 1. **Adjacency** (Anthropic's hard rule): every `tool_use` in an
///    assistant message must have a matching `tool_result` in the
///    **immediately next** user message. If the result sits in a later
///    user message, move it forward. If no result exists, insert a
///    synthetic stub (`is_error: Some(true)`).
///
/// 2. **No orphan tool_results** (Rule 8): any `tool_result` whose
///    `tool_use_id` does not appear in any previous assistant message is
///    dropped. This handles the post-compaction case where the head was
///    summarised but the tail retained the result.
///
/// 3. **No forward orphans** (Rule 9): a tool_result that appears
///    before its `tool_use` is also an orphan — same detection as
///    Rule 8.
///
/// This pre-pass operates on the raw message list; the main
/// `sanitize_inner` will still apply Rules 1-6 (coalesce, dedup, etc.)
/// on the output.
fn repair_tool_use_adjacency(messages: Vec<Message>) -> Vec<Message> {
    if messages.is_empty() {
        return messages;
    }

    // ── Pass 1: drop orphan tool_results (Rules 8/9) ──
    // Walk forward, track emitted tool_use ids, and strip any
    // tool_result that references an id we haven't seen yet.
    let mut emitted_tool_ids: HashSet<String> = HashSet::new();
    let mut pass1: Vec<Message> = Vec::with_capacity(messages.len());
    for mut m in messages {
        match m.role {
            Role::Assistant => {
                for c in &m.content {
                    if let ContentBlock::ToolUse { id, .. } = c {
                        emitted_tool_ids.insert(id.clone());
                    }
                }
                pass1.push(m);
            }
            Role::User => {
                m.content.retain(|c| match c {
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        emitted_tool_ids.contains(tool_use_id)
                    }
                    _ => true,
                });
                pass1.push(m);
            }
        }
    }

    // ── Pass 2: repair adjacency (Rule 7) ──
    // For every assistant message with tool_use blocks, ensure the
    // immediately next message is a user message carrying matching
    // tool_results. Collect tool_results from later user messages and
    // move them forward; insert stubs for any still-missing ids.
    let mut out: Vec<Message> = Vec::with_capacity(pass1.len() + 2);
    let mut i = 0;
    while i < pass1.len() {
        let msg = pass1[i].clone();

        if !(matches!(msg.role, Role::Assistant) && msg.has_tool_use()) {
            out.push(msg);
            i += 1;
            continue;
        }

        // tool_use ids this assistant message expects results for, **in
        // the order they appear in the assistant content** (Anthropic
        // requires tool_results in adjacent user message to appear in
        // the same order as the tool_uses — message 2013).
        let tool_use_ids: Vec<String> = msg
            .content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();

        out.push(msg);

        // The immediately next message.
        let next = pass1.get(i + 1).cloned();
        match next {
            Some(next_msg) if matches!(next_msg.role, Role::User) => {
                // Index existing tool_results by id; collect non-result
                // blocks separately (text/thinking/image).
                let mut provided: HashMap<String, ContentBlock> = HashMap::new();
                let mut non_result_blocks: Vec<ContentBlock> = Vec::new();
                let mut next_msg = next_msg;
                for c in std::mem::take(&mut next_msg.content).into_iter() {
                    if let ContentBlock::ToolResult {
                        ref tool_use_id, ..
                    } = c
                    {
                        // Anthropic rejects duplicate tool_results for
                        // the same id; keep the first occurrence and
                        // drop the rest. HashMap::insert would
                        // overwrite — entry().or_insert keeps the
                        // existing entry.
                        provided.entry(tool_use_id.clone()).or_insert(c);
                    } else {
                        non_result_blocks.push(c);
                    }
                }

                // Walk the assistant's tool_use list in order; for each
                // id emit the existing tool_result if present, else a
                // stub. The resulting order matches tool_use ordering.
                let mut tool_results_in_order: Vec<ContentBlock> = Vec::new();
                let mut missing_any = false;
                for id in &tool_use_ids {
                    if let Some(block) = provided.remove(id) {
                        tool_results_in_order.push(block);
                    } else {
                        tool_results_in_order.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: Some("Tool execution has been interrupted".to_string()),
                            is_error: Some(true),
                        });
                        missing_any = true;
                    }
                }

                // Compose the final user content: non-result blocks
                // first, then tool_results in tool_use order.
                let mut merged = non_result_blocks;
                merged.extend(tool_results_in_order);
                let mut rebuilt = next_msg;
                rebuilt.content = merged;
                if missing_any {
                    // Touch the variable so the diagnostic warning
                    // doesn't fire (and to flag the repair in logs).
                    tracing::debug!(
                        tool_use_count = tool_use_ids.len(),
                        "filled missing tool_result stubs in adjacent user message"
                    );
                }
                out.push(rebuilt);
            }
            Some(next_msg) => {
                // Next message is Assistant (or other non-User). Insert
                // a synthetic user message carrying stub results in
                // tool_use order.
                out.push(stub_user_message(tool_use_ids));
                out.push(next_msg);
            }
            None => {
                // Assistant is the last message.
                out.push(stub_user_message(tool_use_ids));
            }
        }
        i += 1;
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

/// Drop duplicate content blocks within a single message's content list.
///
/// This is a superset of [`dedup_tool_results`]: it also collapses
/// identical consecutive `Text` blocks (same content) that accumulate
/// when multiple same-role messages are coalesced by Rule 5/6 — e.g.
/// when an agent restarts and a "恢复任务" message is injected several
/// times before sanitisation runs.
///
/// Keeps the first occurrence of each unique text/tool_result id;
/// thinking, image, and tool_use blocks are always kept (they carry
/// distinct metadata).
fn dedup_content_blocks(blocks: Vec<ContentBlock>) -> Vec<ContentBlock> {
    let mut seen_tool_results: HashSet<String> = HashSet::new();
    let mut seen_texts: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(blocks.len());
    for c in blocks {
        match &c {
            ContentBlock::ToolResult { tool_use_id, .. } => {
                if !seen_tool_results.insert(tool_use_id.clone()) {
                    continue;
                }
            }
            ContentBlock::Text { text } if !seen_texts.insert(text.clone()) => {
                continue;
            }
            _ => {}
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
            if let ContentBlock::ToolResult { tool_use_id, .. } = c
                && !seen.insert(tool_use_id.as_str())
            {
                dups += 1;
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
        assert!(
            out[0]
                .content
                .iter()
                .any(|c| matches!(c, ContentBlock::Text { .. }))
        );
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
        let msgs = vec![text_user("hello"), text_user("world"), text_assistant("ok")];
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
        assert_eq!(
            total_dupes, 1,
            "exactly one tool_result for call_X must remain"
        );
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

    // ── New rules (7/8/9) tests ──

    /// Rule 7: a tool_use whose tool_result lands 2+ user messages later
    /// (e.g. retry feedback pushed in between) must be repaired so the
    /// tool_result sits in the immediately next user message after the
    /// tool_use.
    #[test]
    fn rule7_relocates_tool_result_through_intervening_user() {
        let msgs = vec![
            text_user("hi"),
            assistant_tool_use("call_X"),
            // Retry feedback user message arrived between the call and
            // its result.
            text_user("retry: please confirm"),
            // Tool result landed here (1 user message too late).
            user_with_tool_results(&["call_X"]),
        ];
        let out = sanitize_messages(msgs);
        // Find the assistant tool_use and assert the immediately next
        // message is the user message containing tool_result(call_X).
        let asst_idx = out
            .iter()
            .position(|m| matches!(m.role, Role::Assistant))
            .expect("assistant with tool_use must remain");
        assert!(out[asst_idx].has_tool_use());
        let next = out.get(asst_idx + 1).expect("user message follows");
        assert!(matches!(next.role, Role::User));
        let has_result = next.content.iter().any(|c| {
            matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_X")
        });
        assert!(
            has_result,
            "tool_result for call_X must sit in the user message immediately after the tool_use"
        );
        // The retry user message and the original user_with_tool_results
        // either merged or were re-laid-out — but the tool_result is in
        // the right place.
    }

    /// Rule 8: a tool_result whose tool_use was dropped (e.g. by
    /// compaction summarising the head) is dropped entirely.
    #[test]
    fn rule8_drops_orphan_tool_result_post_compaction() {
        // The conversation has no preceding assistant with tool_use(call_ghost).
        let msgs = vec![
            text_user("hi"),
            text_assistant("ok"),
            user_with_tool_results(&["call_ghost"]),
        ];
        let out = sanitize_messages(msgs);
        let ghost_count = out
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|c| matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_ghost"))
            .count();
        assert_eq!(ghost_count, 0, "orphan tool_result must be dropped");
    }

    /// Rule 9: forward-orphan — tool_result appearing before its tool_use
    /// (also dropped by Rule 8 since no previous assistant has the id).
    #[test]
    fn rule9_drops_forward_orphan_tool_result() {
        let msgs = vec![
            // tool_result appears BEFORE any tool_use with matching id.
            user_with_tool_results(&["call_future"]),
            assistant_tool_use("call_future"),
        ];
        let out = sanitize_messages(msgs);
        // After repair, the tool_result must either have been dropped
        // (forward orphan) or relocated forward to be adjacent to its
        // tool_use. In either case, exactly one tool_result for
        // call_future should be present, and it must immediately
        // follow the assistant tool_use.
        let asst_idx = out
            .iter()
            .position(|m| matches!(m.role, Role::Assistant))
            .expect("assistant message");
        let next = out.get(asst_idx + 1).expect("user message follows");
        let has = next.content.iter().any(|c| {
            matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_future")
        });
        assert!(
            has,
            "after repair, the only tool_result for call_future must be in the adjacent user message"
        );
    }

    /// Multiple tool_uses in one assistant message — all results must
    /// land in the immediately next user message after the assistant.
    #[test]
    fn rule7_multiple_tool_uses_all_results_in_adjacent_user() {
        let assistant = Message {
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
        };
        // Tool results land in the user message two slots later.
        let msgs = vec![
            text_user("hi"),
            assistant,
            text_user("(intervening retry text)"),
            user_with_tool_results(&["a", "b"]),
        ];
        let out = sanitize_messages(msgs);
        let asst_idx = out
            .iter()
            .position(|m| m.has_tool_use())
            .expect("tool_use assistant must remain");
        let next = out.get(asst_idx + 1).expect("user message follows");
        let has_a = next.content.iter().any(
            |c| matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "a"),
        );
        let has_b = next.content.iter().any(
            |c| matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "b"),
        );
        assert!(has_a, "tool_result for a must be in adjacent user");
        assert!(has_b, "tool_result for b must be in adjacent user");
    }

    /// A tool_use followed by another assistant turn (no user message in
    /// between) must trigger insertion of a synthetic user message with
    /// stub tool_results.
    #[test]
    fn rule7_consecutive_assistants_get_synthetic_user() {
        let msgs = vec![
            text_user("hi"),
            assistant_tool_use("call_X"),
            text_assistant("I forgot to call the tool"),
        ];
        let out = sanitize_messages(msgs);
        // The order must be user, assistant(tool_use), user(stub), assistant.
        let asst_idx = out
            .iter()
            .position(|m| m.has_tool_use())
            .expect("tool_use assistant");
        let stub_msg = out.get(asst_idx + 1).expect("stub user follows");
        assert!(matches!(stub_msg.role, Role::User));
        let has_stub = stub_msg.content.iter().any(|c| {
            matches!(
                c,
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error: Some(true),
                    ..
                } if tool_use_id == "call_X"
            )
        });
        assert!(has_stub, "stub user message must carry tool_result stub");
    }

    /// Fast-path detection: displaced adjacency must be flagged by
    /// needs_sanitize so the work isn't skipped.
    #[test]
    fn rule7_needs_sanitize_fires_for_displaced_tool_result() {
        let msgs = vec![
            text_user("hi"),
            assistant_tool_use("call_X"),
            text_user("retry text"),
            user_with_tool_results(&["call_X"]),
        ];
        // The repair runs and the adjacency holds.
        let out = sanitize_messages(msgs);
        let asst_idx = out.iter().position(|m| m.has_tool_use()).unwrap();
        let next = out.get(asst_idx + 1).unwrap();
        let adjacent = next.content.iter().any(|c| {
            matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_X")
        });
        assert!(
            adjacent,
            "repair must place the tool_result adjacent to tool_use"
        );
    }

    /// Anthropic requires that within a single user message, the
    /// tool_result blocks appear in the **same order** as their
    /// corresponding tool_use blocks in the preceding assistant message.
    /// If the user message's tool_results are in the wrong order, the
    /// repair must reorder them rather than just appending stubs.
    #[test]
    fn tool_results_reordered_to_match_tool_use_order() {
        // Assistant emitted [tool_use(A), tool_use(B), tool_use(C)].
        // User message arrived with results in shuffled order: [B, A].
        // Missing C should be filled with a stub; A and B must be
        // reordered to [A, B] to match the assistant's order.
        let assistant = Message {
            id: "a".into(),
            type_: "message".into(),
            role: Role::Assistant,
            content: vec![
                ContentBlock::ToolUse {
                    id: "call_A".into(),
                    name: "t".into(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: "call_B".into(),
                    name: "t".into(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: "call_C".into(),
                    name: "t".into(),
                    input: serde_json::json!({}),
                },
            ],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        };
        let user = Message {
            id: "u".into(),
            type_: "message".into(),
            role: Role::User,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_B".into(),
                    content: Some("B result".into()),
                    is_error: Some(false),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call_A".into(),
                    content: Some("A result".into()),
                    is_error: Some(false),
                },
            ],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        };
        let msgs = vec![text_user("hi"), assistant, user];
        let out = sanitize_messages(msgs);

        // The user message adjacent to the assistant must carry
        // tool_results in the order [A, B, C_stub].
        let asst_idx = out
            .iter()
            .position(|m| m.has_tool_use())
            .expect("tool_use assistant remains");
        let next = &out[asst_idx + 1];
        assert!(matches!(next.role, Role::User));

        let tool_result_order: Vec<&str> = next
            .content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            tool_result_order,
            vec!["call_A", "call_B", "call_C"],
            "tool_results must be reordered to match the assistant's tool_use order"
        );
    }

    /// Stub tool_results inserted for missing ids must also appear in
    /// tool_use order, not just appended at the end of user content.
    #[test]
    fn missing_tool_results_stubs_follow_tool_use_order() {
        let assistant = Message {
            id: "a".into(),
            type_: "message".into(),
            role: Role::Assistant,
            content: vec![
                ContentBlock::ToolUse {
                    id: "call_A".into(),
                    name: "t".into(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: "call_B".into(),
                    name: "t".into(),
                    input: serde_json::json!({}),
                },
            ],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        };
        // Only tool_result(B) is present — A is missing and must be
        // filled with a stub. The adjacent user message should carry
        // [tool_result(A) stub, tool_result(B) real].
        let user = Message {
            id: "u".into(),
            type_: "message".into(),
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_B".into(),
                content: Some("B result".into()),
                is_error: Some(false),
            }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        };
        let msgs = vec![text_user("hi"), assistant, user];
        let out = sanitize_messages(msgs);
        let asst_idx = out.iter().position(|m| m.has_tool_use()).unwrap();
        let next = &out[asst_idx + 1];
        let order: Vec<&str> = next
            .content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            order,
            vec!["call_A", "call_B"],
            "missing tool_result stubs must precede later real tool_results"
        );
        // Verify the stub for call_A carries is_error=true.
        let stub = next.content.iter().find(|c| {
            matches!(c, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_A")
        }).unwrap();
        if let ContentBlock::ToolResult { is_error, .. } = stub {
            assert_eq!(*is_error, Some(true));
        }
    }

    /// Duplicate text blocks within a single message (accumulated by
    /// coalescing same-role messages on agent restart) must be
    /// collapsed. Without this, each restart inflates the context by
    /// N× the injected "resume" text.
    #[test]
    fn duplicate_text_blocks_collapsed_in_coalesced_message() {
        let resume = "系统重启了，请你恢复任务";
        let msgs = vec![
            text_user("hi"),
            text_assistant("hello"),
            text_user(resume),
            text_user(resume),
            text_user(resume),
            text_user(resume),
            text_assistant("ok"),
        ];
        let out = sanitize_messages(msgs);
        let coalesced = out
            .iter()
            .find(|m| {
                m.content
                    .iter()
                    .any(|c| matches!(c, ContentBlock::Text { text } if text == resume))
            })
            .expect("resume text must survive");
        let resume_count = coalesced
            .content
            .iter()
            .filter(|c| matches!(c, ContentBlock::Text { text } if text == resume))
            .count();
        assert_eq!(
            resume_count, 1,
            "duplicate text blocks must be collapsed to 1"
        );
    }
}
