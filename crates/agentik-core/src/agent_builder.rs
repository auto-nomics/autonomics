use std::sync::Arc;

use agentik_sdk::model::model_pool::ModelPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::{Agent, AgentConfig, TokenBudget};
use crate::context::ContextProvider;
use crate::error::AgentError;
use crate::skill::{self, Skill};
use crate::storage::AgentSnapshotStorage;
use crate::tools::ToolRegistration;
use crate::{lifecycle::AgentLifecycle, memory::Memory, tools::Toolset};
use agentik_sdk::types::messages::Message;

pub struct AgentBuilder {
    model_pool: Option<Arc<ModelPool>>,
    initial_messages: Vec<Message>,
    context_provider: Option<Arc<dyn ContextProvider>>,
    config: AgentConfig,
    storage: Option<Arc<dyn AgentSnapshotStorage>>,
    tools: Vec<ToolRegistration>,
    system_prompt_section: Option<String>,
    system_prompt_identity: Option<String>,
    agent_event_tx: Option<tokio::sync::mpsc::UnboundedSender<agentik_sdk::types::AgentEvent>>,
    /// Stable agent UUID. If `None`, a fresh v4 UUID is generated at build time.
    id: Option<Uuid>,
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
            model_pool: self.model_pool.clone(),
            initial_messages: self.initial_messages.clone(),
            context_provider: self.context_provider.clone(),
            config: self.config.clone(),
            storage: self.storage.clone(),
            tools: Vec::new(), // ToolRegistration is not Clone; re-register if needed
            system_prompt_section: self.system_prompt_section.clone(),
            system_prompt_identity: self.system_prompt_identity.clone(),
            agent_event_tx: self.agent_event_tx.clone(),
            id: self.id,
            memory: self.memory.clone(),
            skill: self.skill.clone(),
            cancel_token: self.cancel_token.clone(),
        }
    }
}

impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            model_pool: None,
            initial_messages: Vec::new(),
            context_provider: None,
            config: AgentConfig::default(),
            storage: None,
            tools: Vec::new(),
            system_prompt_section: None,
            system_prompt_identity: None,
            agent_event_tx: None,
            id: None,
            memory: None,
            skill: None,
            cancel_token: None,
        }
    }

    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_model_pool(mut self, pool: Arc<ModelPool>) -> Self {
        self.model_pool = Some(pool);
        self
    }

    /// Set initial messages to seed the agent's memory at build time.
    pub fn with_initial_messages(mut self, messages: Vec<Message>) -> Self {
        self.initial_messages = messages;
        self
    }

    /// Set an optional context provider for dynamic context injection.
    pub fn with_context_provider(mut self, provider: Arc<dyn ContextProvider>) -> Self {
        self.context_provider = Some(provider);
        self
    }

    pub fn with_storage(mut self, storage: Arc<dyn AgentSnapshotStorage>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Register additional tools on the agent (beyond the built-in lifecycle tools).
    pub fn with_tools(mut self, tools: Vec<ToolRegistration>) -> Self {
        self.tools = tools;
        self
    }

    /// Attach a skill workflow to the agent.
    ///
    /// At build time this constructs a [`SkillRuntime`](crate::skill::SkillRuntime)
    /// and registers the `update_todo` tool. While running, the agent is
    /// constrained to the current step's `allowed_tools` each turn, the
    /// step's todo progress is injected into the system prompt, and the
    /// workflow auto-advances once every todo in a step is completed.
    pub fn with_skill(mut self, skill: Skill) -> Self {
        self.skill = Some(skill);
        self
    }

    /// Set a static extra section for the system prompt.
    pub fn with_system_prompt_section(mut self, section: impl Into<String>) -> Self {
        self.system_prompt_section = Some(section.into());
        self
    }

    /// Set the agent identity line for the system prompt (e.g. "You are a biomedical research assistant.").
    pub fn with_system_prompt_identity(mut self, identity: impl Into<String>) -> Self {
        self.system_prompt_identity = Some(identity.into());
        self
    }

    /// Wire an event channel for streaming `AgentEvent`s to external observers (e.g. a TUI).
    pub fn with_agent_event_tx(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<agentik_sdk::types::AgentEvent>,
    ) -> Self {
        self.agent_event_tx = Some(tx);
        self
    }

    /// Override the agent's UUID. By default a fresh v4 UUID is generated.
    /// Use a stable id to persist an agent's identity across restarts.
    pub fn with_id(mut self, id: Uuid) -> Self {
        self.id = Some(id);
        self
    }

    /// Provide a pre-built `Memory` (e.g. restored from a snapshot).
    /// When set, overrides `initial_messages`.
    pub fn with_memory(mut self, memory: Memory) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn with_cancel_token(mut self, cancel_token: CancellationToken) -> Self {
        self.cancel_token = Some(cancel_token);
        self
    }

    pub async fn build(mut self) -> Result<Agent, AgentError> {
        let model_pool = self
            .model_pool
            .ok_or_else(|| AgentError::MissingConfig("model_pool".to_string()))?;

        // Instantiate the skill runtime (if any) and its `update_todo` tool.
        let skill_runtime = self.skill.take().map(skill::instantiate);

        // Internal event channel — tx is handed to the external runtime,
        // rx is consumed once by Agent::run().
        let (internal_event_tx, internal_event_rx) = tokio::sync::mpsc::unbounded_channel();

        // Register the toolset: caller-supplied external tools and
        // background-task tools.
        let mut toolset = Toolset::new(self.agent_event_tx.clone());
        toolset.register_all(self.tools)?;
        toolset.register_all(crate::tools::task_registrations(toolset.tasks_handle()))?;

        // Register the skill's todo tool so the agent can drive progress.
        if let Some((_, todo_reg)) = &skill_runtime {
            toolset.register(todo_reg.clone())?;
        }

        // Memory: prefer restored snapshot memory, otherwise seed from initial messages.
        let memory = if let Some(memory) = self.memory {
            memory
        } else {
            let mut memory = Memory::new();
            for msg in self.initial_messages {
                let _ = memory.remember(msg);
            }
            memory
        };

        // CancellationToken
        let cancel_token = self.cancel_token.unwrap_or_default();

        Ok(Agent {
            id: self.id.unwrap_or_else(Uuid::new_v4),
            model_pool,
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
            current_model_name: None,
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
