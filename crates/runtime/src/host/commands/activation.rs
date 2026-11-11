//! Virtual-actor activation: buffering deliveries for dormant identities,
//! materializing instances from their stored records, and flushing or
//! failing the buffered work when the activation resolves.

use std::sync::Arc;

use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;

use super::super::events::{ActivationCause, PendingDelivery, SpawnNotice};
use super::super::{AgentHandle, RuntimeHost, SharedInfra};

impl RuntimeHost {
    /// Register an agent whose background spawn task just completed, and
    /// broadcast the registration event. Shared by the command pump and
    /// [`Self::recv_next`]. Virtual-actor activations land here too —
    /// registration is followed by flushing the deliveries buffered for
    /// the identity while it was dormant.
    pub(in crate::host) fn register_background_spawn(
        &mut self,
        handle: AgentHandle,
        info: crate::control::AgentInfo,
        cause: ActivationCause,
    ) {
        let path = handle.path.clone();
        let event_info = self.register_agent(handle, info);
        self.emit_host_event(super::super::HostEvent::AgentRegistered {
            path: path.clone(),
            info: event_info,
            cause,
        });
        self.flush_activation_buffer(path.as_str());
        tracing::info!(agent = %path, ?cause, "background spawn completed and registered");
    }

    /// Handle a background spawn/activation outcome: register successes
    /// (flushing their buffered deliveries), fail the waiters on errors.
    pub(in crate::host) fn process_spawn_notice(&mut self, notice: SpawnNotice) {
        match notice {
            SpawnNotice::Activated {
                handle,
                info,
                cause,
            } => {
                self.register_background_spawn(handle, info, cause);
            }
            SpawnNotice::Failed { path, reason } => {
                self.fail_activation(&path, reason);
            }
        }
    }

    /// Deliver everything buffered for a just-activated identity, in the
    /// order the deliveries arrived. Runs on the pump right after the
    /// instance registers — the same single-threaded ordering that makes
    /// activation-vs-delivery races impossible.
    pub(super) fn flush_activation_buffer(&mut self, path: &str) {
        let Some(pending) = self.activation_pending.remove(path) else {
            return;
        };
        for delivery in pending {
            let sent = self.enqueue_agent_message(
                path,
                delivery.text,
                delivery.delegation_id,
                delivery.origin,
            );
            let result = if sent {
                Ok(())
            } else {
                Err(format!(
                    "Agent '{path}' is not accepting commands (its loop has exited)."
                ))
            };
            if let Some(reply_tx) = delivery.reply_tx {
                let _ = reply_tx.send(result);
            }
        }
    }

    /// Fail everything buffered for a path whose activation just failed,
    /// plus any delegation left Pending on the same identity. The "is not
    /// registered" phrasing is load-bearing: the gateway maps it to HTTP
    /// 404 (unknown identity); every other reason becomes 503.
    pub(super) fn fail_activation(&mut self, path: &str, reason: String) {
        if let Some(pending) = self.activation_pending.remove(path) {
            for delivery in pending {
                if let Some(reply_tx) = delivery.reply_tx {
                    let _ = reply_tx.send(Err(reason.clone()));
                }
            }
        }
        self.fail_pending_delegations(path, &reason);
        tracing::warn!(agent = %path, %reason, "activation failed");
    }

    /// Buffer a delivery for a dormant identity and kick off its
    /// activation if one is not already in flight (single-flight per
    /// path — concurrent deliveries coalesce into the buffer, in order).
    /// The pump flushes the buffer when the instance registers.
    pub(super) fn enqueue_after_activation(&mut self, path: String, delivery: PendingDelivery) {
        if let Some(buffer) = self.activation_pending.get_mut(&path) {
            buffer.push(delivery);
            return;
        }
        self.activation_pending.insert(path.clone(), vec![delivery]);

        let infra = self.infra.clone();
        let reg_tx = self.registration_tx.clone();
        let model_slot = self.model.clone();
        let resolver = self.model_resolver.clone();
        let task_path = path.clone();

        self.infra.runtime_handle.spawn(async move {
            let outcome = activate_identity(&infra, model_slot, resolver, &task_path).await;
            let notice = match outcome {
                Ok((handle, info)) => SpawnNotice::Activated {
                    handle,
                    info,
                    cause: ActivationCause::WakeUp,
                },
                Err(reason) => SpawnNotice::Failed { path, reason },
            };
            let _ = reg_tx.send(notice);
        });
    }
}

/// Activate one identity from its stored record (virtual-actor
/// activation): record → config → model → instance. Pure background work
/// — no host state is touched here; the pump registers the instance on
/// success and flushes its buffered deliveries. Identity reuse is free:
/// [`SharedInfra::spawn_agent`] looks the record up by path and re-binds
/// the same agent ID, and the agent's `run()` bootstrap restores the sole
/// context from storage.
async fn activate_identity(
    infra: &SharedInfra,
    model_slot: Option<Arc<ArcSwapOption<Model>>>,
    resolver: Option<crate::model_actor::ModelHandle>,
    path: &str,
) -> Result<(AgentHandle, crate::control::AgentInfo), String> {
    let record = match infra.storage.get_agent_by_name(path).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            // "is not registered" is load-bearing — the gateway maps it
            // to HTTP 404. Keep it in sync with the relay's send_to.
            return Err(format!(
                "Agent '{path}' is not registered. The live registry is empty until \
                 agents are spawned and resets on daemon restart; re-spawn or recover \
                 the agent before delivering messages."
            ));
        }
        Err(error) => return Err(format!("failed to load agent record for '{path}': {error}")),
    };
    let config = agentik_core::AgentProfileConfig::from_json(&record.config_json);
    let agent_path = agentik_types::AgentPath::try_from(path)
        .map_err(|error| format!("stored agent path `{path}` is invalid: {error}"))?;

    let model_override = match config.preferred_model.as_deref() {
        Some(spec) => {
            let resolver = resolver
                .ok_or("no model resolver installed; cannot resolve the agent's preferred model")?;
            Some(resolver.resolve(spec).await?)
        }
        None => None,
    };
    let slot_active = model_slot
        .as_ref()
        .and_then(|slot| slot.load_full())
        .is_some();
    if model_override.is_none() && !slot_active {
        return Err("No model configured on host.".to_string());
    }
    let slot = model_slot.ok_or_else(|| "No model configured on host.".to_string())?;

    let handle = infra
        .spawn_agent(&agent_path, &config, slot, model_override)
        .await
        .map_err(|error| error.to_string())?;
    let info = super::capability_from_kind(handle.path.name(), handle.path.as_str(), config.kind);
    Ok((handle, info))
}

#[cfg(test)]
mod activation_tests {
    //! Virtual-actor activation & passivation — see
    //! [`activation`](super) for the mechanism under test.

    use std::time::Duration;

    use super::super::test_support::{
        activation_mock_model, config, register_agent_at, wait_for_delivery_event,
    };
    use crate::host::RuntimeHost;

    /// Virtual-actor activation: after the instance is gone, a tracked
    /// delivery to the stored identity re-materializes it from its
    /// record, registers it, and lands the buffered message — with the
    /// delivery confirmation resolving only after the enqueue.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deliver_to_dormant_agent_activates_and_delivers() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(&dir);
        // Keep the startup memory pipeline (which calls a different mock
        // method) out of these turn-driving tests.
        cfg.use_memory = false;
        cfg.generate_memory = false;
        let mut host = RuntimeHost::open(&cfg).await.unwrap();
        let model = activation_mock_model("activation-test");
        host.set_model(model.clone());

        // First incarnation: spawn and let the run-loop bootstrap upsert
        // the agent record, then passivate it explicitly.
        register_agent_at(&mut host, "/root/wanderer", model).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        host.shutdown_agent("/root/wanderer");
        assert!(!host.contains_agent("/root/wanderer"));

        // Dormant delivery: activation → registration → buffered flush.
        let control = host.control();
        let delivery = tokio::spawn(async move {
            control
                .deliver_message_tracked("/root/wanderer", "wake up")
                .await
        });
        host.recv_and_process_command().await; // DeliverMessage → activation task
        host.recv_and_process_command().await; // SpawnNotice::Activated → register + flush
        wait_for_delivery_event(&mut host, "/root/wanderer", "wake up").await;
        assert_eq!(delivery.await.unwrap(), Some(Ok(())));
        assert!(host.contains_agent("/root/wanderer"));
    }

    /// An unknown identity (no stored record) fails activation with the
    /// load-bearing "is not registered" reason — the gateway maps it to
    /// HTTP 404.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deliver_to_unknown_identity_reports_not_registered() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(&dir);
        cfg.use_memory = false;
        cfg.generate_memory = false;
        let mut host = RuntimeHost::open(&cfg).await.unwrap();
        host.set_model(activation_mock_model("unknown-identity-test"));

        let control = host.control();
        let delivery =
            tokio::spawn(async move { control.deliver_message_tracked("/root/ghost", "hi").await });
        host.recv_and_process_command().await; // DeliverMessage → activation task
        host.recv_and_process_command().await; // SpawnNotice::Failed → fail waiters
        let reply = tokio::time::timeout(Duration::from_secs(5), delivery)
            .await
            .expect("reply must resolve after the failure notice")
            .unwrap();
        assert!(
            matches!(&reply, Some(Err(msg)) if msg.contains("is not registered")),
            "unexpected reply: {reply:?}"
        );
        assert!(!host.contains_agent("/root/ghost"));
    }

    /// Idle passivation: an agent idle past the TTL is gracefully evicted
    /// (its record survives), and the next message re-activates it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn idle_ttl_passivates_and_identity_survives() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(&dir);
        cfg.use_memory = false;
        cfg.generate_memory = false;
        cfg.agent_idle_ttl = Some(Duration::from_millis(150));
        let mut host = RuntimeHost::open(&cfg).await.unwrap();
        let model = activation_mock_model("ttl-test");
        host.set_model(model.clone());

        register_agent_at(&mut host, "/root/sleeper", model).await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        host.sweep_idle_agents();
        assert!(
            !host.contains_agent("/root/sleeper"),
            "idle instance must be passivated past the TTL"
        );
        // Identity survives in storage.
        let record = host
            .storage()
            .get_agent_by_name("/root/sleeper")
            .await
            .unwrap()
            .expect("record must survive passivation");

        // The next message re-activates the identity.
        let control = host.control();
        let delivery = tokio::spawn(async move {
            control
                .deliver_message_tracked("/root/sleeper", "again")
                .await
        });
        host.recv_and_process_command().await;
        host.recv_and_process_command().await;
        wait_for_delivery_event(&mut host, "/root/sleeper", "again").await;
        assert_eq!(delivery.await.unwrap(), Some(Ok(())));
        assert_eq!(
            host.agents
                .get("/root/sleeper")
                .and_then(|e| e.info.agent_id),
            Some(record.id),
            "re-activation must reuse the stored identity"
        );
    }
}
