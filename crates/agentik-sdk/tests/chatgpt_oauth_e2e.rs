//! ChatGPT 订阅 OAuth 端到端测试：mock issuer + mock ChatGPT 后端 + 真实
//! HTTP 栈（reqwest/wire/Model），不走任何 mock client。
//!
//! 覆盖三条链路：
//! 1. `login_flow_with_issuer` → 伪浏览器回调（注入 code+state）→ mock
//!    issuer 换 token → `TokenBlob` claims 正确；
//! 2. 真实 `Model::new`（blob JSON 作 api_key）：请求头
//!    （Bearer / chatgpt-account-id / OpenAI-Beta / originator）、
//!    Responses 请求体、非流式响应解码；
//! 3. 401 自愈（非流式 + 流式）：过期 access token → mock 后端 401 →
//!    mock issuer 刷新（断言请求形状）→ 槽位热更 → 重试携带新 Bearer
//!    成功 → 刷新回调上报新 blob JSON。
//!
//! HTTP stub 为手写 `std::net::TcpListener`（每连接一线程，Connection:
//! close），避免为测试引入 httpmock。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agentik_sdk::model::oauth_context::OAuthContext;
use agentik_sdk::model::{Model, ModelInfo, ProviderConfig, ProviderType};
use agentik_sdk::provider::ProviderPreset;
use agentik_sdk::provider::client::AnthropicApiClient;
use agentik_sdk::provider::openai::oauth::{TokenBlob, login_flow_with_issuer};
use agentik_sdk::provider::openai::{MODEL_GPT_6_ASTRA, OpenaiProvider};
use agentik_sdk::{Anthropic, AuthMethod, ClientConfig, WireProtocolKind, build_wire};
use agentik_types::messages::{ContentBlock, Message, Role};
use base64::Engine as _;
use sha2::Digest as _;

/// mock issuer 签发的 id_token：claims 含 `org-e2e` / `plus` /
/// `e2e@example.com`（alg=none，解析端不验签）。
const ID_TOKEN: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJlbWFpbCI6ImUyZUBleGFtcGxlLmNvbSIsImh0dHBzOi8vYXBpLm9wZW5haS5jb20vYXV0aCI6eyJjaGF0Z3B0X3BsYW5fdHlwZSI6InBsdXMiLCJjaGF0Z3B0X3VzZXJfaWQiOiJ1c2VyLWUyZS0xIiwiY2hhdGdwdF9hY2NvdW50X2lkIjoib3JnLWUyZSJ9fQ.sig";
const EXPIRED_ACCESS: &str = "expired-access";
const FRESH_ACCESS: &str = "fresh-access";

// ─── 手写 HTTP stub ─────────────────────────────────────────────────────────

/// 一条收到的请求（名字小写、body 完整读入）。
#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        let lower = name.to_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == lower)
            .map(|(_, v)| v.as_str())
    }
}

type Handler = Arc<dyn Fn(&Recorded) -> (u16, String, String) + Send + Sync>;

struct MockServer {
    url: String,
    log: Arc<Mutex<Vec<Recorded>>>,
}

impl MockServer {
    fn requests(&self, path: &str) -> Vec<Recorded> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == path)
            .cloned()
            .collect()
    }
}

/// 起一个单用途 HTTP 服务器：accept 循环线程 + 每连接一个处理线程。
/// 请求交给 `handler`，返回 `(status, content_type, body)`；响应恒带
/// `Connection: close`，连接随即关闭。
fn spawn_mock(name: &str, handler: Handler) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock listener");
    let addr = listener.local_addr().unwrap();
    let log: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
    let wrapped = {
        let log = log.clone();
        move |req: Recorded| -> (u16, String, String) {
            log.lock().unwrap().push(req.clone());
            handler(&req)
        }
    };
    std::thread::Builder::new()
        .name(format!("mock-{name}"))
        .spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut stream) = conn else { continue };
                let handler = wrapped.clone();
                std::thread::spawn(move || {
                    let Some(req) = read_request(&mut stream) else {
                        return;
                    };
                    let (status, content_type, body) = handler(req);
                    write_response(&mut stream, status, &content_type, &body);
                });
            }
        })
        .expect("spawn mock thread");
    MockServer {
        url: format!("http://127.0.0.1:{}", addr.port()),
        log,
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next()?.to_string();
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_lowercase();
            let v = v.trim().to_string();
            if k == "content-length" {
                content_length = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }
    while buf.len() < header_end + 4 + content_length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body =
        String::from_utf8_lossy(&buf[header_end + 4..header_end + 4 + content_length]).to_string();
    Some(Recorded {
        method,
        path,
        headers,
        body,
    })
}

fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

fn form_get(body: &str, key: &str) -> String {
    url::form_urlencoded::parse(body.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

// ─── mock issuer / mock 后端 ────────────────────────────────────────────────

struct IssuerMock {
    server: MockServer,
    refresh_count: Arc<AtomicUsize>,
}

/// mock `auth.openai.com`：`POST /oauth/token` 两种 grant——
/// authorization_code（form，登录交换）/ refresh_token（JSON，自愈刷新）。
fn spawn_issuer() -> IssuerMock {
    let refresh_count = Arc::new(AtomicUsize::new(0));
    let counter = refresh_count.clone();
    let server = spawn_mock(
        "issuer",
        Arc::new(move |req: &Recorded| {
            if req.method != "POST" || req.path != "/oauth/token" {
                return (404, "text/plain".into(), "not found".into());
            }
            let is_json = req
                .header("content-type")
                .is_some_and(|v| v.contains("application/json"));
            let grant = if is_json {
                serde_json::from_str::<serde_json::Value>(&req.body)
                    .ok()
                    .and_then(|v| {
                        v.get("grant_type")
                            .and_then(|g| g.as_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_default()
            } else {
                form_get(&req.body, "grant_type")
            };
            match grant.as_str() {
                "authorization_code" => (
                    200,
                    "application/json".into(),
                    format!(
                        r#"{{"id_token":"{ID_TOKEN}","access_token":"{FRESH_ACCESS}","refresh_token":"rotated-refresh"}}"#
                    ),
                ),
                "refresh_token" => {
                    counter.fetch_add(1, Ordering::SeqCst);
                    (
                        200,
                        "application/json".into(),
                        format!(
                            r#"{{"id_token":"{ID_TOKEN}","access_token":"{FRESH_ACCESS}","refresh_token":"rotated-refresh-2"}}"#
                        ),
                    )
                }
                _ => (
                    400,
                    "application/json".into(),
                    r#"{"error":"unsupported_grant_type"}"#.into(),
                ),
            }
        }),
    );
    IssuerMock {
        server,
        refresh_count,
    }
}

const BACKEND_PATH: &str = "/backend-api/codex/responses";

/// mock ChatGPT 后端：`Bearer expired-access` → 401；`Bearer fresh-access`
/// → 200。`sse` 切换 200 响应形态（Responses JSON / SSE 事件流）。
fn spawn_backend(sse: bool) -> MockServer {
    spawn_mock(
        "backend",
        Arc::new(move |req: &Recorded| {
            if req.method != "POST" || req.path != BACKEND_PATH {
                return (404, "text/plain".into(), "not found".into());
            }
            match req.header("authorization") {
                Some(auth) if auth == format!("Bearer {EXPIRED_ACCESS}") => (
                    401,
                    "application/json".into(),
                    r#"{"error":{"message":"token expired"}}"#.into(),
                ),
                Some(auth) if auth == format!("Bearer {FRESH_ACCESS}") => {
                    if sse {
                        (
                            200,
                            "text/event-stream".into(),
                            concat!(
                                "event: response.created\n",
                                "data: {\"response\":{\"id\":\"resp_e2e\",\"model\":\"gpt-6-astra\"}}\n\n",
                                "event: response.output_item.added\n",
                                "data: {\"item\":{\"type\":\"message\",\"id\":\"msg_1\"}}\n\n",
                                "event: response.output_text.delta\n",
                                "data: {\"delta\":\"Hello from stream\"}\n\n",
                                "event: response.completed\n",
                                "data: {\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\n",
                            )
                            .into(),
                        )
                    } else {
                        (
                            200,
                            "application/json".into(),
                            r#"{"id":"resp_e2e","model":"gpt-6-astra","output":[{"type":"message","id":"msg_1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"Hello from mock backend"}]}],"usage":{"input_tokens":3,"output_tokens":5}}"#.into(),
                        )
                    }
                }
                _ => (
                    401,
                    "application/json".into(),
                    r#"{"error":{"message":"unexpected bearer"}}"#.into(),
                ),
            }
        }),
    )
}

// ─── 测试公用 ────────────────────────────────────────────────────────────────

fn model_info() -> ModelInfo {
    let mut info = OpenaiProvider::preset_models()
        .into_iter()
        .find(|m| m.model_name == MODEL_GPT_6_ASTRA)
        .expect("preset contains gpt-6-astra");
    info.provider_id = uuid::Uuid::nil();
    info
}

fn user_msg(text: &str) -> Message {
    Message {
        id: String::new(),
        type_: "message".to_string(),
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        model: None,
        stop_reason: None,
        stop_sequence: None,
        usage: None,
        request_id: None,
    }
}

fn text_of(msg: &Message) -> String {
    msg.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// 伪浏览器：向回调端口发 `GET /auth/callback?code&state`（与
/// `send_cancel` 同款裸 TcpStream 手法）。
fn browser_callback(port: u16, code: &str, state: &str) {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).expect("connect callback");
    let req = format!(
        "GET /auth/callback?code={code}&state={state} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = String::new();
    let _ = stream.read_to_string(&mut buf);
    assert!(
        buf.starts_with("HTTP/1.1 200"),
        "callback must accept: {buf}"
    );
}

// ─── 测试 1：登录全流程 → blob ──────────────────────────────────────────────

#[tokio::test]
async fn login_flow_produces_blob_from_mock_issuer() {
    let issuer = spawn_issuer();
    let (authorize_url, waiter) =
        login_flow_with_issuer(&issuer.server.url).expect("login flow starts");

    // 授权 URL 形态：指向 mock issuer，state 可解析。
    let parsed = url::Url::parse(&authorize_url).unwrap();
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert_eq!(parsed.path(), "/oauth/authorize");
    let q: std::collections::HashMap<String, String> = parsed
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let state = q["state"].clone();
    // redirect_uri = "http://localhost:{port}/auth/callback"
    let port: u16 = q["redirect_uri"]
        .strip_prefix("http://localhost:")
        .and_then(|rest| rest.split('/').next())
        .unwrap()
        .parse()
        .unwrap();

    // 伪浏览器完成授权 → 回调。
    browser_callback(port, "e2e-auth-code", &state);

    let blob = waiter.wait().await.expect("login completes");
    assert_eq!(blob.access_token, FRESH_ACCESS);
    assert_eq!(blob.refresh_token, "rotated-refresh");
    assert_eq!(blob.account_id, "org-e2e");
    assert_eq!(blob.email.as_deref(), Some("e2e@example.com"));
    assert_eq!(blob.plan_type.as_deref(), Some("plus"));

    // 交换请求形状：form-urlencoded + 授权码 + PKCE verifier。
    let exchange = issuer.server.requests("/oauth/token");
    assert_eq!(exchange.len(), 1);
    let ex = &exchange[0];
    assert_eq!(form_get(&ex.body, "grant_type"), "authorization_code");
    assert_eq!(form_get(&ex.body, "code"), "e2e-auth-code");
    assert_eq!(
        form_get(&ex.body, "client_id"),
        "app_EMoamEEZ73f0CkXaXp7hrann"
    );
    let verifier = form_get(&ex.body, "code_verifier");
    assert!(!verifier.is_empty());
    // challenge 与 verifier 对应（S256）。
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    assert_eq!(q["code_challenge"], challenge);
    assert_eq!(issuer.refresh_count.load(Ordering::SeqCst), 0);
}

// ─── 测试 2：真实 Model::new → 请求头/请求体/解码 ───────────────────────────

#[tokio::test]
async fn model_new_sends_chatgpt_shaped_request() {
    let backend = spawn_backend(false);
    let blob = TokenBlob {
        access_token: FRESH_ACCESS.to_string(),
        refresh_token: "unused".to_string(),
        account_id: "org-e2e".to_string(),
        email: Some("e2e@example.com".to_string()),
        plan_type: Some("plus".to_string()),
        last_refresh: chrono::Utc::now(),
    };
    let provider = ProviderConfig {
        id: uuid::Uuid::nil(),
        name: "openai".to_string(),
        provider_type: ProviderType::Openai,
        base_url: backend.url.clone(),
        api_key: blob.to_json().unwrap(),
        auth_method: OpenaiProvider::default_auth_method(),
    };
    let model = Model::new(model_info(), &provider).expect("blob builds chatgpt model");
    assert!(model.is_chatgpt());

    let resp = model
        .request(vec![user_msg("hello-chatgpt")], &[])
        .await
        .expect("request succeeds");
    assert_eq!(text_of(&resp), "Hello from mock backend");
    assert_eq!(resp.usage.as_ref().unwrap().input_tokens, 3);
    assert_eq!(resp.usage.as_ref().unwrap().output_tokens, 5);

    // 请求头与请求体。
    let reqs = backend.requests(BACKEND_PATH);
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    assert_eq!(req.header("authorization"), Some("Bearer fresh-access"));
    assert_eq!(req.header("chatgpt-account-id"), Some("org-e2e"));
    assert_eq!(req.header("openai-beta"), Some("responses=experimental"));
    assert_eq!(req.header("originator"), Some("codex_cli_rs"));
    let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
    assert_eq!(body["model"], MODEL_GPT_6_ASTRA);
    assert!(
        req.body.contains("hello-chatgpt"),
        "input must carry the user text: {}",
        req.body
    );
}

// ─── 测试 3：401 自愈（非流式 + 流式） ──────────────────────────────────────

fn expired_blob() -> TokenBlob {
    TokenBlob {
        access_token: EXPIRED_ACCESS.to_string(),
        refresh_token: "seed-refresh".to_string(),
        account_id: "org-e2e".to_string(),
        email: Some("e2e@example.com".to_string()),
        plan_type: Some("plus".to_string()),
        last_refresh: chrono::Utc::now(),
    }
}

fn assert_heal_common(
    issuer: &IssuerMock,
    backend: &MockServer,
    callback_json: &Mutex<Option<String>>,
) {
    // issuer 收到一次 JSON 形状的刷新请求，用了旧 refresh_token。
    assert_eq!(issuer.refresh_count.load(Ordering::SeqCst), 1);
    let refreshes = issuer.server.requests("/oauth/token");
    assert_eq!(refreshes.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&refreshes[0].body).unwrap();
    assert_eq!(body["grant_type"], "refresh_token");
    assert_eq!(body["refresh_token"], "seed-refresh");
    assert_eq!(body["client_id"], "app_EMoamEEZ73f0CkXaXp7hrann");

    // 后端先见 expired（401），再见 fresh（成功）。
    let reqs = backend.requests(BACKEND_PATH);
    assert_eq!(reqs.len(), 2);
    assert_eq!(
        reqs[0].header("authorization"),
        Some("Bearer expired-access")
    );
    assert_eq!(reqs[1].header("authorization"), Some("Bearer fresh-access"));

    // 回调上报了新 blob JSON（含轮转后的 refresh_token）。
    let json = callback_json
        .lock()
        .unwrap()
        .clone()
        .expect("callback fired");
    let reported = TokenBlob::from_json(&json).unwrap();
    assert_eq!(reported.access_token, FRESH_ACCESS);
    assert_eq!(reported.refresh_token, "rotated-refresh-2");
}

#[tokio::test]
async fn request_heals_401_via_refresh_and_slot() {
    let issuer = spawn_issuer();
    let backend = spawn_backend(false);
    let callback_json: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let ctx = OAuthContext::new(expired_blob())
        .with_issuer(issuer.server.url.clone())
        .with_on_refreshed(agentik_sdk::model::RefreshCallback::new({
            let cb = callback_json.clone();
            move |json| *cb.lock().unwrap() = Some(json)
        }));
    let config = ClientConfig::new(EXPIRED_ACCESS, backend.url.clone())
        .with_auth_method(AuthMethod::Chatgpt {
            account_id: "org-e2e".to_string(),
        })
        .with_oauth_token_slot(Some(ctx.token_slot()));
    let anthropic = Anthropic::with_config_and_wire(
        config,
        build_wire(WireProtocolKind::ChatgptResponses).unwrap(),
    )
    .unwrap();
    let model = Model::with_client_and_oauth(model_info(), AnthropicApiClient::new(anthropic), ctx);

    let resp = model
        .request(vec![user_msg("heal-me")], &[])
        .await
        .expect("401 healed and retried");
    assert_eq!(text_of(&resp), "Hello from mock backend");
    assert_heal_common(&issuer, &backend, &callback_json);
}

#[tokio::test]
async fn stream_heals_401_via_refresh_and_slot() {
    let issuer = spawn_issuer();
    let backend = spawn_backend(true);
    let callback_json: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let ctx = OAuthContext::new(expired_blob())
        .with_issuer(issuer.server.url.clone())
        .with_on_refreshed(agentik_sdk::model::RefreshCallback::new({
            let cb = callback_json.clone();
            move |json| *cb.lock().unwrap() = Some(json)
        }));
    let config = ClientConfig::new(EXPIRED_ACCESS, backend.url.clone())
        .with_auth_method(AuthMethod::Chatgpt {
            account_id: "org-e2e".to_string(),
        })
        .with_oauth_token_slot(Some(ctx.token_slot()));
    let anthropic = Anthropic::with_config_and_wire(
        config,
        build_wire(WireProtocolKind::ChatgptResponses).unwrap(),
    )
    .unwrap();
    let model = Model::with_client_and_oauth(model_info(), AnthropicApiClient::new(anthropic), ctx);

    let final_msg = model
        .request_stream(vec![user_msg("heal-me-streaming")], &[])
        .await
        .expect("stream request established after heal")
        .final_message()
        .await
        .expect("stream completes");
    assert_eq!(text_of(&final_msg), "Hello from stream");
    // 注：流式 usage 不随 SSE 事件下发（现行 wire 设计，与 OpenAI Chat
    // wire 同款取舍——非流式路径才解码 usage）。
    assert_heal_common(&issuer, &backend, &callback_json);
}
