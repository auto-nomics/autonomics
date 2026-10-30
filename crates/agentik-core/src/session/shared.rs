//! Stable resources shared across all sessions of one agent.

use std::sync::Arc;

use agentik_sdk::model::Model;
use agentik_sdk::types::AgentEvent;
use arc_swap::{ArcSwap, ArcSwapOption};
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

use crate::agent::AgentConfig;
use crate::context::ContextProvider;
use crate::storage::{AgentStorage, PersistOp};
use crate::tools::ToolRegistry;
use crate::tools::task_runtime::TaskStore;

/// Stable resources shared across all sessions of one agent.
///
/// Built once by [`AgentBuilder`](crate::agent_builder::AgentBuilder),
/// frozen into `Arc`, and cloned into every [`Session`](super::Session).
/// Fields that need runtime mutation use interior mutability (`ArcSwap`,
/// etc.).
///
/// Note that `AgentShared` is per-agent entity, each agent has its own `AgentShared` object.
pub(crate) struct AgentShared {
    pub id: Uuid,
    pub path: agentik_types::AgentPath,
    pub config_json: serde_json::Value,
    pub model: Arc<ArcSwapOption<Model>>,
    pub config: AgentConfig,
    pub storage: Option<Arc<dyn AgentStorage>>,
    pub context_provider: Option<Arc<dyn ContextProvider>>,
    pub system_prompt_section: Option<String>,
    pub system_prompt_identity: Option<String>,
    pub(crate) memory: Option<Arc<crate::memory::MemoryBackend>>,
    pub tool_registry: Arc<ToolRegistry>,
    /// Shared background-task list — the same Arc baked into the registry's
    /// task tools (`wait_task`, `view_task_results`, `view_task_status`).
    /// Sessions MUST create their Toolset with this handle (via
    /// `from_registry_with_tasks`) so that background tasks spawned by the
    /// Toolset are visible to the task-viewer tools.
    pub tasks: Arc<tokio::sync::RwLock<TaskStore>>,
    /// Event channel for external observers. Uses `ArcSwap` for interior
    /// mutability so `set_event_tx` works even after sessions are created.
    pub event_tx: ArcSwapOption<UnboundedSender<AgentEvent>>,
    /// WAL persistence sender, set once during `Agent::run()` bootstrap.
    /// Sessions created after bootstrap read this to wire their
    /// `persist_tx`.
    pub persist_tx: std::sync::OnceLock<UnboundedSender<PersistOp>>,
}

impl AgentShared {
    /// The agent's short name (last path segment).
    pub fn name(&self) -> &str {
        self.path.name()
    }

    /// The full hierarchical agent path.
    pub fn path(&self) -> &agentik_types::AgentPath {
        &self.path
    }

    /// Load the current event sender, if any.
    pub fn event_tx(&self) -> Option<UnboundedSender<AgentEvent>> {
        self.event_tx.load_full().as_deref().cloned()
    }

    /// Send an event to the optional observation channel.
    pub fn send_event(&self, event: AgentEvent) {
        if let Some(tx) = self.event_tx.load_full().as_deref() {
            let _ = tx.send(event);
        }
    }

    /// Build a minimal `AgentShared` suitable for unit-testing Session /
    /// add_message logic. No model, storage, persistence, or context provider
    /// is wired up.
    #[cfg(test)]
    pub(crate) fn new_for_tests() -> Arc<Self> {
        Arc::new(Self {
            id: Uuid::new_v4(),
            path: agentik_types::AgentPath::root(),
            config_json: serde_json::json!({}),
            model: Arc::new(ArcSwapOption::empty()),
            config: AgentConfig::default(),
            storage: None,
            context_provider: None,
            system_prompt_section: None,
            system_prompt_identity: None,
            memory: None,
            tool_registry: Arc::new(ToolRegistry::new()),
            tasks: Arc::new(tokio::sync::RwLock::new(
                crate::tools::task_runtime::TaskStore::new(),
            )),
            event_tx: ArcSwapOption::empty(),
            persist_tx: std::sync::OnceLock::new(),
        })
    }
}
