//! Session — a single conversation within an Agent.
//!
//! An [`Agent`](crate::Agent) owns one or more `Session`s. Each Session has
//! independent conversation memory, lifecycle, tool execution state, and
//! cancellation token, but shares the model, tool registry, storage, and
//! system-prompt configuration with its siblings.
//!
//! Currently only one session is *active* at a time (single-active model).
//! Switching sessions pauses the current one and activates the target.
//!
//! ## Merged Memory Model
//!
//! Session directly holds its conversation data (`messages`, `summary`,
//! `ancestor_summaries`). Compaction operates in-place for model context: the
//! head messages are summarized into `ancestor_summaries`, and only the recent
//! tail is retained in `messages`. The pre-compaction transcript is archived
//! separately so the TUI can retain the user-visible record.
//!
//! ## Module layout
//!
//! The session implementation is split across submodules by concern:
//!
//! - `error` — session-state errors (compaction failures, orphaned
//!   tool_results).
//! - `shared` — `AgentShared`: stable resources shared by all sessions of one
//!   agent (model, storage, tool registry, event channel).
//! - `messages` — conversation storage: `remember` / `add_message` with
//!   tool_result adjacency repair, `render_context`, `sanitize_and_persist`.
//! - `compaction` — context compaction: head/tail selection, summarization
//!   prompt, token estimation, old-tool-output pruning.
//! - `tool_images` — re-attaching tool-returned images for vision models.
//! - `workflow` — the agent loop: `run_session`, `agent_workflow`, the
//!   streaming `request`, retries, and background wait watchers.
//!
//! This module keeps the `Session` / `SessionState` types themselves, the
//! constructors, lifecycle accessors, and persistence glue.

pub mod error;

mod compaction;
mod messages;
mod shared;
mod tool_images;
mod workflow;

pub use compaction::{DEFAULT_KEEP_TOKENS, estimate_message_tokens};
pub(crate) use shared::AgentShared;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use agentik_sdk::types::AgentEvent;
use agentik_sdk::types::messages::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::TokenBudget;
use crate::error::Result;
use crate::lifecycle::AgentLifecycle;
use crate::storage::{AgentSnapshot, AgentStorage, PersistOp};
use crate::tools::Toolset;
use agentik_types::{SessionInfo, SessionTelemetry, TurnTelemetry};

// ─────────────────────────── SessionState ───────────────────────────

/// Serializable conversation state — the snapshot payload.
///
/// Replaces the former `Memory` struct. Stores exactly what is needed to
/// reconstruct a session's conversation: the live messages, any compaction
/// summary for this session, and ancestor summaries from prior compactions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionState {
    pub messages: Vec<Message>,
    pub summary: Option<String>,
    /// Summaries from compaction ancestors, oldest first.
    /// Copied at compaction time so `render_context` is self-contained.
    #[serde(default)]
    pub ancestor_summaries: Vec<String>,
    #[serde(default)]
    pub telemetry: SessionTelemetry,
}

/// One conversation turn — the span from a resting lifecycle state
/// (`Idle` / `Error` / `Cancelled` / `Waiting`) back to a resting one,
/// however many LLM round-trips that takes. A tool call, its results, and
/// the follow-up request are all *within* one turn.
///
/// Owned by [`Session`] as `Option<Turn>`: `None` means no turn is open.
/// A turn stays open across a `Waiting` pause (background `wait_task`), so
/// a watcher resuming the session reuses it rather than starting a new one.
///
/// Deliberately does **not** own the lifecycle: that describes the agent's
/// state, is consumed far outside this crate (TUI, gateway), and outlives
/// any single turn.
struct Turn {
    /// Stable identity, correlated by callers with their delegation request.
    id: Uuid,
    /// Parent agent, when this turn was opened by a delegation.
    delegation_id: Option<Uuid>,
    /// Session telemetry as of turn start; the turn's reported usage is the
    /// delta against this.
    baseline: SessionTelemetry,
    timing: TurnTiming,
}

impl Turn {
    fn new(delegation_id: Option<Uuid>, baseline: SessionTelemetry) -> Self {
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
    /// [`Session::finish_turn`]), which also makes a double close a no-op.
    fn settle(&mut self, current: &SessionTelemetry) -> (TurnTelemetry, u64) {
        let elapsed_ms = self.timing.pause();
        let baseline = std::mem::take(&mut self.baseline);
        (TurnTelemetry::from_session_delta(current, &baseline, elapsed_ms), elapsed_ms)
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

// ─────────────────────────── Session ───────────────────────────

/// One conversation within an agent. Owns the conversation data (formerly
/// Memory), lifecycle, toolset, and cancellation token.
pub struct Session {
    pub id: Uuid,
    pub title: Option<String>,
    pub created_at: i64,
    pub last_active: i64,

    // ── Conversation content (formerly Memory/MemoryItem) ──
    pub messages: Vec<Message>,
    pub summary: Option<String>,
    /// Summaries from compaction ancestors, oldest first.
    pub ancestor_summaries: Vec<String>,
    pub telemetry: SessionTelemetry,
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
    pub(crate) active_wait_watchers: HashMap<u64, CancellationToken>,

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
    pub(crate) shared: Arc<AgentShared>,
}

impl Session {
    /// Build a Session wired to a freshly-constructed `AgentShared`. Used
    /// only by unit tests that exercise `remember` / `add_message` without a
    /// full agent bootstrap.
    #[cfg(test)]
    pub(crate) fn new_for_tests(shared: Arc<AgentShared>, _path: agentik_types::AgentPath) -> Self {
        Self::new(Uuid::new_v4(), shared)
    }

    /// Create a new empty session.
    pub(crate) fn new(id: Uuid, shared: Arc<AgentShared>) -> Self {
        let plan = Arc::new(arc_swap::ArcSwap::new(Arc::new(
            agentik_types::AgentPlan::new(),
        )));
        let mut toolset = Toolset::from_registry_with_tasks(
            shared.tool_registry.clone(),
            shared.tasks.clone(),
            shared.event_tx(),
        );
        // The same Arc must back both the Toolset context and the Session
        // field, or `update_plan` writes and `set_plan` restores would
        // diverge from what `inject_state_notices` reads.
        toolset.set_session_plan(id, Arc::clone(&plan));
        let now = chrono::Utc::now().timestamp_millis();
        let persist_tx = shared.persist_tx.get().cloned();
        Self {
            id,
            title: None,
            created_at: now,
            last_active: now,
            messages: Vec::new(),
            summary: None,
            ancestor_summaries: Vec::new(),
            telemetry: SessionTelemetry::default(),
            parent_session_id: None,
            fork_point_message_id: None,
            persist_tx,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token: CancellationToken::new(),
            active_wait_watchers: HashMap::new(),
            pending_system_prompt: None,
            plan,
            turn: None,
            shared,
        }
    }

    /// Create a session pre-loaded with conversation state (restore path).
    pub(crate) fn new_with_state(
        id: Uuid,
        shared: Arc<AgentShared>,
        state: SessionState,
        cancel_token: CancellationToken,
    ) -> Self {
        let plan = Arc::new(arc_swap::ArcSwap::new(Arc::new(
            agentik_types::AgentPlan::new(),
        )));
        let mut toolset = Toolset::from_registry_with_tasks(
            shared.tool_registry.clone(),
            shared.tasks.clone(),
            shared.event_tx(),
        );
        toolset.set_session_plan(id, Arc::clone(&plan));
        let now = chrono::Utc::now().timestamp_millis();
        let persist_tx = shared.persist_tx.get().cloned();
        Self {
            id,
            title: None,
            created_at: now,
            last_active: now,
            messages: state.messages,
            summary: state.summary,
            ancestor_summaries: state.ancestor_summaries,
            telemetry: state.telemetry,
            parent_session_id: None,
            fork_point_message_id: None,
            persist_tx,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token,
            active_wait_watchers: HashMap::new(),
            pending_system_prompt: None,
            plan,
            turn: None,
            shared,
        }
    }

    /// Fork a new session from an existing one.
    ///
    /// When `through_message_id` is supplied, the fork contains the parent's
    /// summaries and transcript only through (and including) that message;
    /// later messages remain exclusively in the parent. `None` preserves the
    /// older whole-session fork behavior.
    pub(crate) fn fork_from(
        parent: &Session,
        new_id: Uuid,
        through_message_id: Option<&str>,
        shared: Arc<AgentShared>,
    ) -> Self {
        Self::fork_from_source(parent, new_id, through_message_id, &parent.messages, shared)
    }

    /// Fork using an alternate immutable transcript prefix (used for points
    /// archived out of the parent's active model context).
    pub(crate) fn fork_from_source(
        parent: &Session,
        new_id: Uuid,
        through_message_id: Option<&str>,
        source_messages: &[Message],
        shared: Arc<AgentShared>,
    ) -> Self {
        // The fork inherits a snapshot of the parent's plan (revision
        // included); the two diverge independently from here on.
        let plan = Arc::new(arc_swap::ArcSwap::new(Arc::new(
            agentik_types::AgentPlan::clone(&parent.plan.load()),
        )));
        let mut toolset = Toolset::from_registry_with_tasks(
            shared.tool_registry.clone(),
            shared.tasks.clone(),
            shared.event_tx(),
        );
        toolset.set_session_plan(new_id, Arc::clone(&plan));
        let now = chrono::Utc::now().timestamp_millis();
        let persist_tx = shared.persist_tx.get().cloned();
        Self {
            id: new_id,
            title: Some(format!(
                "Fork of {}",
                parent.title.as_deref().unwrap_or("session")
            )),
            created_at: now,
            last_active: now,
            messages: match through_message_id {
                Some(message_id) => source_messages
                    .iter()
                    .position(|message| message.id == message_id)
                    .map(|end| source_messages[..=end].to_vec())
                    .unwrap_or_else(|| source_messages.to_vec()),
                None => source_messages.to_vec(),
            },
            summary: if std::ptr::eq(source_messages.as_ptr(), parent.messages.as_ptr()) {
                parent.summary.clone()
            } else {
                None
            },
            ancestor_summaries: if std::ptr::eq(source_messages.as_ptr(), parent.messages.as_ptr())
            {
                parent.ancestor_summaries.clone()
            } else {
                Vec::new()
            },
            telemetry: SessionTelemetry::default(),
            parent_session_id: Some(parent.id),
            fork_point_message_id: through_message_id.map(str::to_string),
            persist_tx,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token: CancellationToken::new(),
            active_wait_watchers: HashMap::new(),
            pending_system_prompt: None,
            plan,
            turn: None,
            shared,
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
            self.shared.send_event(AgentEvent::LifecycleChanged(status));
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
            agent_id: self.shared.id,
            agent_status: *self.lifecycle.status(),
            state: SessionState {
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
        if let Some(storage) = &self.shared.storage {
            let _ = storage.as_ref().create_snapshot(snapshot).await;
        }
    }

    // ── Pause / Resume ────────────────────────────────────

    /// Pause the session: lifecycle → Idle, persist snapshot, end WAL session.
    pub async fn pause(&mut self) {
        self.pause_telemetry();
        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Idle);
        self.persist_snapshot().await;
        if let Some(storage) = &self.shared.storage {
            let _ = storage.end_session(self.id).await;
        }
    }

    /// Resume the session: open WAL session (if not already open), lifecycle → Idle.
    pub async fn resume(&mut self) {
        // Start a WAL session if we don't have persist_tx wired yet.
        if self.persist_tx.is_none() {
            if let Some(tx) = self.shared.persist_tx.get() {
                self.persist_tx = Some(tx.clone());
            }
        }
        if let Some(storage) = &self.shared.storage {
            let _ = storage.start_session(self.shared.id, self.id).await;
            let _ = storage.touch_agent(self.shared.id).await;
        }
        self.set_lifecycle(agentik_types::AgentLifecycleStatus::Idle);
        self.last_active = chrono::Utc::now().timestamp_millis();
    }
}

// ─────────────────────────── SessionInfo ───────────────────────────

impl From<&Session> for SessionInfo {
    fn from(s: &Session) -> Self {
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

// ── Test support ────────────────────────────────────────────────────

/// Construct a Session with no model / no storage / no events wired.
/// Sufficient for testing `remember` / `add_message` paths in isolation.
#[cfg(test)]
pub(crate) fn make_test_session() -> Session {
    let shared = AgentShared::new_for_tests();
    Session::new_for_tests(shared, agentik_types::AgentPath::root())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_ext::AgentMessageExt;

    /// A fork inherits a snapshot of the parent's plan (revision included)
    /// and diverges independently afterwards.
    #[test]
    fn fork_clones_parent_plan_and_diverges() {
        let shared = AgentShared::new_for_tests();
        let mut parent =
            Session::new_for_tests(Arc::clone(&shared), agentik_types::AgentPath::root());

        let plan = agentik_types::AgentPlan {
            revision: 3,
            update: agentik_types::PlanUpdate {
                explanation: None,
                plan: vec![agentik_types::PlanStep {
                    step: "only step".into(),
                    status: agentik_types::StepStatus::Completed,
                }],
            },
        };
        parent.set_plan(plan.clone());
        assert_eq!(parent.plan_snapshot(), plan);

        let mut child = Session::fork_from(&parent, Uuid::new_v4(), None, Arc::clone(&shared));
        assert_eq!(
            child.plan_snapshot(),
            parent.plan_snapshot(),
            "fork must inherit the parent's plan snapshot including revision"
        );

        // The child's handle is its own: mutating it must not leak back.
        child.set_plan(agentik_types::AgentPlan::new());
        assert_eq!(parent.plan_snapshot().revision, 3);
        assert!(child.plan_snapshot().is_empty());
    }

    #[test]
    fn fork_through_message_stops_at_exact_transcript_point() {
        let shared = AgentShared::new_for_tests();
        let mut parent =
            Session::new_for_tests(Arc::clone(&shared), agentik_types::AgentPath::root());
        parent.remember(Message::user("first")).unwrap();
        parent.remember(Message::assistant_text("second")).unwrap();
        parent.remember(Message::user("later")).unwrap();

        let fork_id = parent.messages[1].id.clone();
        let child =
            Session::fork_from(&parent, Uuid::new_v4(), Some(&fork_id), Arc::clone(&shared));

        assert_eq!(
            child
                .messages
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            parent.messages[..2]
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            "fork must include the selected message and exclude everything after it"
        );
        assert_eq!(child.parent_session_id, Some(parent.id));
        assert_eq!(
            child.fork_point_message_id.as_deref(),
            Some(fork_id.as_str())
        );
        assert_eq!(
            parent.messages.len(),
            3,
            "forking must not mutate the parent"
        );
    }

    #[test]
    fn fork_from_archived_transcript_reuses_exact_prefix_without_summary_duplication() {
        let shared = AgentShared::new_for_tests();
        let mut parent =
            Session::new_for_tests(Arc::clone(&shared), agentik_types::AgentPath::root());
        parent.remember(Message::user("archived first")).unwrap();
        parent
            .remember(Message::assistant_text("archived second"))
            .unwrap();
        parent.ancestor_summaries = vec!["summarized prefix".into()];

        // Simulate compaction: the immutable transcript still contains the
        // selected point even after it has left the active model context.
        let transcript = parent.messages.clone();
        parent.messages.clear();
        let fork_id = transcript[1].id.clone();
        let child = Session::fork_from_source(
            &parent,
            Uuid::new_v4(),
            Some(&fork_id),
            &transcript,
            Arc::clone(&shared),
        );

        assert_eq!(child.messages, transcript[..2].to_vec());
        assert!(child.ancestor_summaries.is_empty());
        assert_eq!(
            child.fork_point_message_id.as_deref(),
            Some(fork_id.as_str())
        );
    }

    /// Two sessions over one shared must never observe each other's plans.
    #[test]
    fn independent_sessions_have_independent_plans() {
        let shared = AgentShared::new_for_tests();
        let mut a = Session::new_for_tests(Arc::clone(&shared), agentik_types::AgentPath::root());
        let b = Session::new_for_tests(shared, agentik_types::AgentPath::root());

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
        let mut baseline = SessionTelemetry::default();
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
        let shared = AgentShared::new_for_tests();
        let mut session = Session::new_for_tests(Arc::clone(&shared), agentik_types::AgentPath::root());

        session.begin_turn(None);
        let taken = session.turn.take().expect("begin_turn opens one");
        assert!(
            session.turn.is_none(),
            "a taken turn is gone, so the second close is a no-op"
        );

        let mut current = SessionTelemetry::default();
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
