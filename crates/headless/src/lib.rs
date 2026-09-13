//! Headless (non-interactive) execution of agentik agents.
//!
//! This crate is the UI-free sibling of the TUI: both entry points sit on
//! the same library layer (`runtime::RuntimeHost`, `agentik-core`) and
//! never share UI objects. It provides
//!
//! - [`event`]: the stable external run-event contract (`RunEvent`),
//!   emitted as JSONL on stdout in `--json` mode;
//! - [`processor`]: pluggable output rendering (human-readable vs JSONL)
//!   over the same translated event stream;
//! - [`run_task`]: the run loop — open a [`RuntimeHost`], spawn one agent
//!   from a profile, deliver a single prompt, translate the agent's event
//!   stream into [`RunEvent`]s until the turn reaches a terminal state,
//!   then shut down cleanly.
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
pub mod manifest;
pub mod processor;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentik_core::AgentProfile;
use agentik_sdk::model::Model;
use agentik_types::{AgentEvent, CompactEvent, TurnExecutionStatus};
use arc_swap::ArcSwapOption;
use processor::OutputProcessor;
use runtime::{HostEvent, RuntimeConfig, RuntimeHost};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use event::{
    AgentMessageItem, ItemEvent, NoticeEvent, NoticeKind, ReasoningItem, RunEndedEvent, RunEvent,
    RunItem, RunItemDetails, RunStartedEvent, RunStatus, ToolCallItem, TurnCompletedEvent,
    TurnFailedEvent, TurnStartedEvent, Usage,
};

/// How long `recv_any` may block before the loop pumps the host command
/// channel again, so agent tools issuing [`runtime::HostControl`] calls
/// never starve.
const COMMAND_PUMP_INTERVAL: Duration = Duration::from_millis(50);

/// Upper bound on the final agent shutdown wait before the host is
/// dropped regardless.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

/// Everything `run_task` needs to execute one prompt.
#[derive(Clone)]
pub struct RunTaskConfig {
    /// The prompt delivered as the single user message of the run.
    pub prompt: String,
    /// Profile path to spawn from; `None` picks the first stored profile.
    pub profile: Option<String>,
    /// Agent name segment, mounted directly under the root path.
    pub agent_name: String,
    /// The model the agent runs with. `None` leaves the host without a
    /// global model — spawning then fails, so callers normally resolve
    /// one via `runtime::model_bootstrap` (or inject a mock in tests).
    pub model: Option<Model>,
    /// Model name reported in `run.started`. `Model` exposes no name
    /// accessor, so the caller passes the name it resolved (or a hint
    /// like `"mock"` in tests).
    pub model_name: Option<String>,
    /// Resume this session instead of continuing the agent's active
    /// one. The agent must already know it (same agent path, persisted
    /// in the state dir); scripts obtain the id from `turn.started`.
    pub session: Option<Uuid>,
    /// Wall-clock budget for the whole run. When it elapses the run is
    /// abandoned as cancelled (exit code 2 territory): a
    /// `turn.failed` event with the timeout message is emitted, then
    /// `run.ended{status: cancelled}`, and the agent is shut down.
    pub timeout: Option<Duration>,
    /// Fully resolved runtime configuration (state dir, feature flags, …).
    pub runtime_config: RuntimeConfig,
}

/// Keeps the ephemeral scratch directories alive for the duration of a
/// run; dropping it removes them.
pub struct EphemeralState {
    _dir: tempfile::TempDir,
}

impl RunTaskConfig {
    /// A run whose data/state directories live under a fresh temp dir,
    /// removed when the returned [`EphemeralState`] drops. The model is
    /// still resolved by the caller (normally from the real app DB) and
    /// set afterwards — ephemerality scopes *conversation state*, not
    /// credentials.
    pub fn ephemeral(prompt: impl Into<String>) -> (Self, EphemeralState) {
        let dir = tempfile::tempdir().expect("create ephemeral dir");
        let mut runtime_config = RuntimeConfig::default();
        runtime_config.data_dir = dir.path().join("data");
        runtime_config.state_dir = dir.path().join("state");
        runtime_config.app_db_path = dir.path().join("app.db");
        let config = Self::new(prompt, runtime_config);
        (config, EphemeralState { _dir: dir })
    }

    pub fn new(prompt: impl Into<String>, runtime_config: RuntimeConfig) -> Self {
        Self {
            prompt: prompt.into(),
            profile: None,
            agent_name: "headless".to_string(),
            model: None,
            model_name: None,
            session: None,
            timeout: None,
            runtime_config,
        }
    }
}

/// Terminal outcome of a completed `run_task` invocation.
#[derive(Debug, Clone)]
pub struct RunSummary {
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
    #[error("failed to open runtime host: {0}")]
    HostOpen(#[from] runtime::Error),
    #[error("no agent profile available (requested: {requested:?})")]
    NoProfile { requested: Option<String> },
    #[error("agent spawn failed: {message}")]
    Spawn { message: String },
    #[error("prompt delivery failed: {message}")]
    Send { message: String },
}

/// Run one prompt headlessly, streaming translated events into `processor`.
///
/// Owns the full lifecycle: host open → profile bootstrap → agent spawn →
/// prompt delivery → event loop → clean shutdown. See the crate docs for
/// termination semantics and the stdout discipline contract.
pub async fn run_task<P: OutputProcessor>(
    config: RunTaskConfig,
    processor: &mut P,
) -> Result<RunSummary, RunError> {
    let started = Instant::now();
    let mut host = RuntimeHost::open(&config.runtime_config).await?;

    // ── Profile bootstrap (same sequence as the TUI startup) ────────
    let profile_storage = host.infra().profile_storage.clone();
    let _ = profile_storage.seed_defaults_if_empty().await;
    let profiles = profile_storage.list_profiles().await.unwrap_or_default();
    let profile = pick_profile(&profiles, config.profile.as_deref()).ok_or(
        RunError::NoProfile {
            requested: config.profile.clone(),
        },
    )?;
    host.set_profiles(profiles);
    host.set_model(Arc::new(ArcSwapOption::from_pointee(config.model)));

    // ── Spawn ────────────────────────────────────────────────────────
    // The control call needs the host command loop pumping while its
    // oneshot reply is pending, so it runs as a task and this side keeps
    // processing commands (same pattern as the TUI and the host tests).
    let control = host.control();
    let profile_for_spawn = profile.clone();
    let agent_name = config.agent_name.clone();
    let mut spawn = tokio::spawn(async move {
        control
            .spawn_with_profile(&agent_name, &agentik_types::AgentPath::root(), profile_for_spawn, None)
            .await
    });
    let agent_path = loop {
        tokio::select! {
            result = &mut spawn => {
                break result.expect("spawn task panicked").map_err(|message| RunError::Spawn { message })?;
            }
            _ = host.recv_and_process_command() => {}
        }
    };

    // The registration event (carrying the agent id) is queued on the
    // host-event channel during command processing; recover it, best
    // effort with a deadline.
    let agent_id = match tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = host.recv_event().await {
            if let HostEvent::AgentRegistered { info, .. } = event {
                return info.agent_id.unwrap_or_default();
            }
        }
        Uuid::nil()
    })
    .await
    {
        Ok(id) => id,
        Err(_) => Uuid::nil(),
    };

    // ── Optional session resume ──────────────────────────────────────
    // Applied before the prompt so the message lands in the resumed
    // session. Fire-and-forget, so dispatch it synchronously (same
    // lesson as the timeout cancel).
    if let Some(session) = config.session {
        host.control().switch_session(&agent_path, session);
        host.try_process_commands();
    }

    // ── Prompt delivery ──────────────────────────────────────────────
    let control = host.control();
    let path_for_send = agent_path.clone();
    let prompt = config.prompt.clone();
    let mut send = tokio::spawn(async move {
        control
            .send_message(&path_for_send, prompt)
            .await
            .ok_or_else(|| "host command channel closed".to_string())
            .and_then(|r| r)
    });
    loop {
        tokio::select! {
            result = &mut send => {
                result.expect("send task panicked").map_err(|message| RunError::Send { message })?;
                break;
            }
            _ = host.recv_and_process_command() => {}
        }
    }

    // ── Event loop ───────────────────────────────────────────────────
    processor.process(&RunEvent::RunStarted(RunStartedEvent {
        agent_id,
        // Bound at the first turn.started; nil until then (single-turn
        // runs have exactly one session, created by the agent itself).
        session_id: Uuid::nil(),
        profile: profile.path.clone(),
        model: config.model_name.clone(),
    }));

    let mut translation = TranslationState::default();
    let deadline = config.timeout.map(|budget| started + budget);
    let terminal = loop {
        if let Some(deadline) = deadline
            && Instant::now() >= deadline
        {
            let budget = config.timeout.expect("deadline implies timeout");
            // Cooperative cancel first: aborts the in-flight turn —
            // interrupts retry backoff and running tools — so the
            // shutdown below doesn't wait on them. `cancel_agent` is
            // fire-and-forget, so drain the host command queue here to
            // actually dispatch it.
            host.control().cancel_agent(&agent_path);
            host.try_process_commands();
            translation.terminal = Some(Terminal::Cancelled);
            processor.process(&RunEvent::TurnFailed(TurnFailedEvent {
                turn_id: translation.current_turn_id,
                message: format!("run timed out after {:.1}s", budget.as_secs_f64()),
            }));
            break Terminal::Cancelled;
        }
        host.try_process_commands();
        match tokio::time::timeout(COMMAND_PUMP_INTERVAL, host.recv_any()).await {
            Ok(Some((name, event))) => {
                for run_event in translation.translate(&name, event) {
                    processor.process(&run_event);
                }
                if translation.terminal.is_some() {
                    // Drain events already queued behind the terminal one
                    // (e.g. the compatibility Done/Error) without blocking.
                    while let Some((name, event)) = host.try_recv_any() {
                        for run_event in translation.translate(&name, event) {
                            processor.process(&run_event);
                        }
                    }
                    break translation.terminal.take().expect("checked above");
                }
            }
            // Every agent task has ended and its event channel closed.
            Ok(None) => {
                break translation
                    .terminal
                    .take()
                    .unwrap_or(Terminal::Failed);
            }
            // Timeout: loop back around to pump host commands.
            Err(_) => continue,
        }
    };

    let wall_time_secs = started.elapsed().as_secs_f64();
    let status = match &terminal {
        Terminal::Completed => RunStatus::Completed,
        Terminal::Failed => RunStatus::Failed,
        Terminal::Cancelled => RunStatus::Cancelled,
    };
    let usage = (translation.turns > 0).then_some(translation.run_usage);
    let turns = translation.turns;
    let tool_calls = translation.tool_calls;
    processor.process(&RunEvent::RunEnded(RunEndedEvent {
        status,
        wall_time_secs,
        usage,
        turns,
        tool_calls,
    }));
    processor.finish();

    // Belt and braces: the cooperative cancel (timeout path) and the
    // terminal turn state (normal path) should make shutdown prompt, but
    // a wedged tool must not hang the process — drop the host instead.
    if tokio::time::timeout(SHUTDOWN_GRACE, host.shutdown_all_agents_and_wait())
        .await
        .is_err()
    {
        tracing::warn!("agents did not shut down within grace; dropping host");
    }

    Ok(RunSummary {
        profile: profile.path.clone(),
        outcome: match terminal {
            Terminal::Completed => processor::Outcome::Completed,
            Terminal::Failed => processor::Outcome::Failed,
            Terminal::Cancelled => processor::Outcome::Cancelled,
        },
        agent_path,
        last_message: processor.last_message().map(str::to_owned),
        wall_time_secs,
        usage,
        turns,
        tool_calls,
    })
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
                self.pending_tools.push_back((id.clone(), name.clone(), input.clone()));
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
                        vec![RunEvent::TurnFailed(TurnFailedEvent {
                            turn_id,
                            message,
                        })]
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
                        vec![RunEvent::TurnFailed(TurnFailedEvent {
                            turn_id,
                            message,
                        })]
                    }
                    Some(other) => {
                        self.terminal = Some(other);
                        vec![]
                    }
                    None => {
                        self.turns += 1;
                        self.fold_turn_usage();
                        self.terminal = Some(Terminal::Failed);
                        vec![RunEvent::TurnFailed(TurnFailedEvent {
                            turn_id,
                            message,
                        })]
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
            self.run_usage.cache_creation_input_tokens = self.turn_usage.cache_creation_input_tokens;
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
mod tests;
