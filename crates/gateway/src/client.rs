//! The gateway client used by every Rust frontend (the TUI, `tui serve
//! stop`, the future headless gateway runner, the desktop shell).
//!
//! Two layers:
//! - [`GatewayClient`]: one method per REST endpoint;
//! - [`EventPump`]: the SSE consumer — connects with `Last-Event-ID`,
//!   auto-reconnects, dedups by seq, and surfaces `lag` as a frame the
//!   application answers by re-hydrating from [`GatewayClient::state`].

use std::time::Duration;

use agentik_core::AgentProfile;
use agentik_types::{AgentEvent, AgentPlan};
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};
use runtime::control::AgentInfo;
use uuid::Uuid;

use crate::proto::*;

/// One SSE frame, after wire parsing.
#[derive(Debug, Clone)]
pub enum GatewayFrame {
    /// Raw SSE frames carry the seq; useful for dedup ordering.
    Sequenced { seq: u64, inner: FrameInner },
    /// The subscriber lagged — the application must re-hydrate
    /// (`GatewayClient::state`) before applying further frames.
    Lag { missed: u64, resume_seq: u64 },
    /// Connection to the daemon was lost (and will be retried). The
    /// application may show a disconnected indicator.
    Disconnected,
    /// Reconnected after a drop.
    Reconnected,
}

#[derive(Debug, Clone)]
pub enum FrameInner {
    Agent { agent: String, event: AgentEvent },
    Host(HostEventView),
    Notice(GatewayNotice),
}

#[derive(serde::Deserialize)]
struct AgentEnvelope {
    agent: String,
    event: AgentEvent,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("api error ({status}): {message}")]
    Api { status: u16, message: String },
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl From<reqwest::Error> for ClientError {
    fn from(error: reqwest::Error) -> Self {
        ClientError::Transport(error.to_string())
    }
}

impl ClientError {
    /// HTTP status for `Api` errors (for callers that branch on it).
    pub fn status(&self) -> Option<u16> {
        match self {
            ClientError::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;

/// Thin REST + SSE client. Clonable (shares one reqwest connection pool).
#[derive(Clone)]
pub struct GatewayClient {
    http: reqwest::Client,
    base: String,
}

impl GatewayClient {
    pub fn new(addr: &str, token: Option<&str>) -> Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(token) = token {
            let value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| ClientError::Transport(format!("invalid token: {e}")))?;
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            base: format!("http://{addr}/api/v1"),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl serde::Serialize>,
    ) -> Result<T> {
        let mut builder = self.http.request(method, self.url(path));
        if let Some(body) = body {
            builder = builder.json(body);
        }
        let response = builder.send().await?;
        let status = response.status();
        if !status.is_success() {
            let message = response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
                .unwrap_or_else(|| status.to_string());
            return Err(ClientError::Api {
                status: status.as_u16(),
                message,
            });
        }
        Ok(response.json::<T>().await?)
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(reqwest::Method::GET, path, None::<&serde_json::Value>)
            .await
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl serde::Serialize>,
    ) -> Result<T> {
        self.request(method, path, body).await
    }

    /// Fire-and-forget POST that only checks the status code.
    async fn fire(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl serde::Serialize>,
    ) -> Result<()> {
        let mut builder = self.http.request(method, self.url(path));
        if let Some(body) = body {
            builder = builder.json(body);
        }
        let response = builder.send().await?;
        let status = response.status();
        if !status.is_success() {
            let message = response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
                .unwrap_or_else(|| status.to_string());
            return Err(ClientError::Api {
                status: status.as_u16(),
                message,
            });
        }
        Ok(())
    }

    // ── daemon lifecycle ──

    pub async fn gateway_status(&self) -> Result<GatewayStatus> {
        self.get("/gateway/status").await
    }

    /// Probe variant: `Ok(None)` when the daemon is unreachable (connection
    /// refused / timeout / non-gateway responder).
    pub async fn gateway_status_opt(&self) -> Result<Option<GatewayStatus>> {
        match self
            .http
            .get(self.url("/gateway/status"))
            .timeout(Duration::from_secs(1))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => Ok(Some(response.json().await?)),
            Ok(_) => Ok(None),
            Err(error) if error.is_connect() || error.is_timeout() => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn shutdown_gateway(&self) -> Result<()> {
        self.fire(
            reqwest::Method::POST,
            "/gateway/shutdown",
            None::<&serde_json::Value>,
        )
        .await
    }

    // ── hydration ──

    pub async fn state(&self) -> Result<StateSnapshot> {
        self.get("/state").await
    }

    pub async fn profiles(&self) -> Result<Vec<AgentProfile>> {
        self.get("/profiles").await
    }

    // ── agents ──

    pub async fn list_agents(&self) -> Result<Vec<AgentInfo>> {
        self.get("/agents").await
    }

    /// Spawn an agent from a full profile. Returns the registered path.
    pub async fn spawn_agent(
        &self,
        name: &str,
        parent_path: &str,
        profile: &AgentProfile,
        model_spec: Option<&str>,
    ) -> Result<String> {
        let response: SpawnAgentResponse = self
            .send(
                reqwest::Method::POST,
                "/agents",
                Some(&SpawnAgentRequest {
                    name: name.to_string(),
                    parent_path: parent_path.to_string(),
                    profile: profile.clone(),
                    model_spec: model_spec.map(String::from),
                }),
            )
            .await?;
        Ok(response.path)
    }

    pub async fn deliver_message(&self, agent: &str, text: impl Into<String>) -> Result<()> {
        self.fire(
            reqwest::Method::POST,
            &format!("/agents/{}/messages", encode_segment(agent)),
            Some(&DeliverMessageRequest { text: text.into() }),
        )
        .await
    }

    pub async fn cancel_agent(&self, agent: &str) -> Result<()> {
        self.fire(
            reqwest::Method::POST,
            &format!("/agents/{}/cancel", encode_segment(agent)),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn compact_agent(&self, agent: &str) -> Result<()> {
        self.fire(
            reqwest::Method::POST,
            &format!("/agents/{}/compact", encode_segment(agent)),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn shutdown_agent(&self, agent: &str) -> Result<()> {
        self.fire(
            reqwest::Method::POST,
            &format!("/agents/{}/shutdown", encode_segment(agent)),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn agent_model_info(&self, agent: &str) -> Result<AgentModelInfoView> {
        self.get(&format!("/agents/{}/model", encode_segment(agent)))
            .await
    }

    /// Hot-swap an agent's model (also persists the preference).
    pub async fn set_agent_model(&self, agent: &str, spec: &str) -> Result<()> {
        self.fire(
            reqwest::Method::PUT,
            &format!("/agents/{}/model", encode_segment(agent)),
            Some(&SetAgentModelRequest {
                spec: spec.to_string(),
            }),
        )
        .await
    }

    pub async fn dag_snapshot(&self, agent: &str) -> Result<dag_core::dag::DagTuiSnapshot> {
        self.get(&format!("/agents/{}/dag", encode_segment(agent)))
            .await
    }

    // ── sessions ──

    pub async fn create_session(
        &self,
        agent: &str,
        title: Option<String>,
        fork_from: Option<Uuid>,
    ) -> Result<()> {
        self.fire(
            reqwest::Method::POST,
            &format!("/agents/{}/sessions", encode_segment(agent)),
            Some(&CreateSessionRequest { title, fork_from }),
        )
        .await
    }

    pub async fn switch_session(&self, agent: &str, session_id: Uuid) -> Result<()> {
        self.fire(
            reqwest::Method::POST,
            &format!(
                "/agents/{}/sessions/{session_id}/activate",
                encode_segment(agent)
            ),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn close_session(&self, agent: &str, session_id: Uuid) -> Result<()> {
        self.fire(
            reqwest::Method::POST,
            &format!(
                "/agents/{}/sessions/{session_id}/close",
                encode_segment(agent)
            ),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn rename_session(&self, agent: &str, session_id: Uuid, title: String) -> Result<()> {
        self.fire(
            reqwest::Method::PATCH,
            &format!(
                "/agents/{}/sessions/{session_id}/title",
                encode_segment(agent)
            ),
            Some(&RenameSessionRequest { title }),
        )
        .await
    }

    /// Request a fresh session list (arrives as `SessionList` agent frames
    /// on the event stream — same async contract as the runtime today).
    pub async fn list_sessions(&self, agent: &str) -> Result<()> {
        self.fire(
            reqwest::Method::GET,
            &format!("/agents/{}/sessions", encode_segment(agent)),
            None::<&serde_json::Value>,
        )
        .await
    }

    // ── storage ──

    pub async fn list_storage_agents(&self) -> Result<Vec<agentik_core::storage::AgentRecord>> {
        self.get("/storage/agents").await
    }

    pub async fn delete_agent_record(&self, agent_id: Uuid) -> Result<()> {
        self.fire(
            reqwest::Method::DELETE,
            &format!("/storage/agents/{agent_id}"),
            None::<&serde_json::Value>,
        )
        .await
    }

    pub async fn rename_agent_record(&self, agent_id: Uuid, new_path: &str) -> Result<()> {
        self.fire(
            reqwest::Method::PATCH,
            &format!("/storage/agents/{agent_id}"),
            Some(&RenameAgentRequest {
                path: new_path.to_string(),
            }),
        )
        .await
    }

    /// Merged session history (transcript + compacted summaries + live
    /// rows), exactly what the TUI used to assemble in-process.
    pub async fn session_history(
        &self,
        agent_id: Uuid,
        session_id: Uuid,
    ) -> Result<Vec<agentik_sdk::types::messages::Message>> {
        self.get(&format!("/agents/{agent_id}/sessions/{session_id}/history"))
            .await
    }

    pub async fn load_plan(&self, agent_id: Uuid) -> Result<Option<AgentPlan>> {
        self.get(&format!("/agents/{agent_id}/plan")).await
    }

    // ── model config ──

    pub async fn model_catalog(&self) -> Result<ModelCatalog> {
        self.get("/model-config").await
    }

    pub async fn save_provider(&self, request: &SaveProviderRequest) -> Result<()> {
        self.fire(
            reqwest::Method::PUT,
            "/model-config/provider",
            Some(request),
        )
        .await
    }

    /// Validate, persist, and activate the default model. On success the
    /// daemon broadcasts `ModelChanged`.
    pub async fn set_active_model(&self, spec: &str) -> Result<()> {
        self.fire(
            reqwest::Method::PUT,
            "/model-config/active-model",
            Some(&SetActiveModelRequest {
                spec: spec.to_string(),
            }),
        )
        .await
    }

    /// Start a ChatGPT login; the daemon completes it in the background
    /// and reports via a `ChatgptLogin` notice frame.
    pub async fn start_chatgpt_login(&self) -> Result<String> {
        let response: ChatgptLoginStart = self
            .send(
                reqwest::Method::POST,
                "/model-config/chatgpt/login",
                None::<&serde_json::Value>,
            )
            .await?;
        Ok(response.url)
    }

    pub async fn refresh_chatgpt(&self) -> Result<bool> {
        let response: ChatgptRefreshResponse = self
            .send(
                reqwest::Method::POST,
                "/model-config/chatgpt/refresh",
                None::<&serde_json::Value>,
            )
            .await?;
        Ok(response.refreshed)
    }

    pub async fn fetch_catalog(&self, provider: &str, base_url: &str) -> Result<()> {
        self.fire(
            reqwest::Method::POST,
            &format!(
                "/model-config/providers/{}/catalog",
                encode_segment(provider)
            ),
            Some(&FetchCatalogRequest {
                base_url: base_url.to_string(),
            }),
        )
        .await
    }

    // ── settings ──

    pub async fn settings(&self) -> Result<std::collections::HashMap<String, String>> {
        let map: SettingsMap = self.get("/settings").await?;
        Ok(map.settings)
    }

    pub async fn put_setting(&self, key: &str, value: &str) -> Result<()> {
        self.fire(
            reqwest::Method::PUT,
            "/settings",
            Some(&PutSettingRequest {
                key: key.to_string(),
                value: value.to_string(),
            }),
        )
        .await
    }

    // ── events ──

    /// Open the SSE stream. The returned response has its status checked
    /// (i.e. the server has accepted the subscription) but no frames
    /// consumed — the caller can then fetch `state()` for the snapshot
    /// and start consuming frames afterwards, applying only frames with
    /// `seq > snapshot.last_seq`.
    pub async fn connect_events(&self, last_event_id: u64) -> Result<reqwest::Response> {
        let response = self
            .http
            .get(self.url("/events"))
            .timeout(Duration::from_secs(10))
            .header("last-event-id", last_event_id.to_string())
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(ClientError::Api {
                status: status.as_u16(),
                message: status.to_string(),
            });
        }
        Ok(response)
    }
}

/// Percent-encode a path segment so agent paths containing `/` survive
/// URL routing (the encoded `%2F` does not split segments).
pub fn encode_segment(value: &str) -> String {
    const RESERVED: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
        .add(b'/')
        .add(b'%')
        .add(b' ')
        .add(b'#')
        .add(b'?');
    percent_encoding::utf8_percent_encode(value, RESERVED).to_string()
}

/// The SSE consumer task. Spawn via [`EventPump::spawn`]; it forwards
/// deduplicated [`GatewayFrame`]s into an unbounded channel until the
/// daemon connection drops permanently (channel closed / fatal error),
/// reconnecting with `Last-Event-ID` on transient failures.
pub struct EventPump;

impl EventPump {
    /// Spawn the pump on the current runtime. `client` is consumed.
    pub fn spawn(
        client: GatewayClient,
        mut last_seq: u64,
        tx: tokio::sync::mpsc::UnboundedSender<GatewayFrame>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut announced_disconnect = false;
            'outer: loop {
                let response = match client.connect_events(last_seq).await {
                    Ok(response) => response,
                    Err(error) => {
                        tracing::warn!(error = %error, "gateway event stream connect failed");
                        if !announced_disconnect {
                            let _ = tx.send(GatewayFrame::Disconnected);
                            announced_disconnect = true;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        continue 'outer;
                    }
                };
                if announced_disconnect {
                    let _ = tx.send(GatewayFrame::Reconnected);
                    announced_disconnect = false;
                }

                let mut stream = response.bytes_stream().eventsource();
                loop {
                    match stream.next().await {
                        Some(Ok(event)) => {
                            let Ok(seq) = event.id.parse::<u64>() else {
                                continue; // keepalive comments / unsequenced frames
                            };
                            if seq <= last_seq {
                                continue; // replay duplicate
                            }
                            let frame = match event.event.as_str() {
                                "agent" => serde_json::from_str::<AgentEnvelope>(&event.data)
                                    .ok()
                                    .map(|envelope| FrameInner::Agent {
                                        agent: envelope.agent,
                                        event: envelope.event,
                                    }),
                                "host" => serde_json::from_str::<HostEventView>(&event.data)
                                    .ok()
                                    .map(FrameInner::Host),
                                "notice" => serde_json::from_str::<GatewayNotice>(&event.data)
                                    .ok()
                                    .map(FrameInner::Notice),
                                "lag" => {
                                    let lag: Option<LagFrame> =
                                        serde_json::from_str(&event.data).ok();
                                    if let Some(lag) = lag {
                                        if tx
                                            .send(GatewayFrame::Lag {
                                                missed: lag.missed,
                                                resume_seq: lag.resume_seq,
                                            })
                                            .is_err()
                                        {
                                            return; // consumer gone
                                        }
                                    }
                                    continue;
                                }
                                _ => None,
                            };
                            let Some(inner) = frame else { continue };
                            last_seq = seq;
                            if tx.send(GatewayFrame::Sequenced { seq, inner }).is_err() {
                                return; // consumer gone
                            }
                        }
                        Some(Err(error)) => {
                            tracing::warn!(error = %error, "gateway event stream error; reconnecting");
                            break;
                        }
                        None => {
                            tracing::warn!("gateway event stream ended; reconnecting");
                            break;
                        }
                    }
                }
                // Stream broke — reconnect from the last applied seq.
                if !announced_disconnect {
                    let _ = tx.send(GatewayFrame::Disconnected);
                    announced_disconnect = true;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_paths_survive_segment_encoding() {
        assert_eq!(encode_segment("/root/worker"), "%2Froot%2Fworker");
        assert_eq!(encode_segment("plain"), "plain");
    }
}
