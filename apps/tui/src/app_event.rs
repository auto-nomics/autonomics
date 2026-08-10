//! Internal events for decoupled communication between subsystems and the
//! main TUI loop. Modeled after codex's `AppEvent` + `AppEventSender` pattern.
//!
//! Components that don't have direct access to the `App` struct (e.g. future
//! file search, plugin watchers, background tasks) can push events through
//! an `AppEventSender` which the main loop drains each tick.

pub(crate) enum AppEvent {
    /// An event from the agent runtime (text delta, tool call, etc.).
    ///
    /// Boxed because `AgentEvent` is large (it carries a full `Message`),
    /// which would otherwise dominate the enum size — see
    /// `clippy::large_enum_variant`.
    Agent(Box<agentik_sdk::types::AgentEvent>),
    /// Request to exit the application.
    Quit,
    /// Config data changed; the Config tab should reload from the database.
    ConfigReload,
    /// A new agent was spawned (or failed to spawn) from a profile.
    AgentSpawned {
        profile_name: String,
        result: std::result::Result<String, String>,
    },
    /// Agent records loaded from storage (for the resume picker).
    AgentRecordsLoaded(Vec<agentik_core::storage::AgentRecord>),
    /// An agent was deleted from storage. Carries the agent ID.
    AgentDeleted(uuid::Uuid),
    /// An agent was renamed in storage. Carries the agent ID and new path.
    AgentRenamed {
        agent_id: uuid::Uuid,
        new_path: agentik_types::AgentPath,
    },
    /// Conversation history loaded from storage for a resumed agent's
    /// specific session.
    HistoryLoaded {
        agent_id: uuid::Uuid,
        session_id: uuid::Uuid,
        messages: Vec<agentik_sdk::types::messages::Message>,
    },
    /// The persistent task plan for a resumed agent, loaded from storage.
    /// Restores the sidebar checklist so the user sees the same plan the
    /// model already knows about (it's injected into the system prompt).
    /// The backend bootstrap does *not* emit `AgentEvent::PlanUpdate`, so
    /// without this the TUI's `PlanState` stays empty until the model
    /// happens to call `update_plan` again.
    PlanLoaded {
        agent_id: uuid::Uuid,
        plan: agentik_types::AgentPlan,
    },
}
