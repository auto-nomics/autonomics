//! First-class delegation ledger: tracked request-response delegation
//! between agents, including progress reporting and lifecycle resolution
//! when a target disappears.

use crate::control::{DelegationSnapshot, DelegationStatus};

use super::RuntimeHost;

/// One in-flight or completed delegation.
pub(super) struct HostDelegation {
    pub(super) snapshot: DelegationSnapshot,
    pub(super) reply_tx: Option<tokio::sync::oneshot::Sender<String>>,
    pub(super) progress: Option<agentik_core::tools::ProgressBuffer>,
}

impl RuntimeHost {
    /// Send a tracked delegation request through the target relay.
    pub(super) fn send_delegation_to(
        &self,
        name: &str,
        message: String,
        delegation_id: uuid::Uuid,
    ) {
        if !self.enqueue_agent_message(name, message, Some(delegation_id), false) {
            tracing::warn!(
                agent = name,
                %delegation_id,
                "delegation dropped: agent not in the live registry or relay channel closed"
            );
        }
    }

    /// Resolve in-flight delegations when a target is unregistered.
    pub(super) fn fail_pending_delegations(&mut self, target: &str, reason: &str) {
        for record in self.delegations.values_mut() {
            if record.snapshot.target_path != target
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
            push_delegation_progress(&progress, "delegation_failed", "failed", reason);
            if let Some(reply_tx) = record.reply_tx.take() {
                let _ = reply_tx.send(reason.to_string());
            }
        }
        let affected: Vec<uuid::Uuid> = self
            .delegations
            .values()
            .filter(|record| record.snapshot.target_path == target)
            .map(|record| record.snapshot.delegation_id)
            .collect();
        for id in affected {
            self.persist_delegation(id);
        }
    }
}

pub(super) fn push_delegation_progress(
    progress: &Option<agentik_core::tools::ProgressBuffer>,
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
