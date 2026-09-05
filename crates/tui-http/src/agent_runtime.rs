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
use agentik_types::AgentPath;
use arc_swap::ArcSwapOption;
use axum::Json;
use axum::extract::{Path, State};
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
        .route("/threads/{id}", patch(rename_thread).delete(delete_thread))
        .route("/threads/{id}/messages", get(thread_messages))
        .route("/threads/{id}/chat", post(thread_chat))
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
    // Web surface: bibliography only. No shell, container, data-engine, or
    // host (spawn/delegate) tooling over HTTP.
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
