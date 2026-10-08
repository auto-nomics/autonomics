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
}

/// User-visible outcome of a ChatGPT login.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChatgptLoginInfo {
    pub email: Option<String>,
    pub plan_type: Option<String>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OpenaiTokenState {
    pub present: bool,
    pub expired: bool,
}

/// `GET /api/v1/gateway/status` — liveness probe for `ensure_running`.
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
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateSessionRequest {
    pub title: Option<String>,
    pub fork_from: Option<Uuid>,
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

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SetActiveModelRequest {
    pub spec: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FetchCatalogRequest {
    /// Empty → registry default base URL.
    pub base_url: String,
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

// ── Skill evolution ───────────────────────────────────────────────────

/// `GET /api/v1/skills/evolution/observations` — recorded evidence,
/// newest first. Includes the full reusable body for the TUI browser.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillObservationView {
    pub id: String,
    pub created_at: i64,
    pub kind: String,
    pub source: String,
    pub summary: String,
    pub body: String,
    pub node_kind: Option<String>,
    pub error: Option<String>,
}

/// `GET /api/v1/skills/evolution` — the dashboard's data snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillEvolutionStatus {
    /// Whether the background service (event + timer triggers) runs.
    pub service_enabled: bool,
    /// Library generation; advances on every mutation.
    pub generation: u64,
    /// Whether cycles may auto-approve (daemon configuration).
    pub auto_approve: bool,
    /// Auto-approve cap per cycle.
    pub max_approvals_per_cycle: usize,
    /// Periodic sweep cadence in seconds, when armed.
    pub sweep_interval_secs: Option<u64>,
    /// Recorded observations feeding distillation.
    pub observations: usize,
    /// Installed skills per tier (workspace > global > builtin).
    pub skills_workspace: usize,
    pub skills_global: usize,
    pub skills_builtin: usize,
    /// Skills carrying the loop's `auto` tag.
    pub skills_auto: usize,
    /// Proposals by status.
    pub proposals_pending: usize,
    pub proposals_approved: usize,
    pub proposals_rejected: usize,
    /// ── Live cycle monitor (flat mirror of skills::CycleStatus) ──
    /// Current worker phase: `idle` | `coalescing` | `distilling`.
    pub phase: String,
    /// Triggers accumulated in the quiet window (coalescing only).
    pub phase_queued: usize,
    /// Trigger labels the running cycle was caused by (distilling
    /// only).
    pub phase_triggers: Vec<String>,
    /// How long the current phase has held, in milliseconds.
    pub phase_elapsed_ms: u64,
    /// Cycles executed since service start.
    pub cycles_completed: u64,
    /// Unix time of the most recent cycle.
    pub last_cycle_at: Option<i64>,
    /// Wall time of the most recent cycle, in milliseconds.
    pub last_cycle_duration_ms: Option<u64>,
    /// Trigger labels of the most recent cycle.
    pub last_triggers: Vec<String>,
    /// Error text when the most recent cycle failed.
    pub last_error: Option<String>,
    /// Commands dropped because the channel was full — an
    /// observability signal, not an error.
    pub dropped_commands: u64,
    /// Cycles short-circuited because the observation pool was
    /// empty. Distinct from `cycles_completed`: the worker never
    /// ran a cycle body.
    pub cycle_skipped_empty: u64,
    /// Stable label for why the most recent cycle (or short-circuit)
    /// was skipped; `None` when the last cycle body actually ran.
    pub last_skipped_reason: Option<String>,
}

/// One proposal row for listings.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillProposalView {
    pub name: String,
    pub status: String,
    /// `distiller` (deterministic loop) or `agent` (LLM-authored via
    /// skill_propose; review-gated even under auto-approve).
    pub authored_by: String,
    pub update: bool,
    pub rationale: String,
    pub cluster_hash: String,
    pub observation_count: usize,
    pub created_at: i64,
}

/// `POST /api/v1/skills/evolution/trigger` request.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TriggerEvolutionRequest {
    /// Explicit per-call override of the review gate. `None` follows
    /// the daemon configuration; `Some(true)` is the TUI equivalent
    /// of `autonomics-skills distill --auto`.
    pub auto_approve: Option<bool>,
}

/// `POST /api/v1/skills/evolution/trigger` response — the cycle's
/// report (serializable form of the skills crate's
/// `EvolutionReport`).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillEvolutionReport {
    pub triggers: Vec<String>,
    pub clusters_considered: usize,
    pub proposals_written: Vec<String>,
    pub updated_existing: Vec<String>,
    pub auto_approved: Vec<String>,
    pub left_pending: usize,
    pub skipped: Vec<SkippedCluster>,
}

/// One skipped cluster with its reason.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkippedCluster {
    pub cluster_hash: String,
    pub reason: String,
}

/// `POST /api/v1/skills/evolution/proposals/{name}/approve` response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillApproveOutcome {
    pub name: String,
    pub destination: String,
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
    /// touched it (pending | approved | rejected).
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
    /// Manifest parse failure detail, when the family is broken.
    pub error: Option<String>,
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
