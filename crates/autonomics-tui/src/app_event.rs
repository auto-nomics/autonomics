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
    /// One bounded page of agent records for the resume picker, tagged
    /// with the query it answers — stale pages (query moved on) are
    /// dropped at apply time.
    AgentRecordsLoaded {
        query: String,
        records: Vec<agentik_core::storage::AgentRecord>,
    },
    /// A keystroke in the resume picker settled (debounce elapsed) —
    /// fetch the matching server-side page.
    SearchStoredAgents(String),
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
    /// A provider's remote catalogue fetch finished daemon-side (the
    /// `CatalogFetched` gateway notice). The payload carries the persisted
    /// model count.
    RemoteCatalogFetched {
        provider_name: String,
        result: std::result::Result<usize, String>,
    },
    /// ChatGPT subscription login: the authorize URL is ready. The
    /// handler copies it to the clipboard, best-effort opens the browser,
    /// and shows a hint.
    ChatgptLoginUrl(String),
    /// ChatGPT subscription login finished (completed and persisted
    /// daemon-side). `Ok` carries the account info for the toast; `Err`
    /// is a directly displayable failure reason.
    ChatgptLoginCompleted {
        result: std::result::Result<gateway::proto::ChatgptLoginInfo, String>,
    },
    /// A structured DAG snapshot arrived for the interactive TUI view.
    DagSnapshotLoaded(std::result::Result<dag_core::dag::DagTuiSnapshot, String>),
    FlowViewLoaded(std::result::Result<data_engine::flow::FlowView, String>),
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
    /// Skill library dashboard data arrived.
    SkillLibraryLoaded {
        library: std::result::Result<Vec<gateway::proto::SkillLibraryView>, String>,
    },
    /// One skill's detail document arrived (fetched on selection
    /// move; rendered only while it matches the selected row).
    SkillDetailLoaded {
        name: String,
        result: std::result::Result<gateway::proto::SkillLibraryDetail, String>,
    },
    /// Per-agent model info arrived (render-path cache fill; render itself
    /// must never issue HTTP). The string is the `provider:model` spec.
    ModelInfoLoaded {
        agent: String,
        info: Option<crate::state::ModelDisplay>,
    },
    /// A hot model swap was rejected by the daemon (unresolvable spec,
    /// network error, …). The optimistically lit marker cannot speak for
    /// itself without a failure toast, so the user must be told.
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
    /// Surface a background-task failure as a toast (async tasks cannot
    /// touch `state.toasts` directly).
    ShowError {
        title: String,
        detail: Option<String>,
    },
    /// Known sole context for an agent (from a `/state` snapshot).
    ContextListKnown {
        agent: String,
        context: Option<agentik_types::ContextInfo>,
    },
    /// HPC site config snapshot from `GET /hpc/config`.
    HpcConfigLoaded(std::result::Result<serde_json::Value, String>),
    /// HPC monitor inventory from `GET /hpc/jobs`.
    HpcJobsLoaded(serde_json::Value),
    /// Site profile persisted via `PUT /hpc/config`.
    HpcProfileSaved(std::result::Result<serde_json::Value, String>),
    /// Directory listing from `GET /vfs/list` (cwd + entries).
    VfsListingLoaded(std::result::Result<(String, Vec<vfs::browse::VfsEntry>), String>),
    /// Text preview from `GET /vfs/text`.
    VfsTextLoaded(std::result::Result<vfs::browse::TextRead, String>),
    /// Mount table from `GET /vfs/mounts` (path, read_only).
    VfsMountsLoaded(std::result::Result<Vec<(String, bool)>, String>),
    /// A VFS mutation finished (status message or error).
    VfsMutationDone(std::result::Result<String, String>),
}
