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
    #[allow(dead_code)]
    Agent(Box<agentik_sdk::types::AgentEvent>),
    /// Request to exit the application.
    #[allow(dead_code)]
    Quit,
    /// Config data changed; the Config tab should reload from the database.
    #[allow(dead_code)]
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
    /// A provider's remote model catalogue finished loading (or failed).
    /// The payload carries metadata-only `ModelInfo`s; the handler persists
    /// them into the `models` table and reloads the catalogue widget.
    RemoteCatalogFetched {
        provider_name: String,
        result: std::result::Result<Vec<agentik_sdk::model::ModelInfo>, String>,
    },
    /// ChatGPT 订阅登录：授权 URL 已就绪。处理器负责复制到剪贴板、
    /// 尽力打开浏览器并提示用户。
    ChatgptLoginUrl(String),
    /// ChatGPT token 主动/自愈刷新产物：新 blob JSON。主循环覆写 openai
    /// 行的 api_key（token 轮转落库，重启免重登）。
    ChatgptTokenRefreshed(String),
    /// A structured DAG snapshot arrived for the interactive TUI view.
    DagSnapshotLoaded(Result<dag_core::dag::DagTuiSnapshot, String>),
    /// ChatGPT 订阅登录结束。`Ok` 携带 token blob（处理器写库并重载目
    /// 录）；`Err` 为可直接展示的失败原因（取消/超时/端口占用/交换失败）。
    ChatgptLoginCompleted {
        result: std::result::Result<agentik_sdk::provider::openai::oauth::TokenBlob, String>,
    },
}
