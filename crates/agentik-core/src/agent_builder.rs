use std::sync::Arc;

use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::{Agent, AgentConfig, TokenBudget};
use crate::context::ContextProvider;
use crate::error::AgentError;
use crate::skill::{self, Skill};
use crate::storage::AgentStorage;
use crate::tools::ToolRegistration;
use crate::{lifecycle::AgentLifecycle, memory::Memory, tools::Toolset};
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
    agent_event_tx: Option<tokio::sync::mpsc::UnboundedSender<agentik_sdk::types::AgentEvent>>,
    /// Stable agent UUID. If `None`, a fresh v4 UUID is generated at build time.
    id: Option<Uuid>,
    /// Human-readable name for the agent (used in the registry).
    name: Option<String>,
    /// Opaque configuration JSON persisted to the registry (e.g. RuntimeConfig).
    config_json: Option<serde_json::Value>,
    /// Pre-built memory (used to restore from a snapshot). When set, overrides
    /// `initial_messages`.
    memory: Option<Memory>,
    /// Optional skill workflow to attach to the agent.
    skill: Option<Skill>,
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
            agent_event_tx: self.agent_event_tx.clone(),
            id: self.id,
            name: self.name.clone(),
            config_json: self.config_json.clone(),
            memory: self.memory.clone(),
            skill: self.skill.clone(),
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
            agent_event_tx: None,
            id: None,
            name: None,
            config_json: None,
            memory: None,
            skill: None,
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

    pub fn with_skill(mut self, skill: Skill) -> Self {
        self.skill = Some(skill);
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

    /// Set the human-readable agent name (used in the persistence registry).
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the opaque config JSON persisted to the registry (e.g. serialized
    /// `RuntimeConfig`). Used by the `runtime` crate to persist/restore agent
    /// configuration across process restarts.
    pub fn with_config_json(mut self, config_json: serde_json::Value) -> Self {
        self.config_json = Some(config_json);
        self
    }

    pub fn with_memory(mut self, memory: Memory) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn with_cancel_token(mut self, cancel_token: CancellationToken) -> Self {
        self.cancel_token = Some(cancel_token);
        self
    }

    pub async fn build(mut self) -> Result<Agent, AgentError> {
        let model = self.model.clone();

        let skill_runtime = self.skill.take().map(skill::instantiate);

        let (internal_event_tx, internal_event_rx) = tokio::sync::mpsc::unbounded_channel();

        let mut toolset = Toolset::new(self.agent_event_tx.clone());
        toolset.register_all(self.tools)?;
        toolset.register_all(crate::tools::task_registrations(toolset.tasks_handle()))?;

        if let Some((_, todo_reg)) = &skill_runtime {
            toolset.register(todo_reg.clone())?;
        }

        let memory = if let Some(memory) = self.memory {
            memory
        } else {
            let mut memory = Memory::new();
            for msg in self.initial_messages {
                let _ = memory.remember(msg);
            }
            memory
        };

        let cancel_token = self.cancel_token.unwrap_or_default();

        Ok(Agent {
            id: self.id.unwrap_or_else(Uuid::new_v4),
            name: self.name.unwrap_or_else(|| "agent".to_string()),
            config_json: self.config_json.unwrap_or(serde_json::json!({})),
            model,
            memory,
            toolset,
            lifecycle: AgentLifecycle::new(),
            config: self.config,
            storage: self.storage,
            token_budget: TokenBudget::default(),
            context_provider: self.context_provider,
            system_prompt_section: self.system_prompt_section,
            system_prompt_identity: self.system_prompt_identity,
            skill_runtime: skill_runtime.map(|(rt, _)| rt),
            agent_event_tx: self.agent_event_tx,
            cancel_token,
            internal_event_tx,
            internal_event_rx: Some(internal_event_rx),
        })
    }
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}
