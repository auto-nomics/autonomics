//! First-class delegation ledger: tracked request-response delegation
//! between agents, including progress reporting and lifecycle resolution
//! when a target disappears.
//!
//! [`DelegationLedger`] owns the in-memory ledger as a self-contained
//! state machine: opening entries, binding turns, terminal resolution,
//! and target-loss failure. It fires reply channels and progress buffers
//! itself, but is deliberately synchronous and storage-free — persistence
//! stays with the host loop ([`RuntimeHost::persist_delegation`]), so the
//! ledger can be unit-tested without infrastructure.

use std::collections::HashMap;

use agentik_core::tools::ProgressBuffer;
use agentik_types::{AgentPath, TurnExecutionStatus};
use tokio::sync::oneshot;

use crate::control::{DelegationSnapshot, DelegationStatus};

use super::RuntimeHost;

/// One in-flight or completed delegation.
struct HostDelegation {
    snapshot: DelegationSnapshot,
    reply_tx: Option<oneshot::Sender<String>>,
    progress: Option<ProgressBuffer>,
}

/// The authoritative in-memory ledger for tracked request-response
/// delegations between agents. Keyed by stable delegation ID, so the same
/// target can safely process multiple queued requests.
///
/// Invariants maintained here:
/// - `reply_tx` fires exactly once — taken at the terminal transition.
/// - Status only moves forward: `Pending → Running → terminal`
///   (a second `complete_turn` on a resolved entry is a no-op for the
///   reply channel).
/// - Every mutation bumps `updated_at`. Persistence is the caller's job:
///   mutating methods report whether/which entries changed.
pub(super) struct DelegationLedger {
    entries: HashMap<uuid::Uuid, HostDelegation>,
}

impl DelegationLedger {
    pub(super) fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Open a new entry. Ownership of the reply channel and the progress
    /// sink transfers to the ledger; the initial "pending" progress record
    /// is pushed here.
    pub(super) fn open(
        &mut self,
        snapshot: DelegationSnapshot,
        reply_tx: Option<oneshot::Sender<String>>,
        progress: Option<ProgressBuffer>,
    ) {
        push_delegation_progress(
            &progress,
            "delegation",
            "pending",
            &format!("target={}", snapshot.target_path),
        );
        let delegation_id = snapshot.delegation_id;
        self.entries.insert(
            delegation_id,
            HostDelegation {
                snapshot,
                reply_tx,
                progress,
            },
        );
    }

    /// Current snapshot of one entry — used by
    /// [`RuntimeHost::persist_delegation`] to re-read state after a
    /// mutation. `None` for unknown or already-removed ids.
    pub(super) fn get(&self, delegation_id: &uuid::Uuid) -> Option<DelegationSnapshot> {
        self.entries
            .get(delegation_id)
            .map(|record| record.snapshot.clone())
    }

    /// Live snapshots of all entries (for `ListDelegations`).
    pub(super) fn snapshots(&self) -> impl Iterator<Item = &DelegationSnapshot> {
        self.entries.values().map(|record| &record.snapshot)
    }

    /// Bind a started turn to its delegation: `Pending → Running` with the
    /// turn identity recorded. Returns whether the entry exists — the
    /// caller persists on `true`.
    ///
    /// The snapshot's `session_id` stays `None` for live entries: with one
    /// context per agent it is derivable from the target, and persisted
    /// rows backfill it via the `agent_turns` join when listed.
    pub(super) fn mark_running(&mut self, delegation_id: &uuid::Uuid, turn_id: uuid::Uuid) -> bool {
        let Some(record) = self.entries.get_mut(delegation_id) else {
            return false;
        };
        record.snapshot.status = DelegationStatus::Running;
        record.snapshot.turn_id = Some(turn_id);
        record.snapshot.updated_at = super::unix_epoch_ms();
        let progress = record.progress.clone();
        push_delegation_progress(
            &progress,
            "turn_started",
            "running",
            &format!("turn={turn_id}"),
        );
        true
    }

    /// Resolve every `Running` delegation aimed at a target whose turn
    /// just reached a TRUE terminal state (the trailing `Done`): final
    /// status, the full response text, and exactly one reply delivery
    /// each. Returns the resolved delegation ids — the caller persists
    /// each.
    ///
    /// Keyed by target path (the `Done` event carries no id); with
    /// delegated turns serialized per agent, at most one entry is
    /// `Running` per target.
    ///
    /// [`TurnExecutionStatus::Pause`] never reaches here — a parked turn
    /// emits no `Done` — but is treated defensively as a no-op.
    pub(super) fn resolve_target(
        &mut self,
        target: &AgentPath,
        status: TurnExecutionStatus,
        response: &str,
    ) -> Vec<uuid::Uuid> {
        if matches!(status, TurnExecutionStatus::Pause) {
            return Vec::new();
        }
        let mut resolved = Vec::new();
        for record in self.entries.values_mut() {
            if record.snapshot.target_path != *target
                || record.snapshot.status != DelegationStatus::Running
            {
                continue;
            }
            record.snapshot.status = match status {
                TurnExecutionStatus::Completed => DelegationStatus::Completed,
                TurnExecutionStatus::Interrupted => DelegationStatus::Interrupted,
                TurnExecutionStatus::Failed => DelegationStatus::Failed,
                TurnExecutionStatus::Pause => unreachable!("guarded above"),
            };
            record.snapshot.response = Some(response.to_string());
            record.snapshot.updated_at = super::unix_epoch_ms();
            let progress = record.progress.clone();
            let completion_status = record.snapshot.status;
            if let Some(reply_tx) = record.reply_tx.take() {
                let _ = reply_tx.send(response.to_string());
            }
            push_delegation_progress(
                &progress,
                "turn_completed",
                completion_status.as_str(),
                &super::status::truncate_preview(response, 240),
            );
            resolved.push(record.snapshot.delegation_id);
        }
        resolved
    }

    /// Fail every `Pending`/`Running` delegation aimed at a target that
    /// disappeared (shutdown, activation failure): each gets the failure
    /// reason as its response and one final reply delivery. Returns the
    /// affected delegation ids — the caller persists each.
    pub(super) fn fail_target(&mut self, target: &AgentPath, reason: &str) -> Vec<uuid::Uuid> {
        let mut affected = Vec::new();
        for record in self.entries.values_mut() {
            if record.snapshot.target_path != *target
                || !matches!(
                    record.snapshot.status,
                    DelegationStatus::Pending | DelegationStatus::Running
                )
            {
                continue;
            }
            record.snapshot.status = DelegationStatus::Failed;
            record.snapshot.response = Some(reason.to_string());
            record.snapshot.updated_at = super::unix_epoch_ms();
            let progress = record.progress.clone();
            if let Some(reply_tx) = record.reply_tx.take() {
                let _ = reply_tx.send(reason.to_string());
            }
            push_delegation_progress(&progress, "delegation_failed", "failed", reason);
            affected.push(record.snapshot.delegation_id);
        }
        affected
    }

    /// Push a live status line into the progress buffers of a target's
    /// running, turn-bound delegations (the `agent_event` feed consumed by
    /// `view_task_status`).
    pub(super) fn push_target_progress(&mut self, target: &AgentPath, status: &str, message: &str) {
        for record in self.entries.values_mut() {
            if record.snapshot.target_path == *target
                && record.snapshot.status == DelegationStatus::Running
                && record.snapshot.turn_id.is_some()
            {
                push_delegation_progress(&record.progress, "agent_event", status, message);
            }
        }
    }
}

impl RuntimeHost {
    /// Send a tracked delegation request through the target relay.
    pub(super) fn send_delegation_to(
        &self,
        name: &str,
        message: String,
        delegation_id: uuid::Uuid,
        origin: agentik_types::MessageOrigin,
    ) {
        if !self.enqueue_agent_message(name, message, Some(delegation_id), origin) {
            tracing::warn!(
                agent = name,
                %delegation_id,
                "delegation dropped: agent not in the live registry or relay channel closed"
            );
        }
    }

    /// Resolve in-flight delegations when a target is unregistered.
    ///
    /// Takes the registry name (a path string) — callers hold `&str` keys
    /// of the live registry, so the `AgentPath` parse happens here at the
    /// membrane. A name that fails to parse cannot have open delegations.
    pub(super) fn fail_pending_delegations(&mut self, target: &str, reason: &str) {
        let Ok(target) = AgentPath::try_from(target) else {
            tracing::warn!(target, "cannot fail delegations for non-path agent name");
            return;
        };
        for delegation_id in self.ledger.fail_target(&target, reason) {
            self.persist_delegation(delegation_id);
        }
    }
}

fn push_delegation_progress(
    progress: &Option<ProgressBuffer>,
    kind: &str,
    status: &str,
    message: &str,
) {
    let Some(progress) = progress else {
        return;
    };
    let mut record = agentik_core::tools::ProgressRecord::new(kind).status(status);
    if !message.is_empty() {
        record = record.message(message);
    }
    if let Ok(mut log) = progress.lock() {
        log.push(record);
    }
}

pub(super) fn delegation_status_from_str(value: &str) -> DelegationStatus {
    match value {
        "pending" => DelegationStatus::Pending,
        "running" => DelegationStatus::Running,
        "completed" => DelegationStatus::Completed,
        "interrupted" => DelegationStatus::Interrupted,
        _ => DelegationStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agentik_core::tools::ProgressLog;
    use tokio::sync::oneshot;

    use super::*;

    fn snapshot(id: uuid::Uuid, target: &str) -> DelegationSnapshot {
        DelegationSnapshot {
            delegation_id: id,
            caller_path: Some(AgentPath::try_from("/root/caller").unwrap()),
            target_path: AgentPath::try_from(target).unwrap(),
            task: "do the thing".into(),
            status: DelegationStatus::Pending,
            turn_id: None,
            session_id: None,
            response: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn path(value: &str) -> AgentPath {
        AgentPath::try_from(value).unwrap()
    }

    fn progress() -> (ProgressBuffer, Arc<std::sync::Mutex<ProgressLog>>) {
        // ProgressBuffer IS Arc<Mutex<ProgressLog>> — hand out a second Arc
        // so tests can read what was pushed.
        let buffer: ProgressBuffer = Arc::new(std::sync::Mutex::new(ProgressLog::new()));
        let reader = Arc::clone(&buffer);
        (buffer, reader)
    }

    #[test]
    fn open_pushes_pending_progress_and_registers_entry() {
        let mut ledger = DelegationLedger::new();
        let id = uuid::Uuid::new_v4();
        let (buffer, reader) = progress();

        ledger.open(snapshot(id, "/root/caller/worker"), None, Some(buffer));

        let snap = ledger.get(&id).expect("entry registered");
        assert_eq!(snap.status, DelegationStatus::Pending);
        let records = reader.lock().unwrap().snapshot();
        assert_eq!(records.len(), 1, "open pushes the pending record");
        assert_eq!(records[0].kind, "delegation");
        assert_eq!(records[0].status.as_deref(), Some("pending"));
    }

    #[test]
    fn mark_running_binds_turn_and_is_noop_for_unknown_id() {
        let mut ledger = DelegationLedger::new();
        let id = uuid::Uuid::new_v4();
        ledger.open(snapshot(id, "/root/caller/worker"), None, None);

        let turn_id = uuid::Uuid::new_v4();
        assert!(ledger.mark_running(&id, turn_id));

        let snap = ledger.get(&id).unwrap();
        assert_eq!(snap.status, DelegationStatus::Running);
        assert_eq!(snap.turn_id, Some(turn_id));
        assert_eq!(
            snap.session_id, None,
            "live entries resolve session on read"
        );

        let unknown = uuid::Uuid::new_v4();
        assert!(!ledger.mark_running(&unknown, turn_id));
    }

    #[test]
    fn resolve_target_delivers_reply_exactly_once() {
        let mut ledger = DelegationLedger::new();
        let id = uuid::Uuid::new_v4();
        let (reply_tx, reply_rx) = oneshot::channel();
        ledger.open(snapshot(id, "/root/caller/worker"), Some(reply_tx), None);
        ledger.mark_running(&id, uuid::Uuid::new_v4());

        let resolved = ledger.resolve_target(
            &path("/root/caller/worker"),
            TurnExecutionStatus::Completed,
            "full response",
        );
        assert_eq!(resolved, vec![id]);
        assert_eq!(reply_rx.blocking_recv().unwrap(), "full response");
        let snap = ledger.get(&id).unwrap();
        assert_eq!(snap.status, DelegationStatus::Completed);
        assert_eq!(snap.response.as_deref(), Some("full response"));

        // A second Done for the same target must not attempt a second
        // reply delivery — the entry is terminal and the oneshot is gone.
        let resolved_again = ledger.resolve_target(
            &path("/root/caller/worker"),
            TurnExecutionStatus::Completed,
            "late duplicate",
        );
        assert!(resolved_again.is_empty(), "terminal entries are skipped");
        assert_eq!(
            ledger.get(&id).unwrap().response.as_deref(),
            Some("full response"),
            "a late duplicate Done does not rewrite the resolved entry",
        );
    }

    #[test]
    fn resolve_target_is_noop_for_unknown_target() {
        let mut ledger = DelegationLedger::new();
        assert!(
            ledger
                .resolve_target(
                    &path("/root/caller/worker"),
                    TurnExecutionStatus::Completed,
                    "orphan",
                )
                .is_empty()
        );
    }

    #[test]
    fn resolve_target_skips_parked_status() {
        // Defensive: a Pause never reaches the Done arm, but if it did it
        // must not resolve anything — the entry stays Running.
        let mut ledger = DelegationLedger::new();
        let id = uuid::Uuid::new_v4();
        let (reply_tx, _reply_rx) = oneshot::channel();
        ledger.open(snapshot(id, "/root/caller/worker"), Some(reply_tx), None);
        ledger.mark_running(&id, uuid::Uuid::new_v4());

        assert!(
            ledger
                .resolve_target(
                    &path("/root/caller/worker"),
                    TurnExecutionStatus::Pause,
                    "parked",
                )
                .is_empty()
        );
        assert_eq!(ledger.get(&id).unwrap().status, DelegationStatus::Running);
    }

    #[test]
    fn fail_target_fails_only_live_entries_for_that_target() {
        let mut ledger = DelegationLedger::new();
        let live = uuid::Uuid::new_v4();
        let done = uuid::Uuid::new_v4();
        let other_target = uuid::Uuid::new_v4();
        let (live_tx, live_rx) = oneshot::channel();
        ledger.open(snapshot(live, "/root/caller/worker"), Some(live_tx), None);
        ledger.open(snapshot(done, "/root/caller/worker"), None, None);
        ledger.mark_running(&done, uuid::Uuid::new_v4());
        ledger.resolve_target(
            &path("/root/caller/worker"),
            TurnExecutionStatus::Completed,
            "already finished",
        );
        ledger.open(snapshot(other_target, "/root/caller/other"), None, None);

        let affected = ledger.fail_target(&path("/root/caller/worker"), "target shut down");

        assert_eq!(affected, vec![live], "only the live entry for the target");
        assert_eq!(
            live_rx.blocking_recv().unwrap(),
            "target shut down",
            "failure reaches the waiting caller",
        );
        assert_eq!(ledger.get(&live).unwrap().status, DelegationStatus::Failed);
        assert_eq!(
            ledger.get(&done).unwrap().status,
            DelegationStatus::Completed,
            "terminal entries are not clobbered",
        );
        assert_eq!(
            ledger.get(&other_target).unwrap().status,
            DelegationStatus::Pending,
            "other targets are untouched",
        );
    }

    #[test]
    fn push_target_progress_reaches_only_running_turn_bound_entries() {
        let mut ledger = DelegationLedger::new();
        let bound = uuid::Uuid::new_v4();
        let unbound = uuid::Uuid::new_v4();
        let (bound_buf, bound_reader) = progress();
        let (unbound_buf, unbound_reader) = progress();
        ledger.open(
            snapshot(bound, "/root/caller/worker"),
            None,
            Some(bound_buf),
        );
        ledger.open(
            snapshot(unbound, "/root/caller/worker"),
            None,
            Some(unbound_buf),
        );
        ledger.mark_running(&bound, uuid::Uuid::new_v4());

        ledger.push_target_progress(&path("/root/caller/worker"), "active", "tool_result[ok]");

        let bound_records = bound_reader.lock().unwrap().snapshot();
        assert!(
            bound_records
                .iter()
                .any(|record| record.kind == "agent_event"
                    && record.status.as_deref() == Some("active")),
            "turn-bound running entry receives the live feed",
        );
        assert!(
            !unbound_reader
                .lock()
                .unwrap()
                .snapshot()
                .iter()
                .any(|record| record.kind == "agent_event"),
            "entries without a bound turn are skipped",
        );
    }
}
