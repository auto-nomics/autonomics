//! Context — the agent's single persistent working context.
//!
//! An [`Agent`](crate::Agent) owns exactly one `Context` (created on its
//! first message, restored from the bound `agent_contexts` record on
//! restart). Independent work creates another agent. The context owns the
//! conversation memory, lifecycle, tool execution state, and cancellation
//! token; the model, tool registry, storage, and system-prompt
//! configuration are shared agent-level resources on [`AgentCore`].
//!
//! ## Merged Memory Model
//!
//! Context directly holds its conversation data (`messages`, `summary`,
//! `ancestor_summaries`). Compaction operates in-place for model context: the
//! head messages are summarized into `ancestor_summaries`, and only the recent
//! tail is retained in `messages`. The pre-compaction transcript is archived
//! separately so the TUI can retain the user-visible record.
//!
//! ## Module layout
//!
//! The context implementation is split across submodules by concern:
//!
//! - `error` — context-state errors (compaction failures, orphaned
//!   tool_results).
//! - `core` — `AgentCore`: stable agent-level resources (model, storage,
//!   tool registry, event channel).
//! - `messages` — conversation storage: `remember` / `add_message` with
//!   tool_result adjacency repair, `render_context`, `sanitize_and_persist`.
//! - `compaction` — context compaction: head/tail selection, summarization
//!   prompt, token estimation, old-tool-output pruning.
//! - `tool_images` — re-attaching tool-returned images for vision models.
//! - `workflow` — the agent loop: `run_context`, `agent_workflow`, the
//!   streaming `request`, retries, and background wait watchers.
//!
//! This module keeps the `Context` / `ContextState` types themselves, the
//! constructors, lifecycle accessors, and persistence glue. Durable
//! history records keep the storage-layer `session` naming (sessions
//! table, `PersistOp::StartSession`, ...) — those name the archived
//! record, not this live object.

pub mod error;

mod compaction;
mod core;
mod messages;
mod tool_images;
mod workflow;

pub use compaction::{DEFAULT_KEEP_TOKENS, estimate_message_tokens};
pub(crate) use core::AgentCore;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use agentik_sdk::types::AgentEvent;
use agentik_sdk::types::messages::{ContentBlock, Message, Role};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::TokenBudget;
use crate::error::Result;
use crate::lifecycle::AgentLifecycle;
use crate::storage::{AgentSnapshot, AgentStorage, PersistOp};
use crate::tools::Toolset;
use agentik_types::{ContextInfo, ContextTelemetry, TurnTelemetry};

// ─────────────────────────── ContextState ───────────────────────────

/// Serializable conversation state — the snapshot payload.
///
/// Replaces the former `Memory` struct. Stores exactly what is needed to
/// reconstruct a session's conversation: the live messages, any compaction
/// summary for this session, and ancestor summaries from prior compactions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextState {
    pub messages: Vec<Message>,
    pub summary: Option<String>,
    /// Summaries from compaction ancestors, oldest first.
    /// Copied at compaction time so `render_context` is self-contained.
    #[serde(default)]
    pub ancestor_summaries: Vec<String>,
    #[serde(default)]
    pub telemetry: ContextTelemetry,
}

/// One conversation turn — the span from a resting lifecycle state
/// (`Idle` / `Error` / `Cancelled` / `Waiting`) back to a resting one,
/// however many LLM round-trips that takes. A tool call, its results, and
/// the follow-up request are all *within* one turn.
///
/// Owned by [`Context`] as `Option<Turn>`: `None` means no turn is open.
/// A `Waiting` pause (background `wait_task`) CLOSES the turn with
/// [`TurnExecutionStatus::Pause`] — no `Done` is emitted — and the
/// watcher's wake message opens a new turn that re-adopts the paused
/// turn's `delegation_id` (see [`Context::resume_delegation_id`]).
///
/// Deliberately does **not** own the lifecycle: that describes the agent's
/// state, is consumed far outside this crate (TUI, gateway), and outlives
/// any single turn.
struct Turn {
    /// Stable identity, correlated by callers with their delegation request.
    id: Uuid,
    /// Parent agent, when this turn was opened by a delegation.
    delegation_id: Option<Uuid>,
    /// Context telemetry as of turn start; the turn's reported usage is the
    /// delta against this.
    baseline: ContextTelemetry,
    timing: TurnTiming,
}

impl Turn {
    fn new(delegation_id: Option<Uuid>, baseline: ContextTelemetry) -> Self {
        Self {
            id: Uuid::new_v4(),
            delegation_id,
            baseline,
            timing: TurnTiming::default(),
        }
    }

    /// Settle the turn's wall-clock accounting and compute its telemetry
    /// delta against `current`.
    ///
    /// Consumes the baseline, so a turn must be settled **once**. Note a
    /// second settle would not return zero — it would re-report the full
    /// totals against the now-empty baseline. Callers therefore take the
    /// turn out of the session before settling it (see
    /// [`Context::finish_turn`]), which also makes a double close a no-op.
    fn settle(&mut self, current: &ContextTelemetry) -> (TurnTelemetry, u64) {
        let elapsed_ms = self.timing.pause();
        let baseline = std::mem::take(&mut self.baseline);
        (
            TurnTelemetry::from_session_delta(current, &baseline, elapsed_ms),
            elapsed_ms,
        )
    }
}

#[derive(Default)]
struct TurnTiming {
    active_since: Option<Instant>,
    active_ms: u64,
}

impl TurnTiming {
    fn start(&mut self) {
        self.active_since.get_or_insert_with(Instant::now);
    }

    fn pause(&mut self) -> u64 {
        if let Some(started) = self.active_since.take() {
            self.active_ms = self
                .active_ms
                .saturating_add(started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
        }
        self.active_ms
    }

    fn is_active(&self) -> bool {
        self.active_since.is_some()
    }
}

// ─────────────────────────── Context ───────────────────────────

/// One conversation within an agent. Owns the conversation data (formerly
/// Memory), lifecycle, toolset, and cancellation token.
pub struct Context {
    pub id: Uuid,
    pub title: Option<String>,
    pub created_at: i64,
    pub last_active: i64,

    // ── Conversation content (formerly Memory/MemoryItem) ──
    pub messages: Vec<Message>,
    pub summary: Option<String>,
    /// Summaries from compaction ancestors, oldest first.
    pub ancestor_summaries: Vec<String>,
    pub telemetry: ContextTelemetry,
    pub parent_session_id: Option<Uuid>,
    pub fork_point_message_id: Option<String>,

    // ── Runtime state ──
    pub persist_tx: Option<UnboundedSender<PersistOp>>,
    pub lifecycle: AgentLifecycle,
    pub toolset: Toolset,
    pub token_budget: TokenBudget,
    pub cancel_token: CancellationToken,

    /// Active `wait_task` watchers keyed by task seq. Each value is the
    /// child cancel token for the spawned watcher. Before spawning a new
    /// watcher for the same task, the old one is cancelled to prevent
    /// duplicate message injection.
    pub(crate) active_wait_watchers:
        HashMap<u64, (CancellationToken, std::sync::Arc<std::sync::atomic::AtomicBool>)>,
    /// Delegation id stashed when a turn closed with
    /// [`TurnExecutionStatus::Pause`]; the next `begin_turn` re-adopts it so
    /// the wake turn stays attributable to the pending delegation.
    pub(crate) resume_delegation_id: Option<Uuid>,

    /// System prompt set by `build_context` and consumed by `request`.
    /// Kept separate from `messages` so the sanitizer never sees it
    /// and tool_use/tool_result indices are not shifted by coalescing.
    pub(crate) pending_system_prompt: Option<String>,
    /// The session's persistent task plan — a first-class citizen that lives
    /// as long as this conversation does. Updated via the `update_plan` tool
    /// (which reaches this same `ArcSwap` through its `ToolContext`) and
    /// injected into the message stream by `inject_state_notices`.
    /// The `ArcSwap` identity is stable for the session's lifetime; use
    /// [`set_plan`](Self::set_plan) to replace contents in place.
    pub(crate) plan: Arc<arc_swap::ArcSwap<agentik_types::AgentPlan>>,
    /// The currently open conversation turn, if any. See [`Turn`].
    turn: Option<Turn>,

    /// Back-reference to shared agent resources.
    pub(crate) core: Arc<AgentCore>,
}

impl Context {
    /// Build a Context wired to a freshly-constructed `AgentCore`. Used
    /// only by unit tests that exercise `remember` / `add_message` without a
    /// full agent bootstrap.
    #[cfg(test)]
    pub(crate) fn new_for_tests(shared: Arc<AgentCore>, _path: agentik_types::AgentPath) -> Self {
        Self::new(Uuid::new_v4(), shared)
    }

    /// Create a new empty session.
    pub(crate) fn new(id: Uuid, core: Arc<AgentCore>) -> Self {
        let plan = Arc::new(arc_swap::ArcSwap::new(Arc::new(
            agentik_types::AgentPlan::new(),
        )));
        let mut toolset = Toolset::from_registry_with_tasks(
            core.tool_registry.clone(),
            core.tasks.clone(),
            core.event_tx(),
        );
        // The same Arc must back both the Toolset context and the Context
        // field, or `update_plan` writes and `set_plan` restores would
        // diverge from what `inject_state_notices` reads.
        toolset.set_session_plan(id, Arc::clone(&plan));
        let now = chrono::Utc::now().timestamp_millis();
        let persist_tx = core.persist_tx.get().cloned();
        Self {
            id,
            title: None,
            created_at: now,
            last_active: now,
            messages: Vec::new(),
            summary: None,
            ancestor_summaries: Vec::new(),
            telemetry: ContextTelemetry::default(),
            parent_session_id: None,
            fork_point_message_id: None,
            persist_tx,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token: CancellationToken::new(),
            active_wait_watchers: HashMap::new(),
            resume_delegation_id: None,
            pending_system_prompt: None,
            plan,
            turn: None,
            core,
        }
    }

    // ── Accessors ──────────────────────────────────────────

    pub fn lifecycle_status(&self) -> agentik_types::AgentLifecycleStatus {
        *self.lifecycle.status()
    }

    pub fn is_running(&self) -> bool {
        self.lifecycle.is_running()
    }

    /// Transition the lifecycle to `status`, emitting
    /// `AgentEvent::LifecycleChanged` if the status actually changed.
    pub(crate) fn set_lifecycle(&mut self, status: agentik_types::AgentLifecycleStatus) {
        if self.lifecycle.set_status(status) {
            self.core.send_event(AgentEvent::LifecycleChanged(status));
        }
    }

    pub fn set_cancel_token(&mut self, token: CancellationToken) {
        self.cancel_token = token;
    }

    // ── Plan ──────────────────────────────────────────────

    /// Replace the plan contents in place (keeps the `ArcSwap` identity the
    /// Toolset context captured, so the `update_plan` tool observes this
    /// immediately). Used by the restore path when a session rehydrates its
    /// persisted plan.
    pub(crate) fn set_plan(&mut self, plan: agentik_types::AgentPlan) {
        self.plan.store(Arc::new(plan));
    }

    /// Load a snapshot of the session's current plan.
    pub(crate) fn plan_snapshot(&self) -> agentik_types::AgentPlan {
        agentik_types::AgentPlan::clone(&self.plan.load())
    }

    // ── Telemetry ─────────────────────────────────────────

    fn persist_telemetry(&self) {
        if let Some(tx) = &self.persist_tx {
            let _ = tx.send(PersistOp::UpdateSessionTelemetry {
                session_id: self.id,
                telemetry: self.telemetry,
            });
        }
    }

    fn pause_telemetry(&mut self) {
        if let Some(turn) = self.turn.as_mut() {
            turn.timing.pause();
        }
    }

    /// Settle `turn`'s telemetry into the session totals and return the
    /// turn's delta.
    fn finish_telemetry_for(&mut self, turn: &mut Turn) -> TurnTelemetry {
        let (telemetry, elapsed_ms) = turn.settle(&self.telemetry);
        self.telemetry.record_elapsed(elapsed_ms);
        self.last_active = chrono::Utc::now().timestamp_millis();
        self.persist_telemetry();
        telemetry
    }

    // ── Snapshot / persistence ────────────────────────────

    pub fn snapshot(&self) -> AgentSnapshot {
        AgentSnapshot {
            snapshot_id: Uuid::new_v4(),
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
            agent_id: self.core.id,
            agent_status: *self.lifecycle.status(),
            state: ContextState {
                messages: self.messages.clone(),
                summary: self.summary.clone(),
                ancestor_summaries: self.ancestor_summaries.clone(),
                telemetry: self.telemetry,
            },
            session_id: Some(self.id),
        }
    }

    pub async fn persist_snapshot(&self) {
        let snapshot = self.snapshot();
        if let Some(storage) = &self.core.storage {
            let _ = storage.as_ref().create_snapshot(snapshot).await;
        }
    }

    // ── Pause / Resume ────────────────────────────────────

    /// Pause the session: lifecycle → Idle, persist snapshot, end WAL session.
    pub async fn pause(&mut self) {
        self.pause_telemetry();
        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Idle);
        self.persist_snapshot().await;
        if let Some(storage) = &self.core.storage {
            let _ = storage.end_session(self.id).await;
        }
    }

    /// Resume the session: open WAL session (if not already open), lifecycle → Idle.
    pub async fn resume(&mut self) {
        // Start a WAL session if we don't have persist_tx wired yet.
        if self.persist_tx.is_none() {
            if let Some(tx) = self.core.persist_tx.get() {
                self.persist_tx = Some(tx.clone());
            }
        }
        if let Some(storage) = &self.core.storage {
            let _ = storage.start_session(self.core.id, self.id).await;
            let _ = storage.touch_agent(self.core.id).await;
        }
        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Idle);
        self.last_active = chrono::Utc::now().timestamp_millis();
    }
}

// ─────────────────────────── ContextInfo ───────────────────────────

impl From<&Context> for ContextInfo {
    fn from(s: &Context) -> Self {
        Self {
            id: s.id,
            title: s.title.clone(),
            created_at: s.created_at,
            last_active: s.last_active,
            telemetry: s.telemetry,
            parent_session_id: s.parent_session_id,
            fork_point_message_id: s.fork_point_message_id.clone(),
        }
    }
}

// ── Dynamic context injection ─────────────────────────
// (merged from the former top-level `context.rs` module)

/// Optional callback for dynamic context injection.
///
/// If provided, the agent calls `poll()` at each loop boundary (before
/// each LLM request). When it returns `Some(text)`, the text is injected
/// as a user message into memory. Return `None` to skip.
///
/// This replaces the previous `AgentContext` reactive store with a much
/// simpler callback interface.
#[async_trait]
pub trait ContextProvider: Send + Sync {
    async fn poll(&self) -> Option<String>;
}

// ── Test support ────────────────────────────────────────────────────

/// Construct a Context with no model / no storage / no events wired.
/// Sufficient for testing `remember` / `add_message` paths in isolation.
#[cfg(test)]
pub(crate) fn make_test_context() -> Context {
    let shared = AgentCore::new_for_tests();
    Context::new_for_tests(shared, agentik_types::AgentPath::root())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_sdk::AgentMessageExt;

    /// Two sessions over one shared must never observe each other's plans.
    #[test]
    fn independent_contexts_have_independent_plans() {
        let shared = AgentCore::new_for_tests();
        let mut a = Context::new_for_tests(Arc::clone(&shared), agentik_types::AgentPath::root());
        let b = Context::new_for_tests(shared, agentik_types::AgentPath::root());

        a.set_plan(agentik_types::AgentPlan {
            revision: 1,
            update: agentik_types::PlanUpdate::default(),
        });
        assert_eq!(a.plan_snapshot().revision, 1);
        assert!(
            b.plan_snapshot().is_empty(),
            "session B must not see session A's plan"
        );
    }

    // ── Turn ────────────────────────────────────────────

    /// A turn's reported usage is the delta against the baseline captured
    /// when it opened — not the session's lifetime totals.
    #[test]
    fn turn_settles_usage_as_a_delta_from_its_baseline() {
        let mut baseline = ContextTelemetry::default();
        baseline.record_usage(&agentik_types::Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            ..Default::default()
        });

        let mut current = baseline;
        current.record_usage(&agentik_types::Usage {
            input_tokens: 400,
            output_tokens: 50,
            ..Default::default()
        });

        let mut turn = Turn::new(None, baseline);
        let (telemetry, _) = turn.settle(&current);

        assert_eq!(telemetry.input_tokens, 400, "only this turn's usage");
        assert_eq!(telemetry.output_tokens, 50);
    }

    /// `finish_turn` takes the turn out of the session *before* settling
    /// it, so a second close finds `session.turn == None` and does
    /// nothing. This pins that guard: settling the same `Turn` twice would
    /// report the full totals again (the baseline was consumed), so the
    /// take-before-settle ordering is what prevents double counting.
    #[test]
    fn a_taken_turn_cannot_be_settled_twice() {
        let shared = AgentCore::new_for_tests();
        let mut session =
            Context::new_for_tests(Arc::clone(&shared), agentik_types::AgentPath::root());

        session.begin_turn(None);
        let taken = session.turn.take().expect("begin_turn opens one");
        assert!(
            session.turn.is_none(),
            "a taken turn is gone, so the second close is a no-op"
        );

        let mut current = ContextTelemetry::default();
        current.record_usage(&agentik_types::Usage {
            input_tokens: 900,
            output_tokens: 90,
            ..Default::default()
        });
        let mut taken = taken;
        let (first, _) = taken.settle(&current);
        assert_eq!(first.input_tokens, 900);

        // Settling again re-reports everything — the baseline is gone.
        let (second, _) = taken.settle(&current);
        assert_eq!(
            second.input_tokens, 900,
            "why finish_turn must take the turn before settling"
        );
    }

    /// Timing accumulates across pause/resume: a turn that pauses on a
    /// background `wait_task` and later resumes keeps counting total
    /// elapsed wall-clock, not just the final active stretch.
    #[test]
    fn turn_timing_accumulates_across_pause_and_resume() {
        let mut timing = TurnTiming::default();
        timing.start();
        assert!(timing.is_active());

        let first = timing.pause();
        assert!(!timing.is_active(), "pause clears the running clock");

        timing.start();
        let second = timing.pause();

        assert_eq!(
            timing.pause(),
            first.saturating_add(second),
            "total elapsed spans both active stretches"
        );
    }
}
