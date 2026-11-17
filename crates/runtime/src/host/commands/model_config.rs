//! Per-agent model and runtime-config commands: model hot-swap, runtime
//! override application (with the memory-pipeline kick on first enable),
//! and model introspection.

use std::sync::Arc;

use agentik_sdk::model::Model;

use super::super::RuntimeHost;
use super::super::relay::AgentCommand;

impl RuntimeHost {
    /// `HostCommand::SetAgentModel` — forward the new model to the relay
    /// and broadcast the change on the host event stream.
    pub(super) fn handle_set_agent_model(&mut self, name: String, model: Model) {
        // Capture the spec from the payload before the relay takes
        // ownership — the async slot store must not race the event.
        let spec = model.model_spec();
        let context_length = model.model_info.context_length;
        let resolved = self.resolve_agent(&name);
        self.send_agent_command(&name, AgentCommand::SetModel(model));
        if let Some(path) = resolved {
            self.emit_host_event(super::super::HostEvent::AgentModelChanged {
                path,
                spec,
                context_length,
            });
        }
    }

    /// `HostCommand::SetAgentRuntimeConfig` — merge the overrides into
    /// the effective runtime config; enabling memory generation on a
    /// root agent kicks a one-shot memory pipeline run.
    pub(super) fn handle_set_agent_runtime_config(
        &mut self,
        name: String,
        overrides: agentik_core::AgentRuntimeOverrides,
        reply_tx: tokio::sync::oneshot::Sender<
            std::result::Result<agentik_core::AgentRuntimeConfig, String>,
        >,
    ) {
        let defaults = self.infra.memory.runtime_config();
        let resolved = self.resolve_agent(&name).and_then(|key| {
            self.agents.get(&key).map(|entry| {
                let previous = entry.memory.runtime_config();
                let mut config = defaults;
                if let Some(enabled) = overrides.use_memory {
                    config.use_memory = enabled;
                }
                if let Some(enabled) = overrides.generate_memory {
                    config.generate_memory = enabled;
                }
                if let Some(effort) = overrides.reasoning_effort {
                    config.reasoning_effort = Some(effort);
                }
                entry.memory.set_runtime_config(config);
                if !previous.generate_memory
                    && config.generate_memory
                    && agentik_types::AgentPath::try_from(key.as_str()).is_ok_and(|path| {
                        self.infra
                            .memory
                            .effective_memory_config()
                            .is_root_agent(&path)
                    })
                    && let Some(agent_id) = entry.info.agent_id
                {
                    let storage = agentik_core::memory::agent_storage_memory_source(
                        self.infra.storage.clone(),
                    );
                    let memory = Arc::clone(&entry.memory);
                    let model = Arc::clone(&entry.model);
                    let task_name = format!("memory_runtime_update::{key}");
                    agentik_core::supervise::spawn_safe_on_drop(
                        &self.infra.runtime_handle,
                        &task_name,
                        agentik_core::memory::run_memory_pipeline(agent_id, storage, memory, model),
                    );
                }
                config
            })
        });
        let reply = resolved.ok_or_else(|| format!("agent `{name}` not found"));
        let _ = reply_tx.send(reply);
    }

    /// `HostCommand::GetAgentModel` — spec, context length, and the
    /// effort a request from this agent would actually send.
    pub(super) fn handle_get_agent_model(
        &mut self,
        name: String,
        reply_tx: tokio::sync::oneshot::Sender<
            Option<(String, u64, Option<agentik_types::ReasoningEffort>)>,
        >,
    ) {
        let resolved = self.resolve_agent(&name);
        let info = resolved
            .as_ref()
            .and_then(|key| self.agents.get(key))
            .and_then(|e| {
                e.model.load_full().as_deref().map(|m| {
                    (
                        m.model_spec(),
                        m.model_info.context_length,
                        effective_reasoning_effort(e, m),
                    )
                })
            });
        let _ = reply_tx.send(info);
    }
}

/// The effort a request from this agent would actually send — mirrors the
/// request layer's `inject_thinking` derivation: per-agent override first
/// (when the model supports thinking at all), else the standard medium
/// default. `None` when the model has no thinking support.
fn effective_reasoning_effort(
    entry: &super::super::relay::AgentEntry,
    model: &Model,
) -> Option<agentik_types::ReasoningEffort> {
    let info = &model.model_info;
    if !info.supports_thinking {
        return None;
    }
    Some(
        entry
            .memory
            .runtime_config()
            .reasoning_effort
            .unwrap_or_else(|| {
                if info.thinking_required && !info.thinking_enabled {
                    agentik_types::ReasoningEffort::Low
                } else {
                    agentik_types::ReasoningEffort::default()
                }
            }),
    )
}
