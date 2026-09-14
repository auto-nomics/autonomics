//! Wire protocol types shared by the gateway server and its clients.
//!
//! Design rule: wherever a runtime type is already serde-complete
//! (`AgentEvent`, `AgentInfo`, `AgentProfile`, …) it crosses the wire
//! as-is, so frontends reuse their existing typed event handlers verbatim.
//! Types that are *not* serde-complete get a `…View` mirror here
//! (`HostEvent` → [`HostEventView`]).

use std::collections::HashMap;

use agentik_core::AgentProfile;
use agentik_types::SessionInfo;
use runtime::control::{AgentInfo, AgentStatus};
use runtime::model_bootstrap::{ModelRow, ProviderRow};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatgptLoginInfo {
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

// ── Hydration ─────────────────────────────────────────────────────────

/// One frame of a `Lagged` notification: the subscriber fell behind the
/// broadcast buffer and must re-hydrate from `GET /state`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LagFrame {
    pub missed: u64,
    pub resume_seq: u64,
}

/// Cold-start snapshot for a freshly connected frontend. `last_seq` is read
/// from the hub *after* the rest of the snapshot is serialized, so applying
/// the snapshot and then all live frames with `seq > last_seq` (dedup by
/// seq) yields a consistent state with no gaps and no duplicates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub last_seq: u64,
    pub active_model_spec: Option<String>,
    pub profiles: Vec<AgentProfile>,
    /// Live registered agents (from `HostControl::get_status`).
    pub agents: Vec<AgentInfo>,
    /// Known per-agent session lists, folded by the driver from
    /// `AgentEvent::SessionList`. Absent until an agent has been asked
    /// (`POST /agents/{name}/sessions/list`) or emitted a list.
    pub sessions: HashMap<String, Vec<SessionInfo>>,
    pub display_settings: DisplaySettings,
    pub model_catalog: ModelCatalog,
}

/// Display toggles persisted in the daemon's `settings` table.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct DisplaySettings {
    pub collapse_thinking: bool,
    pub collapse_tool_calls: bool,
    pub collapse_tool_results: bool,
}

/// Model catalogue + credentials view over the daemon-owned app DB.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub providers: Vec<ProviderRow>,
    pub models: Vec<ModelRow>,
    /// Active default model as `provider_name:model_name`.
    pub active_model: Option<String>,
    /// Health of the openai ChatGPT subscription token, when present.
    pub openai_token: Option<OpenaiTokenState>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OpenaiTokenState {
    pub present: bool,
    pub expired: bool,
}

/// `GET /api/v1/gateway/status` — liveness probe for `ensure_running`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayStatus {
    pub pid: u32,
    pub version: String,
    pub uptime_secs: u64,
    pub last_seq: u64,
    pub agent_count: usize,
}

// ── Requests / responses ──────────────────────────────────────────────

/// `POST /api/v1/agents` — spawn from a full profile (covers both the
/// profile picker and the resume-from-storage flow, which may reconstruct
/// profiles that no longer exist in the profile store).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentRequest {
    /// Agent name segment (joined onto `parent_path`).
    pub name: String,
    /// Parent path as a string, e.g. `/root`.
    pub parent_path: String,
    pub profile: AgentProfile,
    /// Optional `provider:model` override; unresolvable specs fall back to
    /// the daemon's default model (same semantics as the TUI today).
    pub model_spec: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentResponse {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverMessageRequest {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetAgentModelRequest {
    pub spec: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModelInfoView {
    pub model: String,
    pub context_length: u64,
}

/// `GET /storage/agents/{id}/sessions` — persisted session metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredSession {
    pub id: Uuid,
    pub title: Option<String>,
    /// Unix epoch milliseconds.
    pub created_at: i64,
    /// Unix epoch milliseconds; equals `created_at` while still active.
    pub last_active: i64,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub title: Option<String>,
    pub fork_from: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameSessionRequest {
    pub title: String,
}

/// `PATCH /api/v1/storage/agents/{uuid}` — rename (new full path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameAgentRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveProviderRequest {
    pub name: String,
    pub provider_type: String,
    /// Empty → registry default for the provider type.
    pub base_url: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetActiveModelRequest {
    pub spec: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchCatalogRequest {
    /// Empty → registry default base URL.
    pub base_url: String,
}

/// `POST /api/v1/model-config/chatgpt/login` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatgptLoginStart {
    pub url: String,
}

/// `POST /api/v1/model-config/chatgpt/refresh` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatgptRefreshResponse {
    pub refreshed: bool,
}

/// `GET /api/v1/settings` / `PUT /api/v1/settings` body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsMap {
    pub settings: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
