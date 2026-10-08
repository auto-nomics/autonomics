//! Internal events for decoupled communication between subsystems and the
//! main TUI loop. Modeled after codex's `AppEvent` + `AppEventSender` pattern.
//!
//! Components that don't have direct access to the `App` struct (e.g. future
//! file search, plugin watchers, background tasks) can push events through
//! an `AppEventSender` which the main loop drains each tick.

pub(crate) enum AppEvent {
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
    /// A fresh model catalogue snapshot arrived from the daemon (hydration,
    /// `ReloadCatalog`, or after a provider/catalog change). The handler
    /// rebuilds the config widget's catalog state.
    ModelCatalogLoaded(gateway::proto::ModelCatalog),
    /// The daemon's active default model changed (set-default, ChatGPT
    /// login/refresh). Mirrors the `ModelChanged` gateway notice.
    ActiveModelChanged {
        spec: Option<String>,
        reason: String,
    },
    /// A provider's remote catalogue fetch finished daemon-side (the
    /// `CatalogFetched` gateway notice). The payload carries the persisted
    /// model count.
    RemoteCatalogFetched {
        provider_name: String,
        result: std::result::Result<usize, String>,
    },
    /// ChatGPT 订阅登录：授权 URL 已就绪。处理器负责复制到剪贴板、
    /// 尽力打开浏览器并提示用户。
    ChatgptLoginUrl(String),
    /// ChatGPT 订阅登录结束（daemon 侧完成并落库）。`Ok` 携带账户信息
    /// 用于 toast 展示；`Err` 为可直接展示的失败原因。
    ChatgptLoginCompleted {
        result: std::result::Result<gateway::proto::ChatgptLoginInfo, String>,
    },
    /// A structured DAG snapshot arrived for the interactive TUI view.
    DagSnapshotLoaded(std::result::Result<dag_core::dag::DagTuiSnapshot, String>),
    /// Skill-evolution dashboard data arrived: service status, unified
    /// skill library, and recorded observations.
    SkillEvolutionLoaded {
        status: std::result::Result<gateway::proto::SkillEvolutionStatus, String>,
        library: std::result::Result<Vec<gateway::proto::SkillLibraryView>, String>,
        observations: std::result::Result<Vec<gateway::proto::SkillObservationView>, String>,
    },
    /// One skill's detail document arrived (fetched on selection
    /// move; rendered only while it matches the selected row).
    SkillDetailLoaded {
        name: String,
        result: std::result::Result<gateway::proto::SkillLibraryDetail, String>,
    },
    /// A manually triggered evolution cycle finished.
    SkillEvolutionTriggered(std::result::Result<gateway::proto::SkillEvolutionReport, String>),
    /// Polled status snapshot from the periodic dashboard poll —
    /// lighter than a full `SkillEvolutionLoaded` because it carries
    /// only the status fields. The handler promotes the snapshot
    /// fields and re-triggers the full refresh when generation /
    /// cycle counts have advanced since the last full load.
    SkillEvolutionStatusPolled {
        status: std::result::Result<gateway::proto::SkillEvolutionStatus, String>,
    },
    /// An approve/reject action on a proposal finished.
    SkillProposalActioned {
        action: &'static str,
        result: std::result::Result<String, String>,
    },
    /// Per-agent model info arrived (render-path cache fill; render itself
    /// must never issue HTTP). The string is the `provider:model` spec.
    ModelInfoLoaded {
        agent: String,
        info: Option<(String, u64)>,
    },
    /// 模型热切换被 daemon 拒绝（spec 不可解析、网络错误等）。乐观点亮
    /// 的标记没有失败提示无法自证，必须以 toast 告知用户。
    ModelSwapFailed { agent: String, error: String },
    /// A provider row save finished (PUT /model-config/provider).
    ProviderSaved {
        provider_name: String,
        result: std::result::Result<(), String>,
    },
    /// Per-agent runtime configuration arrived from the daemon.
    AgentConfigLoaded {
        agent: String,
        result: std::result::Result<gateway::proto::AgentRuntimeConfigView, String>,
    },
    /// Per-agent runtime configuration was saved.
    AgentConfigSaved {
        agent: String,
        result: std::result::Result<gateway::proto::AgentRuntimeConfigView, String>,
    },
    /// A live agent seen in a `/state` snapshot that the UI doesn't know
    /// yet (reconcile after lag / reconnect).
    AgentUpserted(gateway::AgentInfo),
    /// Known session list for an agent (from a `/state` snapshot).
    SessionListKnown {
        agent: String,
        sessions: Vec<agentik_types::SessionInfo>,
    },
}
