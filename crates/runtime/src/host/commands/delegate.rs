//! Inter-agent delegation commands: `Delegate` (request-response, gated
//! to direct children, with virtual-actor activation of dormant targets)
//! and `ListDelegations` (live ledger + persisted history merge).

use agentik_core::tools::ProgressBuffer;

use crate::control::{DelegationSnapshot, DelegationStatus};
use super::super::delegation::delegation_status_from_str;
use super::super::events::PendingDelivery;
use super::super::unix_epoch_ms;
use super::super::RuntimeHost;

impl RuntimeHost {
    /// `HostCommand::Delegate` — open a ledger entry and deliver the task
    /// to the resolved direct child (activating it first if dormant).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_delegate(
        &mut self,
        to: String,
        message: String,
        caller_path: Option<String>,
        delegation_id: uuid::Uuid,
        progress: Option<ProgressBuffer>,
        reply_tx: tokio::sync::oneshot::Sender<String>,
    ) {
        let Some(caller_path) = caller_path else {
            let _ = reply_tx.send(
                "Delegation failed: the calling agent path is unavailable.".to_string(),
            );
            return;
        };
        // Resolve the target from the caller's viewpoint: full paths
        // match exactly, short names prefer the caller's own child.
        // Unresolvable and non-child targets return the SAME message
        // so callers cannot probe the registry for foreign agents.
        let resolved = match agentik_types::AgentPath::try_from(caller_path.as_str())
            .ok()
            .and_then(|caller| self.resolve_agent_scoped(&caller, &to))
        {
            Some(path) => path,
            None => {
                let _ = reply_tx.send(format!(
                    "Error: agent '{to}' is not one of your direct children. \
                     Delegation reaches only agents you spawned; use list_agents \
                     to see them, or spawn_agent to create one first."
                ));
                return;
            }
        };
        let target_path = match self.validate_delegation_paths(&caller_path, &resolved) {
            Ok(path) => path,
            Err(error) => {
                let _ = reply_tx.send(error);
                return;
            }
        };
        let now = unix_epoch_ms();
        let origin = agentik_types::MessageOrigin::agent(&caller_path);
        self.ledger.open(
            DelegationSnapshot {
                delegation_id,
                // The validator already proved the caller parses;
                // the ledger stores the typed path.
                caller_path: agentik_types::AgentPath::try_from(caller_path.as_str()).ok(),
                target_path,
                task: message.clone(),
                status: DelegationStatus::Pending,
                turn_id: None,
                session_id: None,
                response: None,
                created_at: now,
                updated_at: now,
            },
            Some(reply_tx),
            progress,
        );
        self.persist_delegation(delegation_id);
        if self.agents.contains_key(&resolved) {
            self.send_delegation_to(&resolved, message, delegation_id, origin);
        } else {
            // Dormant target: activate it, then deliver on
            // registration. The delegation ledger owns the reply
            // channel; activation failure fails the ledger entry.
            self.enqueue_after_activation(
                resolved,
                PendingDelivery {
                    text: message,
                    delegation_id: Some(delegation_id),
                    origin,
                    reply_tx: None,
                },
            );
        }
    }

    /// `HostCommand::ListDelegations` — merge the live ledger with
    /// persisted history (deduped by delegation id, newest first).
    pub(super) fn handle_list_delegations(
        &mut self,
        caller_path: Option<String>,
        target_path: Option<String>,
        status: Option<String>,
        reply_tx: tokio::sync::oneshot::Sender<Vec<DelegationSnapshot>>,
    ) {
        let target_path =
            target_path.map(|target| self.resolve_agent(&target).unwrap_or(target));
        let live_records: Vec<_> = self
            .ledger
            .snapshots()
            .filter(|&record| {
                caller_path.as_deref().is_none_or(|value| {
                    record
                        .caller_path
                        .as_ref()
                        .is_some_and(|caller| caller.as_str() == value)
                }) && target_path
                    .as_deref()
                    .is_none_or(|value| record.target_path.as_str() == value)
                    && status
                        .as_deref()
                        .is_none_or(|value| record.status.as_str() == value)
            })
            .cloned()
            .collect();
        let storage = self.infra.storage.clone();
        agentik_core::supervise::spawn_safe_on(
            &self.infra.runtime_handle,
            "list_delegations",
            async move {
                let mut records = live_records;
                let live_ids: std::collections::HashSet<_> =
                    records.iter().map(|record| record.delegation_id).collect();
                if let Ok(persisted) = storage
                    .list_agent_delegations(
                        caller_path.as_deref(),
                        target_path.as_deref(),
                        status.as_deref(),
                        1000,
                    )
                    .await
                {
                    records.extend(persisted.into_iter().filter_map(move |record| {
                        let delegation_id = record.delegation_id;
                        if live_ids.contains(&delegation_id) {
                            return None;
                        }
                        // Storage boundary: rows are written from
                        // validated paths, so both parse; a corrupt
                        // row is dropped rather than poisoning the
                        // typed snapshot.
                        let caller_path = record
                            .caller_path
                            .as_deref()
                            .and_then(|path| agentik_types::AgentPath::try_from(path).ok());
                        let target_path =
                            agentik_types::AgentPath::try_from(record.target_path.as_str()).ok()?;
                        Some(DelegationSnapshot {
                            delegation_id,
                            caller_path,
                            target_path,
                            task: record.task,
                            status: delegation_status_from_str(&record.status),
                            turn_id: record.turn_id,
                            session_id: record.session_id,
                            response: record.response,
                            created_at: record.created_at,
                            updated_at: record.updated_at,
                        })
                    }));
                }
                records.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
                let _ = reply_tx.send(records);
            },
        );
    }
}
