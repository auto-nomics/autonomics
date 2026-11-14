//! Inter-agent delegation command: request-response delivery to direct children,
//! with virtual-actor activation of dormant targets.

use agentik_core::tools::ProgressBuffer;

use super::super::RuntimeHost;
use super::super::events::PendingDelivery;
use super::super::unix_epoch_ms;
use crate::control::{DelegationSnapshot, DelegationStatus};

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
            let _ = reply_tx
                .send("Delegation failed: the calling agent path is unavailable.".to_string());
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
}
