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
    /// Identity of the currently open conversation turn. A turn remains open
    /// across a `Waiting` pause (for example `wait_task`) and closes only on
    /// completion, interruption, or failure.
    pub(crate) active_turn_id: Option<Uuid>,
    pub(crate) active_delegation_id: Option<Uuid>,
    turn_baseline: Option<SessionTelemetry>,
    turn_timing: TurnTiming,

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
            persist_tx,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token: CancellationToken::new(),
            active_wait_watchers: HashMap::new(),
            pending_system_prompt: None,
            plan,
            active_turn_id: None,
            active_delegation_id: None,
            turn_baseline: None,
            turn_timing: TurnTiming::default(),
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
            persist_tx,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token,
            active_wait_watchers: HashMap::new(),
            pending_system_prompt: None,
            plan,
            active_turn_id: None,
            active_delegation_id: None,
            turn_baseline: None,
            turn_timing: TurnTiming::default(),
            shared,
        }
    }

    /// Fork a new session from an existing one, deep-cloning its state.
    pub(crate) fn fork_from(parent: &Session, new_id: Uuid, shared: Arc<AgentShared>) -> Self {
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
            messages: parent.messages.clone(),
            summary: parent.summary.clone(),
            ancestor_summaries: parent.ancestor_summaries.clone(),
            telemetry: SessionTelemetry::default(),
            persist_tx,
            lifecycle: AgentLifecycle::new(),
            toolset,
            token_budget: TokenBudget::default(),
            cancel_token: CancellationToken::new(),
            active_wait_watchers: HashMap::new(),
            pending_system_prompt: None,
            plan,
            active_turn_id: None,
            active_delegation_id: None,
            turn_baseline: None,
            turn_timing: TurnTiming::default(),
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
        self.turn_timing.pause();
    }

    fn finish_telemetry(&mut self) -> TurnTelemetry {
        let elapsed_ms = self.turn_timing.pause();
        let baseline = self.turn_baseline.take().unwrap_or_default();
        self.telemetry.record_elapsed(elapsed_ms);
        self.last_active = chrono::Utc::now().timestamp_millis();
        self.persist_telemetry();
        TurnTelemetry::from_session_delta(&self.telemetry, &baseline, elapsed_ms)
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

        let mut child = Session::fork_from(&parent, Uuid::new_v4(), Arc::clone(&shared));
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
}
