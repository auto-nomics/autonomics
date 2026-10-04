use std::sync::Arc;

use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::{Agent, AgentConfig};
use crate::context::ContextProvider;
use crate::error::AgentError;
use crate::memory::{
    MemoryBackend, MemoryConfig, MemoryStore, SemanticGrounding, memory_registrations,
};
use crate::session::AgentShared;
use crate::storage::AgentStorage;
use crate::tools::{ToolRegistration, ToolRegistry};
use agentik_sdk::types::messages::Message;

pub struct AgentBuilder {
    model: Arc<ArcSwapOption<Model>>,
    initial_messages: Vec<Message>,
    context_provider: Option<Arc<dyn ContextProvider>>,
    config: AgentConfig,
    storage: Option<Arc<dyn AgentStorage>>,
    tools: Vec<ToolRegistration>,
    system_prompt_section: Option<String>,
    system_prompt_identity: Option<String>,
    memory: Option<Arc<MemoryBackend>>,
    agent_event_tx: Option<tokio::sync::mpsc::UnboundedSender<agentik_sdk::types::AgentEvent>>,
    /// Stable agent UUID. If `None`, a fresh v4 UUID is generated at build time.
    id: Option<Uuid>,
    /// Hierarchical agent path. If `None`, defaults to `/root` at build time.
    path: Option<agentik_types::AgentPath>,
    /// Opaque configuration JSON persisted to the registry (e.g. RuntimeConfig).
    config_json: Option<serde_json::Value>,
    cancel_token: Option<CancellationToken>,
}

impl Clone for AgentBuilder {
    fn clone(&self) -> Self {
        Self {
            model: self.model.clone(),
            initial_messages: self.initial_messages.clone(),
            context_provider: self.context_provider.clone(),
            config: self.config.clone(),
            storage: self.storage.clone(),
            tools: Vec::new(),
            system_prompt_section: self.system_prompt_section.clone(),
            system_prompt_identity: self.system_prompt_identity.clone(),
            memory: self.memory.clone(),
            agent_event_tx: self.agent_event_tx.clone(),
            id: self.id,
            path: self.path.clone(),
            config_json: self.config_json.clone(),
            cancel_token: self.cancel_token.clone(),
        }
    }
}

impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            model: Default::default(),
            initial_messages: Vec::new(),
            context_provider: None,
            config: AgentConfig::default(),
            storage: None,
            tools: Vec::new(),
            system_prompt_section: None,
            system_prompt_identity: None,
            memory: None,
            agent_event_tx: None,
            id: None,
            path: None,
            config_json: None,
            cancel_token: None,
        }
    }

    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_model(mut self, model: Arc<ArcSwapOption<Model>>) -> Self {
        self.model = model;
        self
    }

    pub fn with_initial_messages(mut self, messages: Vec<Message>) -> Self {
        self.initial_messages = messages;
        self
    }

    pub fn with_context_provider(mut self, provider: Arc<dyn ContextProvider>) -> Self {
        self.context_provider = Some(provider);
        self
    }

    pub fn with_storage(mut self, storage: Arc<dyn AgentStorage>) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolRegistration>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_system_prompt_section(mut self, section: impl Into<String>) -> Self {
        self.system_prompt_section = Some(section.into());
        self
    }

    pub fn with_system_prompt_identity(mut self, identity: impl Into<String>) -> Self {
        self.system_prompt_identity = Some(identity.into());
        self
    }

    /// Enable persistent cross-session memory for this agent.
    ///
    /// Root-level agents generate and consolidate memory; all agents receive
    /// read/search tools and summary injection when `use_memory` is enabled.
    pub fn with_memory(
        mut self,
        config: MemoryConfig,
        store: Arc<dyn MemoryStore>,
        grounding: Option<Arc<dyn SemanticGrounding>>,
    ) -> Self {
        self.memory = Some(Arc::new(MemoryBackend::new(config, store, grounding)));
        self
    }

    /// Attach an existing backend so runtime config updates are shared with
    /// the host handle that requested them.
    pub fn with_memory_backend(mut self, memory: Arc<MemoryBackend>) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn with_agent_event_tx(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<agentik_sdk::types::AgentEvent>,
    ) -> Self {
        self.agent_event_tx = Some(tx);
        self
    }

    pub fn with_id(mut self, id: Uuid) -> Self {
        self.id = Some(id);
        self
    }

    /// Set the agent's hierarchical path directly.
    pub fn with_path(mut self, path: agentik_types::AgentPath) -> Self {
        self.path = Some(path);
        self
    }

    /// Backward-compatible: validates `name` as a segment and joins it to
    /// `/root`, producing e.g. `/root/researcher`.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.path = Some(
            agentik_types::AgentPath::root()
                .join(&name)
                .unwrap_or_else(|_| agentik_types::AgentPath::root()),
        );
        self
    }

    /// Set the opaque config JSON persisted to the registry (e.g. serialized
    /// `RuntimeConfig`). Used by the `runtime` crate to persist/restore agent
    /// configuration across process restarts.
    pub fn with_config_json(mut self, config_json: serde_json::Value) -> Self {
        self.config_json = Some(config_json);
        self
    }

    pub fn with_cancel_token(mut self, cancel_token: CancellationToken) -> Self {
        self.cancel_token = Some(cancel_token);
        self
    }

    pub async fn build(self) -> Result<Agent, AgentError> {
        let model = self.model.clone();

        let (internal_event_tx, internal_event_rx) = tokio::sync::mpsc::unbounded_channel();

        // ── Build the shared tool registry ──────────────────
        let tasks: Arc<tokio::sync::RwLock<crate::tools::task_runtime::TaskStore>> = Arc::new(
            tokio::sync::RwLock::new(crate::tools::task_runtime::TaskStore::new()),
        );

        let agent_id = self.id.unwrap_or_else(Uuid::new_v4);
        let plan_state = Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
            agentik_types::AgentPlan::new(),
        )));

        let event_tx = ArcSwapOption::new(self.agent_event_tx.clone().map(Arc::new));

        let plan_handle = crate::tools::builtins::PlanHandle::new(
            Arc::clone(&plan_state),
            agent_id,
            self.storage.clone(),
            self.agent_event_tx.clone(),
        );

        let mut registry = ToolRegistry::new();
        registry.register_all(self.tools)?;
        let memory = self.memory;
        if let Some(backend) = memory.as_ref() {
            registry.register_all(memory_registrations(Arc::clone(backend)))?;
        }
        registry.register_all(crate::tools::task_registrations(tasks.clone()))?;
        registry.register_all(crate::tools::plan_registrations(plan_handle))?;
        let registry = Arc::new(registry);

        // ── Build AgentShared ───────────────────────────────
        let shared = Arc::new(AgentShared {
            id: agent_id,
            path: self.path.unwrap_or_else(agentik_types::AgentPath::root),
            config_json: self.config_json.unwrap_or(serde_json::json!({})),
            model,
            config: self.config,
            storage: self.storage,
            context_provider: self.context_provider,
            system_prompt_section: self.system_prompt_section,
            system_prompt_identity: self.system_prompt_identity,
            memory,
            tool_registry: registry,
            tasks,
            event_tx,
            persist_tx: std::sync::OnceLock::new(),
            plan: plan_state,
        });

        // ── No default session at build time ────────────────
        // Sessions are restored from storage in `Agent::run()` or
        // auto-created on the first `MessageInject`.
        let cancel_token = self.cancel_token.unwrap_or_default();

        Ok(Agent {
            shared,
            sessions: std::collections::HashMap::new(),
            active_session_id: None,
            internal_event_tx,
            internal_event_rx: Some(internal_event_rx),
            initial_messages: if self.initial_messages.is_empty() {
                None
            } else {
                Some(self.initial_messages)
            },
            cancel_token,
        })
    }
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}
