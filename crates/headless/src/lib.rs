//! Headless (non-interactive) execution of agentik agents.
//!
//! This crate is the UI-free sibling of the TUI: both entry points sit on
//! the gateway daemon (`gateway::daemon`) and never share UI objects. It
//! provides
//!
//! - [`event`]: the stable external run-event contract (`RunEvent`),
//!   emitted as JSONL on stdout in `--json` mode;
//! - [`processor`]: pluggable output rendering (human-readable vs JSONL)
//!   over the same translated event stream;
//! - [`gateway_runner`]: the run loop — submit one prompt to the resident
//!   gateway daemon (auto-spawned when absent), translate the agent's
//!   event stream into [`RunEvent`]s until the turn reaches a terminal
//!   state, then shut the one-shot agent down cleanly.
//!
//! # Termination semantics
//!
//! The authoritative signal is `AgentEvent::TurnCompleted` for the
//! top-level turn (no delegation): `Completed`, `Failed`, or
//! `Interrupted` (mapped to a cancelled run). The compatibility `Done` /
//! `Error` events are cross-checks only, never triggers.
//!
//! # stdout discipline
//!
//! Borrowed from codex's `exec`: in human mode the only bytes written to
//! stdout are the final agent message (if any); in `--json` mode stdout is
//! strictly JSONL, one event per line. Everything else — progress, tool
//! output, warnings — goes to the progress stream (stderr). Processors
//! therefore write through injected [`std::io::Write`] targets instead of
//! printing, which also keeps them unit-testable against buffers.

#![deny(clippy::print_stdout)]

pub mod event;
pub mod gateway_runner;
pub mod manifest;
pub mod processor;

use std::collections::VecDeque;

use agentik_core::AgentProfile;
use agentik_types::{AgentEvent, CompactEvent, TurnExecutionStatus};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use event::{
    AgentMessageItem, ItemEvent, NoticeEvent, NoticeKind, ReasoningItem, RunEvent, RunItem,
    RunItemDetails, ToolCallItem, TurnCompletedEvent, TurnFailedEvent, TurnStartedEvent, Usage,
};

/// Terminal outcome of a completed headless run.
#[derive(Debug, Clone)]
pub struct RunSummary {
    pub run_id: Uuid,
    pub outcome: processor::Outcome,
    pub agent_path: String,
    /// Resolved profile path the agent ran with.
    pub profile: String,
    pub last_message: Option<String>,
    pub wall_time_secs: f64,
    /// Cumulative usage across all turns, when any was reported.
    pub usage: Option<Usage>,
    pub turns: u64,
    /// Tool calls that completed during the run.
    pub tool_calls: u64,
}

/// Startup-phase failures — distinct from a failed *turn*, which is a
/// measured result of the run, not an infrastructure error. The CLI maps
/// these to a different exit code (3) than turn failure (1).
#[derive(Debug, Error)]
pub enum RunError {
    #[error("gateway error: {0}")]
    Gateway(String),
    #[error("no agent profile available (requested: {requested:?})")]
    NoProfile { requested: Option<String> },
    #[error("agent spawn failed: {message}")]
    Spawn { message: String },
    #[error("prompt delivery failed: {message}")]
    Send { message: String },
    #[error("session switch failed for {session}: {message}")]
    SessionSwitch { session: Uuid, message: String },
    #[error("run cancelled")]
    Cancelled,
}

/// Pick the profile to run: exact `path` match when requested, else the
/// first stored profile.
fn pick_profile(profiles: &[AgentProfile], requested: Option<&str>) -> Option<AgentProfile> {
    match requested {
        Some(path) => profiles
            .iter()
            .find(|p| p.path == path)
            .or_else(|| profiles.iter().find(|p| p.name() == path))
            .cloned(),
        None => profiles.first().cloned(),
    }
}

/// Terminal state extracted from the authoritative turn event.
#[derive(Debug, Clone)]
enum Terminal {
    Completed,
    /// The failure detail was already emitted as a `turn.failed` event;
    /// the variant itself only discriminates the outcome.
    Failed,
    Cancelled,
}

/// One-shot translation state: pairs tool calls with results, tracks
/// per-turn and cumulative usage, and captures the terminal turn status.
#[derive(Debug, Default)]
struct TranslationState {
    next_item: usize,
    pending_tools: VecDeque<(String, String, Value)>,
    turn_usage: Usage,
    run_usage: Usage,
    turns: u64,
    tool_calls: u64,
    terminal: Option<Terminal>,
    /// Turn id of the in-flight top-level turn, from `TurnStarted`.
    current_turn_id: Uuid,
    /// Most recent retryable-error message — used as the failure text
    /// when the turn then fails.
    error_hint: Option<String>,
}

impl TranslationState {
    fn next_id(&mut self, prefix: &str) -> String {
        self.next_item += 1;
        format!("{prefix}-{}", self.next_item)
    }

    /// Translate one agent event into zero or more run events. Events
    /// from agents other than the spawned one (delegated children) are
    /// translated too, but their turn boundaries never terminate the run.
    fn translate(&mut self, _agent: &str, event: AgentEvent) -> Vec<RunEvent> {
        match event {
            AgentEvent::TurnStarted {
                turn_id,
                session_id,
                delegation_id: None,
            } => {
                // A fresh top-level turn: reset per-turn usage.
                self.turn_usage = Usage::default();
                self.current_turn_id = turn_id;
                vec![RunEvent::TurnStarted(TurnStartedEvent {
                    turn_id,
                    session_id,
                })]
            }

            AgentEvent::ToolCall { name, input } => {
                let id = self.next_id("tool");
                self.pending_tools
                    .push_back((id.clone(), name.clone(), input.clone()));
                vec![RunEvent::ItemStarted(ItemEvent {
                    item: RunItem {
                        id,
                        details: RunItemDetails::ToolCall(ToolCallItem {
                            tool: name,
                            input,
                            result: None,
                            ok: None,
                        }),
                    },
                })]
            }

            AgentEvent::ToolResult { ok, content } => {
                // Pair with the oldest pending tool call. A result with no
                // pending call (e.g. stray background completion) is
                // dropped — background tools report through notices.
                let Some((id, tool, input)) = self.pending_tools.pop_front() else {
                    return vec![];
                };
                self.tool_calls += 1;
                vec![RunEvent::ItemCompleted(ItemEvent {
                    item: RunItem {
                        id,
                        details: RunItemDetails::ToolCall(ToolCallItem {
                            tool,
                            input,
                            result: Some(content),
                            ok: Some(ok),
                        }),
                    },
                })]
            }

            AgentEvent::LlmResponse(text) => {
                let id = self.next_id("msg");
                vec![RunEvent::ItemCompleted(ItemEvent {
                    item: RunItem {
                        id,
                        details: RunItemDetails::AgentMessage(AgentMessageItem { text }),
                    },
                })]
            }

            AgentEvent::Thinking(text) => {
                let id = self.next_id("think");
                vec![RunEvent::ItemCompleted(ItemEvent {
                    item: RunItem {
                        id,
                        details: RunItemDetails::Reasoning(ReasoningItem { text }),
                    },
                })]
            }

            AgentEvent::UsageUpdate {
                input_tokens,
                output_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
            } => {
                // Each delta carries stream-cumulative values; merge with
                // Option-awareness (input/cache fields are Some only on
                // the final delta of a stream).
                self.turn_usage.output_tokens = output_tokens;
                if let Some(v) = input_tokens {
                    self.turn_usage.input_tokens = Some(v);
                }
                if let Some(v) = cache_creation_input_tokens {
                    self.turn_usage.cache_creation_input_tokens = Some(v);
                }
                if let Some(v) = cache_read_input_tokens {
                    self.turn_usage.cache_read_input_tokens = Some(v);
                }
                vec![]
            }

            AgentEvent::TurnCompleted {
                turn_id,
                delegation_id: None,
                status,
                ..
            } => {
                self.turns += 1;
                self.fold_turn_usage();
                match status {
                    TurnExecutionStatus::Completed => {
                        self.terminal = Some(Terminal::Completed);
                        vec![RunEvent::TurnCompleted(TurnCompletedEvent {
                            turn_id,
                            usage: self.turn_usage,
                        })]
                    }
                    TurnExecutionStatus::Failed => {
                        let message = self
                            .error_hint
                            .clone()
                            .unwrap_or_else(|| "turn failed".to_string());
                        self.terminal = Some(Terminal::Failed);
                        vec![RunEvent::TurnFailed(TurnFailedEvent { turn_id, message })]
                    }
                    TurnExecutionStatus::Interrupted => {
                        self.terminal = Some(Terminal::Cancelled);
                        vec![RunEvent::TurnFailed(TurnFailedEvent {
                            turn_id,
                            message: "turn interrupted".to_string(),
                        })]
                    }
                }
            }

            AgentEvent::Error(message) => {
                // Contract: TurnCompleted(Failed) precedes Error. Treat a
                // bare Error without a prior terminal as a defensive
                // failure; otherwise it adds the detail the turn event
                // could not carry.
                let turn_id = self.current_turn_id;
                match self.terminal.take() {
                    Some(Terminal::Failed) => {
                        self.terminal = Some(Terminal::Failed);
                        vec![RunEvent::TurnFailed(TurnFailedEvent { turn_id, message })]
                    }
                    Some(other) => {
                        self.terminal = Some(other);
                        vec![]
                    }
                    None => {
                        self.turns += 1;
                        self.fold_turn_usage();
                        self.terminal = Some(Terminal::Failed);
                        vec![RunEvent::TurnFailed(TurnFailedEvent { turn_id, message })]
                    }
                }
            }

            AgentEvent::RetryableError { message, .. } => {
                self.error_hint = Some(message.clone());
                vec![notice(NoticeKind::RetryableError, message)]
            }

            AgentEvent::Compact { event } => {
                let message = match event {
                    CompactEvent::CompactStart { .. } => "context compaction started".to_string(),
                    CompactEvent::CompactFinish { .. } => "context compaction finished".to_string(),
                };
                vec![notice(NoticeKind::Compact, message)]
            }

            AgentEvent::PlanUpdate { revision, .. } => vec![notice(
                NoticeKind::PlanUpdate,
                format!("plan updated (revision {revision})"),
            )],

            AgentEvent::ToolCallBackground { seq, name } => vec![notice(
                NoticeKind::BackgroundTool,
                format!("tool {name} moved to background (#{seq})"),
            )],

            AgentEvent::TurnAborted => {
                if self.terminal.is_none() {
                    self.terminal = Some(Terminal::Cancelled);
                }
                vec![]
            }

            // Streaming deltas, lifecycle/session bookkeeping, delegated
            // child turns, and compatibility signals carry no run events.
            _ => vec![],
        }
    }

    /// Fold the finished turn's usage into the run totals: outputs sum
    /// across turns; input/cache fields reflect the latest known value.
    fn fold_turn_usage(&mut self) {
        self.run_usage.output_tokens += self.turn_usage.output_tokens;
        if self.turn_usage.input_tokens.is_some() {
            self.run_usage.input_tokens = self.turn_usage.input_tokens;
        }
        if self.turn_usage.cache_creation_input_tokens.is_some() {
            self.run_usage.cache_creation_input_tokens =
                self.turn_usage.cache_creation_input_tokens;
        }
        if self.turn_usage.cache_read_input_tokens.is_some() {
            self.run_usage.cache_read_input_tokens = self.turn_usage.cache_read_input_tokens;
        }
    }
}

fn notice(kind: NoticeKind, message: String) -> RunEvent {
    RunEvent::Notice(NoticeEvent { kind, message })
}

#[cfg(test)]
mod gateway_runner_tests;
#[cfg(test)]
mod tests;
