//! Resident runtime-hosted web agents: thread ≡ agent session.
//!
//! The ephemeral module (`agent.rs`) builds a throwaway [`Agent`] per request;
//! this module is the P1 design in `docs/design/web-agent-runtime.md`: one
//! long-lived agent per `agent_type` (spawned through the same
//! `SharedInfra::spawn_agent` entrance the TUI uses), with each web thread
//! mapped onto one of its agentik **sessions**. Transcripts live in
//! `agent.db` (snapshot + WAL) instead of the frontend's `bib_meta` payload.
//!
//! Turn serialization: an agent has a single active session, so each turn
//! takes the agent's handle lock for its whole lifetime — including after the
//! SSE client disconnects, the driver keeps draining until the terminal event
//! so the next turn never observes stale events. A second concurrent turn on
//! the same agent gets 409 `agent_busy`.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agentik_core::storage::{AgentProfileRegistry, AgentStorage, StorageError};
use agentik_core::AgentProfile;
use agentik_sdk::model::Model;
use agentik_types::{AgentPath, ContentBlock, Message, Role};
use arc_swap::ArcSwapOption;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Router, http::header};
use futures::stream::Stream;
use runtime::host::{AgentHandle, SharedInfra};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::sync::Mutex;

use crate::agent::{Frame, Mapped, PING_INTERVAL, frame_to_event, map_agent_event, sse};

/// Agent types the frontend may address (`AgentType` in `agentTypes.ts`).
pub(crate) const AGENT_TYPES: &[&str] = &["homepage", "paperReader", "screening"];

// ─────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub(crate) struct RuntimeAgentState {
    registry: Arc<AgentRegistry>,
}

impl RuntimeAgentState {
    pub(crate) fn new(infra: SharedInfra, model: Arc<ArcSwapOption<Model>>) -> Self {
        Self {
            registry: Arc::new(AgentRegistry::new(infra, model)),
        }
    }
}

pub(crate) struct AgentRegistry {
    infra: SharedInfra,
    model: Arc<ArcSwapOption<Model>>,
    /// `agent_type` → resident agent. Populated lazily on first use.
    agents: Mutex<HashMap<String, Arc<ResidentAgent>>>,
}

struct ResidentAgent {
    /// The handle mutex doubles as the turn lock: held for the entire
    /// lifetime of one streaming turn (see module doc).
    handle: Mutex<AgentHandle>,
    /// Set while a turn is streaming; guards against concurrent turns on the
    /// same agent (its event stream has no per-turn demultiplexing).
    busy: AtomicBool,
}

impl AgentRegistry {
    fn new(infra: SharedInfra, model: Arc<ArcSwapOption<Model>>) -> Self {
        Self {
            infra,
            model,
            agents: Mutex::new(HashMap::new()),
        }
    }

    /// Snapshot of the resident agents currently held (P5a live rows for
    /// `GET /agents`), as `(agent_type, entry)` pairs.
    async fn resident_agents(&self) -> Vec<(String, Arc<ResidentAgent>)> {
        self.agents
            .lock()
            .await
            .iter()
            .map(|(agent_type, entry)| (agent_type.clone(), entry.clone()))
            .collect()
    }

    fn model_slot(&self) -> &Arc<ArcSwapOption<Model>> {
        &self.model
    }

    fn infra(&self) -> &SharedInfra {
        &self.infra
    }

    /// Fetch (or lazily spawn) the resident agent for an agent type.
    async fn get(&self, agent_type: &str) -> Result<Arc<ResidentAgent>, Response> {
        let mut agents = self.agents.lock().await;
        if let Some(entry) = agents.get(agent_type) {
            return Ok(entry.clone());
        }

        let profile = ensure_web_profile(&self.infra, agent_type).await?;
        // Resident web agents live in their own subtree (`/root/web/...`) so
        // their persistent records never collide with TUI-spawned agents and
        // they stay invisible to the TUI's agent picker.
        let path = AgentPath::root()
            .join("web")
            .and_then(|p| p.join(profile.name()))
            .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &format!("agent path: {e}")))?;
        let handle = self
            .infra
            .spawn_agent(&path, &profile, self.model.clone(), None)
            .await
            .map_err(|e| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("spawn agent failed: {e}"),
                )
            })?;

        let entry = Arc::new(ResidentAgent {
            handle: Mutex::new(handle),
            busy: AtomicBool::new(false),
        });
        agents.insert(agent_type.to_owned(), entry.clone());
        Ok(entry)
    }
}

/// Releases the busy flag when the driver finishes (normal end, agent death,
/// or task abort).
struct BusyGuard<'a>(&'a AtomicBool);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Router
// ─────────────────────────────────────────────────────────────────────────

pub(crate) fn router(state: RuntimeAgentState) -> Router {
    Router::new()
        .route("/threads", post(create_thread))
        .route("/threads/import", post(import_thread))
        .route("/threads/{id}", patch(rename_thread).delete(delete_thread))
        .route("/threads/{id}/messages", get(thread_messages))
        .route("/threads/{id}/chat", post(thread_chat))
        .route("/threads/{id}/compact", post(thread_compact))
        // P5a read-only observability (docs/design/web-agent-delegation.md §4)
        .route("/agents", get(list_agents))
        .route("/agents/history", get(agent_history))
        .route("/delegations", get(list_delegations))
        .with_state(state)
}

#[derive(Deserialize)]
struct CreateThreadRequest {
    agent_type: String,
    #[serde(default)]
    title: Option<String>,
    /// Fork (deep-clone) an existing session — the web "追问" inline thread.
    #[serde(default)]
    fork_from: Option<uuid::Uuid>,
}

async fn create_thread(
    State(state): State<RuntimeAgentState>,
    Json(request): Json<CreateThreadRequest>,
) -> Response {
    let entry = match state.registry.get(&request.agent_type).await {
        Ok(entry) => entry,
        Err(response) => return response,
    };
    let handle = entry.handle.lock().await;
    let thread_id = handle.create_session(request.title, request.fork_from);
    Json(json!({ "thread_id": thread_id })).into_response()
}

#[derive(Deserialize)]
struct RenameThreadRequest {
    agent_type: String,
    title: String,
}

// ── Import (legacy transcript migration) ─────────────────────────────────

/// One replayed turn from a legacy `bib_meta` transcript.
#[derive(Deserialize)]
struct ImportMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ImportThreadRequest {
    agent_type: String,
    #[serde(default)]
    title: Option<String>,
    messages: Vec<ImportMessage>,
}

/// Create a thread pre-seeded with an existing transcript.
///
/// The P4 migration path for conversations that lived in the frontend's
/// `bib_meta` blob: write the session row + WAL directly (durable before the
/// response returns, so an immediate `GET /messages` sees the transcript),
/// then [`AgentHandle::register_session`] so a *live* agent adopts the
/// session into its in-memory map — `switch_session` ignores sessions it
/// doesn't know, so storage alone is not enough.
async fn import_thread(
    State(state): State<RuntimeAgentState>,
    Json(request): Json<ImportThreadRequest>,
) -> Response {
    // Tolerant mapping, mirroring the retired ephemeral endpoint: only
    // user/assistant turns exist in a transcript; anything else is dropped.
    let messages: Vec<Message> = request
        .messages
        .iter()
        .filter(|entry| matches!(entry.role.as_str(), "user" | "assistant"))
        .map(|entry| Message {
            id: uuid::Uuid::new_v4().to_string(),
            type_: "message".to_owned(),
            role: match entry.role.as_str() {
                "user" => Role::User,
                _ => Role::Assistant,
            },
            content: vec![ContentBlock::Text {
                text: entry.content.clone(),
            }],
            model: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            request_id: None,
        })
        .collect();
    if messages.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "messages must contain at least one user/assistant turn",
        );
    }

    let entry = match state.registry.get(&request.agent_type).await {
        Ok(entry) => entry,
        Err(response) => return response,
    };
    let handle = entry.handle.lock().await;
    let thread_id = uuid::Uuid::new_v4();
    let storage = state.registry.infra().storage.clone();
    let result: Result<(), StorageError> = async {
        storage.start_session(handle.agent_id, thread_id).await?;
        for message in &messages {
            // Also writes the transcript table, so GET /messages serves the
            // imported turns without any extra step.
            storage.append_message(thread_id, message).await?;
        }
        storage.end_session(thread_id).await?;
        if let Some(title) = request.title.as_deref() {
            storage.update_session_title(thread_id, title).await?;
        }
        Ok(())
    }
    .await;
    if let Err(e) = result {
        return internal(&format!("import transcript failed: {e}"));
    }
    handle.register_session(thread_id);
    Json(json!({ "thread_id": thread_id })).into_response()
}

// ── Compact ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ThreadCompactRequest {
    agent_type: String,
}

/// Trigger server-side compaction on a thread (`AgentHandle::compact` acts
/// on the agent's *active* session, so switch first). Fire-and-forget: the
/// compaction itself runs inside the agent's serialized event loop — a turn
/// racing this request simply queues behind it.
async fn thread_compact(
    State(state): State<RuntimeAgentState>,
    Path(thread_id): Path<uuid::Uuid>,
    Json(request): Json<ThreadCompactRequest>,
) -> Response {
    if state.registry.model_slot().load().as_ref().is_none() {
        return error(StatusCode::SERVICE_UNAVAILABLE, "no model configured");
    }
    let entry = match state.registry.get(&request.agent_type).await {
        Ok(entry) => entry,
        Err(response) => return response,
    };
    let handle = entry.handle.lock().await;
    handle.switch_session(thread_id);
    handle.compact();
    Json(json!({ "ok": true })).into_response()
}


async fn rename_thread(
    State(state): State<RuntimeAgentState>,
    Path(thread_id): Path<uuid::Uuid>,
    Json(request): Json<RenameThreadRequest>,
) -> Response {
    let title = request.title.trim().to_owned();
    if title.is_empty() {
        return error(StatusCode::BAD_REQUEST, "title must not be empty");
    }
    let entry = match state.registry.get(&request.agent_type).await {
        Ok(entry) => entry,
        Err(response) => return response,
    };
    let handle = entry.handle.lock().await;
    handle.rename_session(thread_id, title);
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct DeleteThreadQuery {
    agent_type: String,
}

async fn delete_thread(
    State(state): State<RuntimeAgentState>,
    Path(thread_id): Path<uuid::Uuid>,
    axum::extract::Query(query): axum::extract::Query<DeleteThreadQuery>,
) -> Response {
    let entry = match state.registry.get(&query.agent_type).await {
        Ok(entry) => entry,
        Err(response) => return response,
    };
    let handle = entry.handle.lock().await;
    handle.close_session(thread_id);
    Json(json!({ "ok": true })).into_response()
}

async fn thread_messages(
    State(state): State<RuntimeAgentState>,
    Path(thread_id): Path<uuid::Uuid>,
) -> Response {
    let messages = match state.registry.infra().storage.get_transcript_messages(thread_id).await {
        Ok(messages) => messages,
        Err(StorageError::NotFound(_)) => {
            return error(StatusCode::NOT_FOUND, "unknown thread");
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("read transcript failed: {e}"),
            );
        }
    };
    let messages: Vec<_> = messages
        .iter()
        .map(|message| {
            let text = message
                .content
                .iter()
                .filter_map(|block| match block {
                    agentik_types::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            json!({
                "role": match message.role {
                    agentik_types::Role::User => "user",
                    agentik_types::Role::Assistant => "assistant",
                },
                "text": text,
            })
        })
        .collect();
    Json(json!({ "thread_id": thread_id, "messages": messages })).into_response()
}

#[derive(Deserialize)]
struct ThreadChatRequest {
    agent_type: String,
    message: String,
    /// Dynamic per-turn context (e.g. the currently open paper). Static
    /// personas live in the profile's system prompt instead.
    #[serde(default)]
    context: Option<String>,
}

async fn thread_chat(
    State(state): State<RuntimeAgentState>,
    Path(thread_id): Path<uuid::Uuid>,
    Json(request): Json<ThreadChatRequest>,
) -> Response {
    let message = request.message.trim().to_owned();
    if message.is_empty() {
        return error(StatusCode::BAD_REQUEST, "message must not be empty");
    }
    if state.registry.model_slot().load().as_ref().is_none() {
        return error(StatusCode::SERVICE_UNAVAILABLE, "no model configured");
    }
    let entry = match state.registry.get(&request.agent_type).await {
        Ok(entry) => entry,
        Err(response) => return response,
    };
    if entry.busy.swap(true, Ordering::SeqCst) {
        return error(
            StatusCode::CONFLICT,
            "agent busy: another turn is streaming for this assistant",
        );
    }

    let text = match request
        .context
        .as_deref()
        .map(str::trim)
        .filter(|context| !context.is_empty())
    {
        Some(context) => format!("【当前上下文】\n{context}\n\n{message}"),
        None => message,
    };

    let (tx, rx) = mpsc::channel::<Frame>(64);
    let registry = state.registry.clone();
    tokio::spawn(async move {
        let _busy = BusyGuard(&entry.busy);
        let mut handle = entry.handle.lock().await;

        // `spawn_agent` snapshots the model slot at spawn time; re-sync every
        // turn so TUI-side model swaps apply from the next request on, the
        // same live-tracking the ephemeral endpoint provides.
        if let Some(model) = registry.model_slot().load_full().as_deref() {
            handle.set_model(model.clone());
        }

        // Drop events queued between turns (e.g. late background-tool
        // completions) so the stream opens on this turn's events.
        while handle.poll_event().is_some() {}

        handle.switch_session(thread_id);
        handle.send_message(text);

        let mut pending_tools = VecDeque::new();
        while let Some(event) = handle.recv_event().await {
            match map_agent_event(event, &mut pending_tools) {
                Mapped::Continue(frame) => {
                    if let Some(frame) = frame {
                        // A send error means the client went away mid-turn;
                        // keep draining so the turn completes and the lock
                        // releases cleanly.
                        let _ = tx.send(frame).await;
                    }
                }
                Mapped::Finished(frame) => {
                    let _ = tx.send(frame).await;
                    break;
                }
            }
        }
        // `tx` and the handle guard drop here: the relay stream ends and the
        // turn lock releases only after the terminal event.
    });

    let mut ping = tokio::time::interval(PING_INTERVAL);
    // Consume the immediate first tick so the stream opens with real content.
    ping.tick().await;

    let stream = futures::stream::unfold(
        RelayState {
            rx,
            ping,
            finished: false,
        },
        |mut state| async move {
            if state.finished {
                return None;
            }
            let sse_event = tokio::select! {
                maybe_frame = state.rx.recv() => match maybe_frame {
                    Some(frame) => {
                        state.finished = matches!(frame.event, "done" | "error");
                        frame_to_event(frame)
                    }
                    // Driver ended without a terminal frame (agent task died
                    // or was cancelled); end the stream cleanly so the
                    // client's idle timer does not hang.
                    None => {
                        state.finished = true;
                        sse("done", json!({}))
                    }
                },
                _ = state.ping.tick() => sse("ping", json!({})),
            };
            Some((Ok::<_, std::convert::Infallible>(sse_event), state))
        },
    );

    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    response
}

struct RelayState {
    rx: mpsc::Receiver<Frame>,
    ping: tokio::time::Interval,
    finished: bool,
}

// ─────────────────────────────────────────────────────────────────────────
// P5a read-only observability: agents / delegations / history
// (docs/design/web-agent-delegation.md §4)
// ─────────────────────────────────────────────────────────────────────────

/// `GET /agents` — transparent host view from three sources: the host's
/// live registry (TUI-side agents), this module's resident web agents,
/// and the persisted agent graph; rows from previous runs (shut-down
/// children, orphans) join as `live:false`. No subtree filtering — the
/// web shell shares the host with the TUI and paths make the split
/// visible.
///
/// Resident web agents never enter the host registry: registering them
/// would move their event stream into the host's relay (the thread SSE
/// driver owns it), so their live rows are synthesized here instead.
async fn list_agents(State(state): State<RuntimeAgentState>) -> Response {
    let Some(control) = state.registry.infra().host_control.clone() else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "host control unavailable",
        );
    };
    let (Some(status), Some(persisted)) = (
        control.get_status().await,
        control.list_persisted_agents().await,
    ) else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "host event loop unavailable",
        );
    };

    let mut agents = Vec::with_capacity(status.agents.len() + persisted.len());
    let mut live_paths = std::collections::HashSet::new();
    for info in status.agents {
        live_paths.insert(info.path.clone());
        agents.push(json!({
            "name": info.name,
            "path": info.path,
            "agent_id": info.agent_id,
            "summary": info.summary,
            "tags": info.tags,
            "tools": info.tools,
            "status": info.status.tag(),
            "last_event": info.last_event,
            "live": true,
        }));
    }
    for (_, entry) in state.registry.resident_agents().await {
        let handle = entry.handle.lock().await;
        let path = handle.path.as_str().to_owned();
        if !live_paths.insert(path.clone()) {
            continue;
        }
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        agents.push(json!({
            "name": name,
            "path": path,
            "agent_id": handle.agent_id,
            "status": if entry.busy.load(Ordering::SeqCst) { "running" } else { "idle" },
            "last_event": serde_json::Value::Null,
            "live": true,
        }));
    }
    for row in persisted {
        if live_paths.contains(&row.path) {
            continue;
        }
        let name = row.path.rsplit('/').next().unwrap_or(&row.path).to_owned();
        let status = serde_json::from_str::<runtime::control::AgentStatus>(&row.status_json)
            .map(|status| status.tag().to_owned())
            .unwrap_or_else(|_| "unknown".to_owned());
        agents.push(json!({
            "name": name,
            "path": row.path,
            "parent_path": row.parent_path,
            "profile_path": row.profile_path,
            "agent_id": row.agent_id,
            "status": status,
            "last_event": row.last_event,
            "live": false,
        }));
    }
    Json(json!({ "agents": agents })).into_response()
}

#[derive(Deserialize)]
struct DelegationsQuery {
    /// Optional terminal-state filter.
    status: Option<String>,
    /// Optional target-agent filter (full or short name).
    target: Option<String>,
    limit: Option<usize>,
}

/// `GET /delegations` — the delegation ledger, newest first. Host-wide view
/// (no caller filter): the web drawer is an operator surface, not an agent.
async fn list_delegations(
    State(state): State<RuntimeAgentState>,
    Query(query): Query<DelegationsQuery>,
) -> Response {
    const STATUSES: [&str; 5] = ["pending", "running", "completed", "interrupted", "failed"];
    if let Some(status) = &query.status {
        if !STATUSES.contains(&status.as_str()) {
            return error(
                StatusCode::BAD_REQUEST,
                "status must be one of pending|running|completed|interrupted|failed",
            );
        }
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let Some(control) = state.registry.infra().host_control.clone() else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "host control unavailable",
        );
    };
    let Some(mut delegations) = control
        .list_delegations(None, query.target.as_deref(), query.status.as_deref())
        .await
    else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "host event loop unavailable",
        );
    };
    delegations.truncate(limit);
    Json(json!({ "delegations": delegations })).into_response()
}

#[derive(Deserialize)]
struct AgentHistoryQuery {
    /// Full path (`/root/web/homepage`) or unique short name.
    agent: String,
    limit: Option<usize>,
}

/// `GET /agents/history?agent=…` — an agent's recent transcript. The agent
/// is a query param (not a path segment) so full paths with slashes need no
/// encoding games. Resident web agents are resolved through this module's
/// registry and read straight from storage (they are invisible to the host
/// registry; see [`list_agents`]); everything else goes through
/// `HostControl::agent_history` (live TUI agents + persisted-graph
/// fallback). Unknown agents resolve to an empty history, not an error.
async fn agent_history(
    State(state): State<RuntimeAgentState>,
    Query(query): Query<AgentHistoryQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(20).clamp(1, 100);

    let requested = query.agent.trim().trim_start_matches('/').to_owned();
    let mut resident = None;
    for (agent_type, entry) in state.registry.resident_agents().await {
        let handle = entry.handle.lock().await;
        let path = handle.path.as_str().trim_start_matches('/');
        // Full path (`root/web/homepage`), short path (`homepage`), or the
        // frontend agent_type key (`paperReader`) all address the agent.
        if path == requested
            || path.rsplit('/').next() == Some(requested.as_str())
            || agent_type == requested
        {
            resident = Some((handle.agent_id, handle.path.as_str().to_owned()));
            break;
        }
    }

    let history = if let Some((agent_id, path)) = resident {
        runtime::host::read_agent_history(
            state.registry.infra().storage.clone(),
            agent_id,
            path,
            limit,
        )
        .await
    } else {
        let Some(control) = state.registry.infra().host_control.clone() else {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "host control unavailable",
            );
        };
        let Some(history) = control.agent_history(&query.agent, limit).await else {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "host event loop unavailable",
            );
        };
        history
    };
    // Same {role, text} shape as GET /threads/:id/messages so the drawer can
    // reuse the transcript renderer.
    let messages: Vec<_> = history
        .messages
        .iter()
        .map(|message| {
            let text = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            json!({
                "role": match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                "text": text,
            })
        })
        .collect();
    Json(json!({
        "agent_path": history.agent_path,
        "agent_id": history.agent_id,
        "session_id": history.session_id,
        "messages": messages,
    }))
    .into_response()
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

// ─────────────────────────────────────────────────────────────────────────
// Web profiles
// ─────────────────────────────────────────────────────────────────────────

/// One built-in web profile spec. The identity line mirrors the old
/// `identity_for` one-liners; the system prompt is the frontend's persona
/// text (`DEFAULT_PERSONAS` in `agentTypes.ts`, v2 2026-06-28) moved
/// server-side so the persona has a single source of truth.
struct WebProfileSpec {
    /// Profile path (and registry key). Resident agent path is derived as
    /// `/root/` + this value.
    path: &'static str,
    description: &'static str,
    identity: &'static str,
    system_prompt: &'static str,
}

const CORE_PERSONA_RULES: &str = "\
【核心约束】
- 中文回复，保留英文专名不翻译（如 React、TypeScript、BERT、Transformer、GLUE）。
- **数据忠实**：引用具体数字、术语、模型名时严格按原文（如 80.5% accuracy、BERT-large 340M、Transformer encoder），不要改写或近似。
- **客观语言**：用流畅的中文学术语言，避免夸张修辞（如\"里程碑式\"\"开创性\"），除非原文用了类似措辞。
- **不确定时明确承认**（如\"原文未提及\"\"这一点我不确定\"），不要编造事实或数据。";

const HOMEPAGE_PROMPT: &str = "\
你是 Autonomics 的 AI 助手，能够回答各类问题、进行写作、分析、整理资料。

【场景能力】
- 通用问答：技术、学术、写作、生活常识均可。
- 文档处理：整理、归纳、改写文本。
- 资料查询：需要最新信息或外部资料时，主动调用当前会话可用的工具（如 PubMed 文献查询等），无需询问用户。具体可用工具见系统提示词的工具列表。

";

const PAPER_READER_PROMPT: &str = "\
你是一位严谨的学术研究助手，帮助用户深入理解论文内容。基于提供的论文信息（标题、摘要、章节导览、段落上下文）回答用户问题。

【场景能力】
- **忠于论文**：答案必须基于提供的论文内容；论文未涉及的问题明确说明\"论文未提及\"，不要基于通用知识编造。
- **章节定位**：当回答涉及论文具体内容时，标注章节出处（如 [Section 3.3 - Pre-training Tasks]）。
- **相关文献**：用户问到当前论文之外的相关文献（综述、同类研究、引用关系）时，可调用当前会话可用的文献检索工具（如 PubMed），具体工具见系统提示词的工具列表。

";

const SCREENING_PROMPT: &str = "\
你是学术论文筛选助手，帮助用户快速判断一批论文的价值、相关性和横向差异。

【场景能力】
- **横向对比**：当用户问\"哪篇更适合 X / 哪篇更新 / 哪篇效果更好\"时，明确列出对比维度（方法 / 数据集 / 关键贡献 / 局限），给出排序建议。
- **简洁判断**：每篇论文用 1-2 句话总结核心贡献，标注关键数字（accuracy、参数量、数据集规模）。
- **文献检索**：用户给关键词想找候选论文时，可调用当前会话可用的文献检索工具（如 PubMed），具体工具见系统提示词的工具列表。

";

impl WebProfileSpec {
    fn for_agent_type(agent_type: &str) -> Option<Self> {
        match agent_type {
            "homepage" => Some(Self {
                path: "web/homepage",
                description: "Web homepage assistant (bibliography tools only).",
                identity: "Autonomics 文献库助手（Web）：帮用户检索、管理与研读文献，可用 lit_search / lit_fetch 检索 PubMed、arXiv、OpenAlex、Crossref、Semantic Scholar，用 bib_save 保存文献到本地文献库。",
                system_prompt: HOMEPAGE_PROMPT,
            }),
            "paperReader" => Some(Self {
                path: "web/paper_reader",
                description: "Web paper-reading assistant (bibliography tools only).",
                identity: "Autonomics 阅读助手（Web）：围绕用户当前打开的文献回答与讨论，必要时可用文献工具检索补充资料。",
                system_prompt: PAPER_READER_PROMPT,
            }),
            "screening" => Some(Self {
                path: "web/screening",
                description: "Web screening assistant (bibliography tools only).",
                identity: "Autonomics 筛查助手（Web）：帮助用户快速判断文献是否纳入，给出简明依据。",
                system_prompt: SCREENING_PROMPT,
            }),
            _ => None,
        }
    }

    fn system_prompt(&self) -> String {
        format!("{}{}", self.system_prompt, CORE_PERSONA_RULES)
    }
}

/// Seed the web profile for an agent type if missing, then return it. An
/// existing row always wins — the profile registry is the single source of
/// truth, and user edits to `web/*` profiles are honored.
async fn ensure_web_profile(infra: &SharedInfra, agent_type: &str) -> Result<AgentProfile, Response> {
    let Some(spec) = WebProfileSpec::for_agent_type(agent_type) else {
        return Err(error(
            StatusCode::BAD_REQUEST,
            &format!(
                "unknown agent_type: {agent_type} (expected one of {AGENT_TYPES:?})"
            ),
        ));
    };
    let registry = infra.profile_storage.clone();
    if let Some(existing) = registry
        .get_profile_by_path(spec.path)
        .await
        .map_err(|e| internal(&format!("profile lookup failed: {e}")))?
    {
        return Ok(existing);
    }

    let mut profile = AgentProfile::new(spec.path);
    profile.description = spec.description.to_owned();
    profile.agent_identity = spec.identity.to_owned();
    profile.system_prompt = Some(spec.system_prompt());
    // Web surface: bibliography only. No shell, container, data-engine,
    // host (spawn/delegate), or KMS memory tooling over HTTP.
    profile.enable_bibliography = true;
    profile.enable_writing = false;
    profile.enable_opengwas = false;
    profile.enable_opentargets = false;
    profile.enable_gwascatalog = false;
    profile.enable_dag_history = false;
    profile.enable_vfs_shell = false;
    profile.enable_container_dev = false;
    profile.enable_data_engine = false;
    profile.enable_host_tools = false;
    profile.enable_kms_readonly = false;

    registry
        .create_profile(profile.clone())
        .await
        .map_err(|e| internal(&format!("profile seed failed: {e}")))?;
    Ok(profile)
}

fn internal(message: &str) -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_profile_specs_cover_all_agent_types() {
        for agent_type in AGENT_TYPES {
            let spec =
                WebProfileSpec::for_agent_type(agent_type).expect("known agent_type has a spec");
            assert!(
                spec.path.starts_with("web/"),
                "profile path {agent_type} must live under web/: {}",
                spec.path
            );
            // The resident agent path derives from the profile path's last
            // segment; both must satisfy the [a-z0-9_] path segment rule.
            let segment = spec.path.rsplit('/').next().unwrap();
            agentik_types::validate_segment(segment)
                .unwrap_or_else(|e| panic!("segment {segment} invalid: {e}"));
        }
        assert!(WebProfileSpec::for_agent_type("nope").is_none());
    }

    #[test]
    fn web_profile_prompts_end_with_core_rules() {
        for agent_type in AGENT_TYPES {
            let spec = WebProfileSpec::for_agent_type(agent_type).unwrap();
            let prompt = spec.system_prompt();
            assert!(
                prompt.ends_with(CORE_PERSONA_RULES),
                "{agent_type} prompt must carry the shared core rules"
            );
            assert!(!prompt.contains("网页搜索"), // homepage v2 persona dropped
                "{agent_type} prompt must not advertise tools the web profile does not register");
        }
    }
}

/// HTTP-level integration tests against a real `SharedInfra` (tempdir-backed
/// agent.db), covering the P4 surface: transcript import, per-thread chat
/// routing, and compaction.
#[cfg(test)]
mod http_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Scripted LLM client: fails every request with a *non-retryable* 4xx
    /// so the turn terminates immediately (agentik would otherwise burn its
    /// retry budget on retryable errors and slow the test down).
    struct FailingClient;

    #[async_trait::async_trait]
    impl agentik_sdk::provider::client::ApiClient for FailingClient {
        async fn request(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<agentik_types::ToolDefinition>,
            _model_info: &agentik_sdk::model::ModelInfo,
        ) -> Result<Message, agentik_types::errors::AnthropicError> {
            Err(agentik_types::errors::AnthropicError::HttpError {
                status: 400,
                message: "scripted failure".to_owned(),
            })
        }

        async fn request_stream(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<agentik_types::ToolDefinition>,
            _model_info: &agentik_sdk::model::ModelInfo,
        ) -> Result<agentik_sdk::streaming::MessageStream, agentik_types::errors::AnthropicError>
        {
            Err(agentik_types::errors::AnthropicError::HttpError {
                status: 400,
                message: "scripted failure".to_owned(),
            })
        }

        async fn test_connection(&self) -> Result<(), agentik_types::errors::AnthropicError> {
            Ok(())
        }
    }

    /// All derived paths (agent.db / dag-history.db / …) must land under
    /// the tempdir. Going through the builder matters: `Default` resolves
    /// them eagerly from `$HOME` / env, and mutating `state_dir` on the
    /// struct afterwards does NOT re-derive `agent_db` — tests written
    /// that way silently open the user's real `~/.autonomics/agent.db`.
    fn config(dir: &tempfile::TempDir) -> runtime::RuntimeConfig {
        runtime::RuntimeConfig::builder()
            .data_dir(dir.path().join("data"))
            .state_dir(dir.path().join("state"))
            .build()
    }

    fn json_request(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn body_text(response: axum::response::Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn import_replays_transcript_and_routes_turns_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = runtime::RuntimeHost::open(&config(&dir)).await.unwrap();
        let model: Arc<ArcSwapOption<Model>> = Arc::new(ArcSwapOption::from_pointee(
            Model::with_client(
                agentik_core::testing::dummy_model_info("test-model"),
                FailingClient,
            ),
        ));
        let app = router(RuntimeAgentState::new(host.infra(), model));

        // Import a legacy transcript (the P4 migration path).
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/threads/import",
                serde_json::json!({
                    "agent_type": "homepage",
                    "title": "migrated",
                    "messages": [
                        { "role": "user", "content": "旧问题" },
                        { "role": "assistant", "content": "旧回答" },
                    ],
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let thread_id: String = body_json(response).await["thread_id"]
            .as_str()
            .expect("thread_id")
            .to_owned();

        // The imported turns are immediately readable (durable before the
        // import response returned).
        let response = app
            .clone()
            .oneshot(Request::get(format!("/threads/{thread_id}/messages")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let messages = body_json(response).await["messages"].as_array().unwrap().clone();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["text"], "旧问题");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["text"], "旧回答");

        // A chat turn on the imported thread must land in THAT session —
        // this is the RegisterSession contract (switch_session ignores
        // sessions missing from the agent's in-memory map).
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/threads/{thread_id}/chat"),
                serde_json::json!({ "agent_type": "homepage", "message": "新消息" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let stream = body_text(response).await;
        assert!(stream.contains("event: error"), "stream: {stream}");
        assert!(stream.contains("scripted failure"), "stream: {stream}");

        // The WAL append of the injected user message is asynchronous; give
        // the persist worker a beat, then re-read.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let response = app
            .clone()
            .oneshot(Request::get(format!("/threads/{thread_id}/messages")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let messages = body_json(response).await["messages"].as_array().unwrap().clone();
        assert_eq!(
            messages.len(),
            3,
            "injected turn must land in the imported session: {messages:?}"
        );
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["text"], "新消息");

        host.shutdown_all_agents_and_wait().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn import_rejects_transcripts_without_replayable_turns() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = runtime::RuntimeHost::open(&config(&dir)).await.unwrap();
        let model: Arc<ArcSwapOption<Model>> = Arc::new(ArcSwapOption::from_pointee(None));
        let app = router(RuntimeAgentState::new(host.infra(), model));

        for (label, messages) in [
            ("empty", serde_json::json!([])),
            ("system-only", serde_json::json!([{ "role": "system", "content": "x" }])),
        ] {
            let response = app
                .clone()
                .oneshot(json_request(
                    "POST",
                    "/threads/import",
                    serde_json::json!({ "agent_type": "homepage", "messages": messages }),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "case {label}");
        }

        host.shutdown_all_agents_and_wait().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn chat_and_compact_guard_on_missing_model_and_blank_message() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = runtime::RuntimeHost::open(&config(&dir)).await.unwrap();
        // Empty model slot: every turn-bearing endpoint must refuse with 503.
        let model: Arc<ArcSwapOption<Model>> = Arc::new(ArcSwapOption::from_pointee(None));
        let app = router(RuntimeAgentState::new(host.infra(), model.clone()));

        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/threads",
                serde_json::json!({ "agent_type": "homepage" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "create_thread body: {body:?}");
        let thread_id: String = body["thread_id"]
            .as_str()
            .expect("thread_id")
            .to_owned();

        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/threads/{thread_id}/chat"),
                serde_json::json!({ "agent_type": "homepage", "message": "hi" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/threads/{thread_id}/compact"),
                serde_json::json!({ "agent_type": "homepage" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        // With a model wired, a blank message is rejected before any turn
        // starts, and compact answers ok.
        model.store(Some(Arc::new(Model::with_client(
            agentik_core::testing::dummy_model_info("test-model"),
            FailingClient,
        ))));
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/threads/{thread_id}/chat"),
                serde_json::json!({ "agent_type": "homepage", "message": "   " }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/threads/{thread_id}/compact"),
                serde_json::json!({ "agent_type": "homepage" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["ok"], true);

        host.shutdown_all_agents_and_wait().await;
    }

    /// P5a observability: /agents merges the live registry with the
    /// persisted graph (live wins, one row per path), /delegations exposes
    /// the ledger plus filter validation, /agents/history reads transcripts.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn observability_endpoints_expose_agents_and_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let host = runtime::RuntimeHost::open(&config(&dir)).await.unwrap();
        let model: Arc<ArcSwapOption<Model>> = Arc::new(ArcSwapOption::from_pointee(
            Model::with_client(
                agentik_core::testing::dummy_model_info("test-model"),
                FailingClient,
            ),
        ));
        let app = router(RuntimeAgentState::new(host.infra(), model));
        // The observability endpoints go through HostControl commands, which
        // only a driven host processes (see RuntimeHost::spawn_driver) — the
        // desktop shell runs the same driver.
        let driver = host.spawn_driver();

        // Untouched host: resident web agents spawn lazily, so both the
        // registry and the persisted graph are empty.
        let response = app
            .clone()
            .oneshot(Request::get("/agents").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "empty agents: {body:?}");
        assert_eq!(
            body["agents"].as_array().map(Vec::len),
            Some(0),
            "untouched host: {body:?}"
        );

        // Touching a thread spawns the resident agent; the graph row it
        // writes must dedupe against the live row (one entry, live:true).
        // Host-side registration is asynchronous (registration channel →
        // driver tick), so poll until the view settles.
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/threads",
                serde_json::json!({ "agent_type": "homepage" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "create thread: {body:?}");

        let agents = {
            let mut agents = Vec::new();
            for _ in 0..40 {
                let response = app
                    .clone()
                    .oneshot(Request::get("/agents").body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                let body = body_json(response).await;
                agents = body["agents"].as_array().cloned().unwrap_or_default();
                if !agents.is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            agents
        };
        assert_eq!(agents.len(), 1, "live + persisted dedup: {agents:?}");
        assert_eq!(agents[0]["path"], "/root/web/homepage");
        assert_eq!(agents[0]["live"], true);
        assert_eq!(agents[0]["status"], "idle");
        assert!(agents[0]["agent_id"].is_string());

        // Ledger is empty (no delegation has run) but the view is valid.
        let response = app
            .clone()
            .oneshot(Request::get("/delegations").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "empty ledger: {body:?}");
        assert_eq!(body["delegations"].as_array().map(Vec::len), Some(0));

        // Status filter is validated against the ledger's five states.
        let response = app
            .clone()
            .oneshot(
                Request::get("/delegations?status=bogus")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // History by full path: the just-created session exists but holds
        // no messages yet.
        let response = app
            .clone()
            .oneshot(
                Request::get("/agents/history?agent=/root/web/homepage")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "history by path: {body:?}");
        assert_eq!(body["agent_path"], "/root/web/homepage");
        assert_eq!(body["messages"].as_array().map(Vec::len), Some(0));

        // Unknown agents fall back to the persisted-graph path: an empty
        // history, not an error.
        let response = app
            .clone()
            .oneshot(
                Request::get("/agents/history?agent=nobody")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "unknown agent: {body:?}");
        assert_eq!(body["messages"].as_array().map(Vec::len), Some(0));

        let mut host = driver.join().await;
        host.shutdown_all_agents_and_wait().await;
    }
}
