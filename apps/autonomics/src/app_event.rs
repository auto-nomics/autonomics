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
    /// The persistent task plan for a resumed context, loaded from storage.
    /// Restores the sidebar checklist so the user sees
    /// the same plan the model already knows about (it's injected as a
    /// tail notice). The backend restore does *not* emit an
    /// `AgentEvent::PlanUpdate`, so without this the TUI's `PlanState`
    /// stays empty until the model happens to call `update_plan` again.
    PlanLoaded {
        agent_id: uuid::Uuid,
        session_id: uuid::Uuid,
        plan: agentik_types::AgentPlan,
    },
    /// An effort override finished applying (async cycle path).
    EffortApplied { agent: String, label: String },
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
    /// A resident KMS worker changed state. The TUI keeps a lightweight
    /// observability map without polling the daemon.
    KmsWorkerStatusChanged(gateway::proto::KmsWorkerStatusView),
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
    /// A bibliography popup snapshot arrived. The token discards replies from
    /// a superseded refresh/search.
    BibliographySnapshotLoaded {
        token: u64,
        result: std::result::Result<gateway::proto::BibliographySnapshot, String>,
    },
    /// One selected article's full-text/annotation details arrived.
    BibliographyDetailsLoaded {
        token: u64,
        article_id: String,
        result: std::result::Result<gateway::proto::BibliographyArticleDetail, String>,
    },
    /// KMS skill-evolution dashboard data arrived: resident workers,
    /// active skills, durable proposals, and evidence.
    SkillEvolutionLoaded {
        workers: std::result::Result<Vec<gateway::proto::KmsWorkerStatusView>, String>,
        knowledge: std::result::Result<Vec<gateway::proto::KmsKnowledgeNodeView>, String>,
        skills: std::result::Result<Vec<gateway::proto::KmsSkillLibraryView>, String>,
        proposals: std::result::Result<Vec<gateway::proto::KmsSkillProposalView>, String>,
        evidence: std::result::Result<Vec<gateway::proto::KmsEvidenceView>, String>,
    },
    /// One skill's detail document arrived (fetched on selection
    /// move; rendered only while it matches the selected row).
    SkillDetailLoaded {
        name: String,
        result: std::result::Result<gateway::proto::SkillLibraryDetail, String>,
    },
    /// A human KMS proposal decision finished.
    SkillProposalDecision {
        approved: bool,
        result: std::result::Result<gateway::proto::KmsSkillProposalDecision, String>,
    },
    /// Polled KMS worker/proposal snapshot from the periodic dashboard poll.
    KmsDashboardStatusPolled {
        workers: std::result::Result<Vec<gateway::proto::KmsWorkerStatusView>, String>,
        proposals: std::result::Result<Vec<gateway::proto::KmsSkillProposalView>, String>,
    },
    /// Per-agent model info arrived (render-path cache fill; render itself
    /// must never issue HTTP). The string is the `provider:model` spec.
    ModelInfoLoaded {
        agent: String,
        info: Option<crate::state::ModelDisplay>,
    },
    /// 模型热切换被 daemon 拒绝（spec 不可解析、网络错误等）。乐观点亮
    /// 的标记没有失败提示无法自证，必须以 toast 告知用户。
    ModelSwapFailed { agent: String, error: String },
    /// A provider row save finished (PUT /model-config/provider).
    ProviderSaved {
        provider_name: String,
        result: std::result::Result<(), String>,
    },
    /// A custom model add/edit finished (PUT /model-config/model).
    ModelSaved {
        provider_name: String,
        result: std::result::Result<(), String>,
    },
    /// A custom model delete finished (DELETE /model-config/model).
    ModelDeleted {
        provider_name: String,
        model_name: String,
        result: std::result::Result<(), String>,
    },
    EmbeddingConfigLoaded {
        result: std::result::Result<gateway::proto::EmbeddingConfigView, String>,
    },
    EmbeddingConfigSaved {
        result: std::result::Result<gateway::proto::EmbeddingConfigView, String>,
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
