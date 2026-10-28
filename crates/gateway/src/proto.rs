//! Wire protocol types shared by the gateway server and its clients.
//!
//! Design rule: wherever a runtime type is already serde-complete
//! (`AgentEvent`, `AgentInfo`, `AgentKind`, …) it crosses the wire
//! as-is, so frontends reuse their existing typed event handlers verbatim.
//! Types that are *not* serde-complete get a `…View` mirror here
//! (`HostEvent` → [`HostEventView`]).

use std::collections::HashMap;

use agentik_core::{AgentRuntimeConfig, AgentRuntimeOverrides};
use agentik_types::SessionInfo;
use runtime::control::{AgentInfo, AgentStatus};
use runtime::model_actor::{DisplaySettings as DisplaySettingsData, ModelCatalogData};
use runtime::model_bootstrap::{ModelRow, ProviderRow};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn opaque_object() -> utoipa::openapi::Object {
    utoipa::openapi::Object::new()
}

/// serde mirror of [`runtime::HostEvent`] (which is not serde — it carries
/// an `AgentPath` and is only consumed in-process today).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostEventView {
    AgentRegistered {
        path: String,
        info: AgentInfo,
    },
    AgentUnregistered {
        path: String,
    },
    AgentStatusChanged {
        path: String,
        status: AgentStatus,
        last_event: Option<String>,
    },
    AgentModelChanged {
        path: String,
        spec: String,
        context_length: u64,
    },
}

impl From<runtime::HostEvent> for HostEventView {
    fn from(event: runtime::HostEvent) -> Self {
        match event {
            runtime::HostEvent::AgentRegistered { path, info } => HostEventView::AgentRegistered {
                path: path.as_str().to_string(),
                info,
            },
            runtime::HostEvent::AgentUnregistered { path } => {
                HostEventView::AgentUnregistered { path }
            }
            runtime::HostEvent::AgentStatusChanged {
                path,
                status,
                last_event,
            } => HostEventView::AgentStatusChanged {
                path,
                status,
                last_event,
            },
            runtime::HostEvent::AgentModelChanged {
                path,
                spec,
                context_length,
            } => HostEventView::AgentModelChanged {
                path,
                spec,
                context_length,
            },
        }
    }
}

/// Daemon-side lifecycle notices that are neither agent events nor host
/// events — e.g. "the active model changed", "the ChatGPT login finished".
/// Delivered as `event: notice` SSE frames.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GatewayNotice {
    /// The daemon-wide default model slot changed (set-default, ChatGPT
    /// login/refresh rebuilding an openai model). `spec` is the new
    /// `provider:model` string; `reason` names the trigger.
    ModelChanged {
        spec: Option<String>,
        reason: String,
    },
    /// A ChatGPT OAuth login flow started via the daemon completed.
    ChatgptLogin {
        result: Result<ChatgptLoginInfo, String>,
    },
    /// A provider's remote model catalogue fetch finished (models
    /// persisted daemon-side; the result carries the persisted count).
    CatalogFetched {
        provider: String,
        result: Result<usize, String>,
    },
    /// A resident KMS worker changed state or completed a drain pass.
    KmsWorkerChanged { status: KmsWorkerStatusView },
}

impl From<agentik_core::memory::KnowledgeWorkerEvent> for KmsWorkerStatusView {
    fn from(event: agentik_core::memory::KnowledgeWorkerEvent) -> Self {
        Self {
            worker: event.worker.as_str().to_string(),
            state: match event.state {
                agentik_core::memory::KnowledgeWorkerState::Idle => "idle".to_string(),
                agentik_core::memory::KnowledgeWorkerState::Running => "running".to_string(),
                agentik_core::memory::KnowledgeWorkerState::Stopped => "stopped".to_string(),
            },
            running: event.status.running,
            last_run_at: event.status.last_run_at,
            last_error: event.status.last_error,
            total_claimed: event.status.total_claimed,
            total_executed: event.status.total_executed,
            total_released: event.status.total_released,
            at_ms: event.at_ms,
        }
    }
}

impl From<agentik_core::memory::KmsWorkerStatusSnapshot> for KmsWorkerStatusView {
    fn from(snapshot: agentik_core::memory::KmsWorkerStatusSnapshot) -> Self {
        Self {
            worker: snapshot.worker.as_str().to_string(),
            state: if snapshot.status.running {
                "running".to_string()
            } else {
                "idle".to_string()
            },
            running: snapshot.status.running,
            last_run_at: snapshot.status.last_run_at,
            last_error: snapshot.status.last_error,
            total_claimed: snapshot.status.total_claimed,
            total_executed: snapshot.status.total_executed,
            total_released: snapshot.status.total_released,
            at_ms: snapshot.status.last_run_at.unwrap_or_default(),
        }
    }
}

/// Wire view of one resident knowledge worker.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct KmsWorkerStatusView {
    pub worker: String,
    pub state: String,
    pub running: bool,
    pub last_run_at: Option<i64>,
    pub last_error: Option<String>,
    pub total_claimed: u64,
    pub total_executed: u64,
    pub total_released: u64,
    pub at_ms: i64,
}

/// One durable KMS evidence row, including consolidation status.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct KmsEvidenceView {
    pub id: String,
    pub source_kind: String,
    pub source_id: String,
    pub evidence_kind: String,
    pub summary: String,
    pub body: String,
    pub anchor_path: Option<String>,
    pub error: Option<String>,
    pub status: String,
    pub occurrence_count: u64,
    pub created_at: i64,
    pub last_seen_at: i64,
}

/// One active wiki node in the durable KMS knowledge tree.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct KmsKnowledgeNodeView {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub path: String,
    pub title: String,
    pub body_md: String,
    pub status: String,
    pub confidence: f64,
    pub source_hash: String,
    pub structural: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One durable KMS skill-revision proposal, including the material a
/// reviewer needs to inspect before approving it.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct KmsSkillProposalView {
    pub id: Uuid,
    pub skill_name: String,
    pub node_path: String,
    pub status: String,
    pub title: String,
    pub body_md: String,
    pub rationale: String,
    pub source_evidence_ids: Vec<String>,
    pub artifacts: Vec<KmsSkillArtifactView>,
    pub content_hash: String,
    pub approved_skill_version_id: Option<Uuid>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One text artifact attached to a proposed skill revision.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct KmsSkillArtifactView {
    pub relative_path: String,
    pub content: String,
}

/// Active KMS skill projection used by management UI listings.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct KmsSkillLibraryView {
    pub id: Uuid,
    pub skill_name: String,
    pub node_path: String,
    pub version: u32,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub tier: String,
    pub source_tier: String,
    pub created_at: i64,
    pub artifact_paths: Vec<String>,
}

/// One version in a KMS skill's immutable history.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct KmsSkillVersionSummaryView {
    pub id: Uuid,
    pub version: u32,
    pub status: String,
    pub title: String,
    pub content_hash: String,
    pub created_at: i64,
}

/// KMS skill detail: active document plus version history and artifacts.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct KmsSkillDetailView {
    pub id: Uuid,
    pub skill_name: String,
    pub node_path: String,
    pub version: u32,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub tier: String,
    pub source_tier: String,
    pub body_md: String,
    pub created_at: i64,
    pub artifacts: Vec<KmsSkillArtifactView>,
    pub history: Vec<KmsSkillVersionSummaryView>,
}

/// Terminal result of approving or rejecting a KMS skill proposal.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct KmsSkillProposalDecision {
    pub id: Uuid,
    pub skill_name: String,
    pub status: String,
    pub skill_version_id: Option<Uuid>,
}

/// User-visible outcome of a ChatGPT login.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChatgptLoginInfo {
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

// ── Bibliography ────────────────────────────────────────────────────────

/// Bounded library snapshot for the TUI bibliography popup: collections plus
/// the articles matching the current search query. The endpoint remains the
/// source of truth for ranking/FTS; the TUI only refines locally while typing.
#[derive(Debug, Clone, Deserialize)]
pub struct BibliographySnapshot {
    pub collections: Vec<bib_types::Collection>,
    pub articles: Vec<bib_types::Article>,
}

/// Article details shown in the popup's right pane.
#[derive(Debug, Clone, Deserialize)]
pub struct BibliographyArticleDetail {
    pub article: bib_types::Article,
    pub fulltext: Option<bib_types::FullText>,
    pub annotations: Vec<bib_types::Annotation>,
}

// ── Hydration ─────────────────────────────────────────────────────────

/// One frame of a `Lagged` notification: the subscriber fell behind the
/// broadcast buffer and must re-hydrate from `GET /state`.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LagFrame {
    pub missed: u64,
    pub resume_seq: u64,
}

/// Cold-start snapshot for a freshly connected frontend. `last_seq` is read
/// from the hub *after* the rest of the snapshot is serialized, so applying
/// the snapshot and then all live frames with `seq > last_seq` (dedup by
/// seq) yields a consistent state with no gaps and no duplicates.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StateSnapshot {
    pub last_seq: u64,
    pub active_model_spec: Option<String>,
    #[schema(schema_with = opaque_object)]
    pub profiles: Vec<agentik_core::AgentKind>,
    /// Live registered agents (from `HostControl::get_status`).
    #[schema(schema_with = opaque_object)]
    pub agents: Vec<AgentInfo>,
    /// Known per-agent session lists, folded by the driver from
    /// `AgentEvent::SessionList`. Absent until an agent has been asked
    /// (`POST /agents/{name}/sessions/list`) or emitted a list.
    #[schema(schema_with = opaque_object)]
    pub sessions: HashMap<String, Vec<SessionInfo>>,
    pub display_settings: DisplaySettings,
    /// Daemon-wide defaults resolved from the runtime configuration.
    #[schema(schema_with = opaque_object)]
    pub runtime_defaults: AgentRuntimeConfig,
    pub model_catalog: ModelCatalog,
}

/// Display toggles persisted in the daemon's `settings` table.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DisplaySettings {
    pub collapse_thinking: bool,
    pub collapse_tool_calls: bool,
    pub collapse_tool_results: bool,
}

impl From<DisplaySettingsData> for DisplaySettings {
    fn from(data: DisplaySettingsData) -> Self {
        Self {
            collapse_thinking: data.collapse_thinking,
            collapse_tool_calls: data.collapse_tool_calls,
            collapse_tool_results: data.collapse_tool_results,
        }
    }
}

/// Model catalogue + credentials view over the daemon-owned app DB.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ModelCatalog {
    #[schema(schema_with = opaque_object)]
    pub providers: Vec<ProviderRow>,
    #[schema(schema_with = opaque_object)]
    pub models: Vec<ModelRow>,
    /// Active default model as `provider_name:model_name`.
    pub active_model: Option<String>,
    /// Health of the openai ChatGPT subscription token, when present.
    pub openai_token: Option<OpenaiTokenState>,
}

impl From<ModelCatalogData> for ModelCatalog {
    fn from(data: ModelCatalogData) -> Self {
        Self {
            providers: data.providers,
            models: data.models,
            active_model: data.active_model,
            openai_token: data.openai_token.map(|t| OpenaiTokenState {
                present: t.present,
                expired: t.expired,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OpenaiTokenState {
    pub present: bool,
    pub expired: bool,
}

/// `GET /api/v1/gateway/status` — liveness probe for `manager::probe`.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GatewayStatus {
    pub pid: u32,
    pub addr: String,
    pub version: String,
    pub uptime_secs: u64,
    pub last_seq: u64,
    pub agent_count: usize,
}

// ── Requests / responses ──────────────────────────────────────────────

/// `POST /api/v1/agents` — spawn from an agent kind (covers both the
/// profile picker and the resume-from-storage flow).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpawnAgentRequest {
    /// Agent name segment (joined onto `parent_path`).
    pub name: String,
    /// Parent path as a string, e.g. `/root`.
    pub parent_path: String,
    /// Agent kind to instantiate: `researcher` or `developer`.
    pub profile: String,
    /// Optional per-agent runtime overrides (used by the resume flow to
    /// restore the persisted settings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(schema_with = opaque_object)]
    pub runtime: Option<AgentRuntimeOverrides>,
    /// Optional `provider:model` override; unresolvable specs fall back to
    /// the daemon's default model (same semantics as the TUI today).
    pub model_spec: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpawnAgentResponse {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeliverMessageRequest {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SetAgentModelRequest {
    pub spec: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AgentModelInfoView {
    /// The agent's current model as a `provider:model` spec.
    pub model: String,
    pub context_length: u64,
    /// The reasoning effort this agent's next request would send
    /// (`low | medium | high | …`), absent when the model has no
    /// thinking support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// Effective per-agent runtime settings plus the persisted override layer.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AgentRuntimeConfigView {
    #[schema(schema_with = opaque_object)]
    pub runtime: AgentRuntimeOverrides,
    #[schema(schema_with = opaque_object)]
    pub effective: AgentRuntimeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SetAgentRuntimeConfigRequest {
    #[schema(schema_with = opaque_object)]
    pub runtime: AgentRuntimeOverrides,
}

/// `GET /storage/agents/{id}/sessions` — persisted session metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct StoredSession {
    pub id: Uuid,
    pub title: Option<String>,
    /// Unix epoch milliseconds.
    pub created_at: i64,
    /// Unix epoch milliseconds; equals `created_at` while still active.
    pub last_active: i64,
    pub active: bool,
    #[serde(default)]
    pub telemetry: agentik_types::SessionTelemetry,
    #[serde(default)]
    pub parent_session_id: Option<Uuid>,
    #[serde(default)]
    pub fork_point_message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateSessionRequest {
    pub title: Option<String>,
    pub fork_from: Option<Uuid>,
    /// Final parent message to include in the new session. Requires
    /// `fork_from`; ignored for new root sessions.
    #[serde(default)]
    pub fork_message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RenameSessionRequest {
    pub title: String,
}

/// `PATCH /api/v1/storage/agents/{uuid}` — rename (new full path).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RenameAgentRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SaveProviderRequest {
    pub name: String,
    pub provider_type: String,
    /// Empty → registry default for the provider type.
    pub base_url: String,
    pub api_key: String,
}

/// `PUT /api/v1/model-config/model` — add or update a custom model.
/// `original_model_name` identifies the storage row being renamed.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SaveModelRequest {
    pub provider_name: String,
    #[serde(default)]
    pub original_model_name: Option<String>,
    pub model_name: String,
    pub context_length: u64,
    pub max_output_tokens: u64,
    pub vision_ability: bool,
    pub supports_function_calling: bool,
    pub supports_streaming: bool,
    pub supports_thinking: bool,
    pub thinking_enabled: bool,
    pub input_token_price: f64,
    pub output_token_price: f64,
}

/// `DELETE /api/v1/model-config/model?provider_name=...&model_name=...` —
/// remove a custom model row from storage. Built-in SDK presets are
/// protected and rejected by the storage layer with an HTTP 400.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeleteModelRequest {
    pub provider_name: String,
    pub model_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SetActiveModelRequest {
    pub spec: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FetchCatalogRequest {
    /// Empty → registry default base URL.
    pub base_url: String,
}

/// Ollama embedding configuration exposed by `/embedding-config`.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EmbeddingConfigView {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub batch_size: usize,
    pub timeout_secs: u64,
}

impl From<text_embedding::EmbeddingConfig> for EmbeddingConfigView {
    fn from(config: text_embedding::EmbeddingConfig) -> Self {
        Self {
            enabled: config.enabled,
            base_url: config.base_url,
            model: config.model,
            batch_size: config.batch_size,
            timeout_secs: config.timeout_secs,
        }
    }
}

impl From<EmbeddingConfigView> for text_embedding::EmbeddingConfig {
    fn from(view: EmbeddingConfigView) -> Self {
        Self {
            enabled: view.enabled,
            base_url: view.base_url,
            model: view.model,
            batch_size: view.batch_size,
            timeout_secs: view.timeout_secs,
        }
    }
}

/// `PUT /api/v1/embedding-config` body.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SaveEmbeddingConfigRequest {
    pub config: EmbeddingConfigView,
}

/// `POST /api/v1/model-config/chatgpt/login` response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChatgptLoginStart {
    pub url: String,
}

/// `POST /api/v1/model-config/chatgpt/refresh` response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChatgptRefreshResponse {
    pub refreshed: bool,
}

/// `GET /api/v1/settings` / `PUT /api/v1/settings` body.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SettingsMap {
    pub settings: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PutSettingRequest {
    pub key: String,
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire contract for `AgentEvent` is "serde default externally
    /// tagged enum" — pin the JSON shape so frontends can rely on it.
    #[test]
    fn host_event_view_round_trips() {
        let view = HostEventView::AgentStatusChanged {
            path: "/root/worker".into(),
            status: AgentStatus::AwaitingTool {
                tool: "run_bash".into(),
            },
            last_event: Some("run_bash".into()),
        };
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains(r#""kind":"agent_status_changed""#), "{json}");
        let back: HostEventView = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, HostEventView::AgentStatusChanged { .. }));
    }

    #[test]
    fn notice_round_trips() {
        let notice = GatewayNotice::ModelChanged {
            spec: Some("openai:gpt-5".into()),
            reason: "set_default".into(),
        };
        let json = serde_json::to_string(&notice).unwrap();
        let back: GatewayNotice = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, GatewayNotice::ModelChanged { .. }));
    }
}

/// Legacy filesystem proposal metadata embedded in compatibility
/// library responses. Durable governance proposals use
/// [`KmsSkillProposalView`].
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillProposalView {
    pub name: String,
    pub status: String,
    /// `distiller` (the deterministic loop) — the only author; legacy
    /// `agent` manifests from the removed skill_propose channel may
    /// appear in old state dirs.
    pub authored_by: String,
    pub update: bool,
    pub rationale: String,
    pub cluster_hash: String,
    pub observation_count: usize,
    pub created_at: i64,
}

/// `GET /api/v1/skills/library` row — one skill from the unified
/// library view: every installed skill (all tiers) plus every
/// proposed-but-not-installed name, with the proposal pipeline as an
/// attribute rather than a separate listing.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillLibraryView {
    pub name: String,
    /// builtin | global | workspace | proposed (not installed yet).
    pub tier: String,
    pub tags: Vec<String>,
    pub description: String,
    /// False only for rows that exist solely as a pending proposal.
    pub installed: bool,
    /// Latest proposal status for this name, when the pipeline ever
    /// touched it (pending | applied | rejected-legacy).
    pub proposal_status: Option<String>,
    /// The proposal revises an already-installed skill.
    pub proposal_update: bool,
    // ── usage telemetry (the evolution fitness signal) ──
    pub usage_gets: u64,
    pub usage_search_hits: u64,
    pub usage_runs: u64,
    pub usage_evals: u64,
    /// Unix seconds of the last recorded use; 0 = never.
    pub usage_last_used: i64,
}

/// `GET /api/v1/skills/library/{name}` response — one skill's full
/// detail: metadata, usage, its proposal (with evidence count), and
/// the complete SKILL.md body.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillLibraryDetail {
    pub name: String,
    pub tier: String,
    pub tags: Vec<String>,
    pub description: String,
    pub installed: bool,
    /// The full SKILL.md body below the frontmatter.
    pub body: String,
    /// Workflow template stems bundled with the skill (installed
    /// rows only).
    pub workflows: Vec<String>,
    /// Eval file stems bundled with the skill (installed rows only).
    pub evals: Vec<String>,
    /// The pipeline record for this name, when one exists.
    pub proposal: Option<SkillProposalView>,
    /// Observations backing the proposal (evidence chain size).
    pub evidence_count: usize,
    pub usage_gets: u64,
    pub usage_search_hits: u64,
    pub usage_runs: u64,
    pub usage_evals: u64,
    pub usage_last_used: i64,
}

// ── Plugins ──────────────────────────────────────────────────────────

/// `GET /api/v1/plugins` — the manifest plugin families installed under
/// the daemon's plugins root. Built-in node bundles are compiled into the
/// binary and are not plugin families; a dedicated node-listing endpoint
/// covers them.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PluginListView {
    /// The plugins root the daemon's node registry actually scanned at
    /// startup (env-pinned by the startup sync, else the loader default).
    pub root: String,
    pub plugins: Vec<PluginView>,
}

/// One installed plugin family.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PluginView {
    /// Family name from the manifest (the directory name while the
    /// manifest is unparseable).
    pub name: String,
    /// Daemon-owned lifecycle state from the plugin manifest.
    pub status: String,
    /// Image reference `host/path@sha256:…` (absent when the manifest
    /// failed to parse).
    pub image: Option<String>,
    /// Node kinds this family declares.
    pub kinds: Vec<String>,
    /// Catalog panel bindings shared by the family's nodes.
    pub panels: Vec<PluginPanelView>,
    /// Installation source declared in `plugins.toml` — the declaration
    /// the startup sync re-materializes from, so it survives restarts
    /// (absent for families materialized without a declaration).
    pub source: Option<PluginSourceView>,
    /// Whether every declared kind is present in the live node registry
    /// (the daemon registered the family at startup). False after
    /// on-disk drift until the next restart.
    pub registered: bool,
}

/// One catalog panel binding: mount contract plus the dataset repo that
/// satisfies it.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PluginPanelView {
    pub binding: String,
    pub mount: String,
    /// Hugging Face dataset repository `owner/name`.
    pub bundle: String,
}

/// Installation source from `plugins.toml` — exactly one of git(+rev) or
/// path per entry.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PluginSourceView {
    pub git: Option<String>,
    /// Pinned commit SHA for git sources (tags/branches are rejected by
    /// the sync layer).
    pub rev: Option<String>,
    /// Local directory installed as a symlink (development iterations).
    pub path: Option<String>,
    /// Development-workspace commit for a daemon-owned local snapshot.
    pub local_commit: Option<String>,
    /// Immutable tree digest for a daemon-owned local snapshot.
    pub local_digest: Option<String>,
}

/// `GET /api/v1/plugins/environments` — the host-approved, digest-pinned
/// base images available to plugin RSI.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PluginEnvironmentListView {
    pub environments: Vec<PluginEnvironmentView>,
}

/// One approved plugin environment.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PluginEnvironmentView {
    /// Stable environment id selected by the approving user.
    pub id: String,
    /// Digest-pinned image reference.
    pub reference: String,
    /// Interpreters the environment guarantees, such as `sh` or `Rscript`.
    pub interpreters: Vec<String>,
}

/// Docker Hub repository search results.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DockerHubSearchView {
    pub repositories: Vec<DockerHubRepositoryView>,
}

/// One Docker Hub repository discovered by a read-only search.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DockerHubRepositoryView {
    pub repository: String,
    pub description: String,
    pub official: bool,
    pub stars: i64,
    pub pulls: i64,
}

/// A Docker Hub tag resolved to an immutable manifest digest.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DockerHubTagView {
    pub repository: String,
    pub tag: String,
    pub digest: String,
    pub size_bytes: i64,
    pub last_pushed: String,
    /// Digest-pinned reference suitable for environment approval.
    pub reference: String,
}

/// User-approved addition to the plugin environment allow list.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApprovePluginEnvironmentRequest {
    pub repository: String,
    pub tag: String,
    pub environment_id: String,
    pub interpreters: Vec<String>,
    /// Frontend consent marker. The current approval stub allows every image
    /// request; the next PR will enforce this field and persist an audit record.
    #[serde(default = "default_true")]
    pub approved: bool,
}

fn default_true() -> bool {
    true
}
