//! Input-history search and persisted transcript conversion helpers.

// ── Ctrl+R history search helpers ──────────────────────────
//
// Free functions operating on `AgentTabState`. Search state lives on the
// state struct (see `state.rs`); these compute matches and drive previews.

use std::collections::VecDeque;

use crate::state;
use agentik_sdk::types::messages::{ContentBlock, Message, Role};

/// Return indices into `history` (newest-first) whose text case-insensitively
/// contains `query`. An empty query matches everything, so the user starts at
/// the most recent entry and narrows as they type.
pub(super) fn compute_search_matches(history: &VecDeque<String>, query: &str) -> Vec<usize> {
    let needle = query.to_lowercase();
    history
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, s)| needle.is_empty() || s.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

/// Load the match at `history_search_selected` into the input buffer so the
/// user sees a live preview as they navigate matches. Clears the buffer when
/// no match is selected.
pub(super) fn load_selected_history_match(ts: &mut crate::state::AgentTabState) {
    if let Some(&idx) = ts.history_search_matches.get(ts.history_search_selected) {
        if let Some(entry) = ts.input_history.get(idx).cloned() {
            ts.input.clear();
            ts.input.insert_str(&entry);
        }
    } else {
        ts.input.clear();
    }
}

/// Recompute the match list for the current query, reset selection to the
/// newest match, and load its preview into the buffer.
pub(super) fn recompute_history_search(ts: &mut crate::state::AgentTabState) {
    let q = ts.history_search_query.clone();
    ts.history_search_matches = compute_search_matches(&ts.input_history, &q);
    ts.history_search_selected = 0;
    load_selected_history_match(ts);
}

/// Leave history search mode, clearing transient search state. The buffer
/// retains whatever was previewed (on Enter) or the restored draft (on Esc);
/// the caller is responsible for buffer contents on the way in.
pub(super) fn end_history_search(ts: &mut crate::state::AgentTabState) {
    ts.in_history_search = false;
    ts.history_search_query.clear();
    ts.history_search_matches.clear();
    ts.history_search_selected = 0;
    ts.history_search_draft = None;
}

/// Convert a flat list of SDK `Message`s (as returned by
/// `Session::render_context()`) into TUI `ChatLine`s for display.
///
/// Each message can contain multiple `ContentBlock`s:
/// - User text → `ChatLine::User`
/// - System text → skipped (system prompt isn't shown)
/// - Assistant text → `ChatLine::Assistant`
/// - Assistant thinking → `ChatLine::Thinking`
/// - `tool_use` → `ChatLine::ToolCall`
/// - `tool_result` → `ChatLine::ToolResult`
///
/// Summary checkpoint messages (from compaction) are rendered as
/// `ChatLine::Assistant` with a prefix so the user can see what was
/// summarized.
pub(super) fn messages_to_chatlines(messages: &[Message]) -> Vec<state::ChatLine> {
    let mut lines = Vec::new();
    for msg in messages {
        match msg.role {
            Role::User => {
                // A user message may contain multiple blocks (text +
                // tool_result). Separate them.
                let mut text_parts = String::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text_parts.is_empty() {
                                text_parts.push('\n');
                            }
                            text_parts.push_str(text);
                        }
                        ContentBlock::ToolResult {
                            content, is_error, ..
                        } => {
                            if !text_parts.is_empty() {
                                lines.push(state::ChatLine::User(std::mem::take(&mut text_parts)));
                            }
                            lines.push(state::ChatLine::ToolResult {
                                ok: !is_error.unwrap_or(false),
                                content: content.clone().unwrap_or_default(),
                            });
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    lines.push(state::ChatLine::User(text_parts));
                }
            }
            Role::Assistant => {
                let mut text_parts = String::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text_parts.is_empty() {
                                text_parts.push('\n');
                            }
                            text_parts.push_str(text);
                        }
                        ContentBlock::Thinking { thinking, .. } if !thinking.is_empty() => {
                            if !text_parts.is_empty() {
                                lines.push(state::ChatLine::Assistant {
                                    text: std::mem::take(&mut text_parts),
                                    usage: None,
                                });
                            }
                            lines.push(state::ChatLine::Thinking(thinking.clone()));
                        }
                        ContentBlock::ToolUse { name, input, .. } => {
                            if !text_parts.is_empty() {
                                lines.push(state::ChatLine::Assistant {
                                    text: std::mem::take(&mut text_parts),
                                    usage: None,
                                });
                            }
                            lines.push(state::ChatLine::ToolCall {
                                name: name.clone(),
                                input: serde_json::to_string_pretty(input)
                                    .unwrap_or_else(|_| input.to_string()),
                            });
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    lines.push(state::ChatLine::Assistant {
                        text: text_parts,
                        usage: None,
                    });
                }
            }
        }
    }
    lines
}
