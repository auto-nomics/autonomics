use std::path::PathBuf;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const DEFAULT_ADDR: &str = "127.0.0.1:8765";
const MAX_BIB_UPLOAD_BYTES: u64 = 50 * 1024 * 1024;

pub struct GatewayState {
    http: reqwest::Client,
    base: std::sync::RwLock<String>,
    token: std::sync::RwLock<Option<String>>,
    event_task: Mutex<Option<JoinHandle<()>>>,
    requests: std::sync::Arc<crate::request_cancellation::Requests>,
}

impl GatewayState {
    pub fn new() -> Self {
        Self::for_endpoint(&env_addr(), read_token())
    }

    pub(crate) fn for_endpoint(addr: &str, token: Option<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .expect("build gateway HTTP client"),
            base: std::sync::RwLock::new(format!("http://{addr}/api/v1")),
            token: std::sync::RwLock::new(token),
            event_task: Mutex::new(None),
            requests: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeView {
    pub running: bool,
    pub addr: String,
    pub token_found: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiOk {
    pub ok: bool,
    pub data: Value,
}

fn env_token() -> Option<String> {
    std::env::var("AUTONOMICS_HTTP_API_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn env_addr() -> String {
    std::env::var("AUTONOMICS_HTTP_API_ADDR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ADDR.to_string())
}

fn token_path() -> PathBuf {
    let state_dir = std::env::var_os("AUTONOMICS_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".autonomics"))
        });
    state_dir
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gateway.token")
}

fn read_token() -> Option<String> {
    env_token().or_else(|| {
        std::fs::read_to_string(token_path())
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
    })
}

fn encoded_segment(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('/', "%2F")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('?', "%3F")
}

fn endpoint(base: &str, path: &str) -> String {
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

fn lock<T>(value: &std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    value.read().expect("gateway state lock poisoned")
}

#[tauri::command]
pub fn frontend_ready(stage: String, detail: Value) -> Result<(), String> {
    eprintln!("gui frontend ready stage={stage} detail={:?}", detail);
    Ok(())
}

#[tauri::command]
pub async fn probe(state: State<'_, GatewayState>) -> Result<ProbeView, String> {
    let base = lock(&state.base).clone();
    let token = lock(&state.token).clone();
    let running = state
        .http
        .get(endpoint(&base, "/gateway/status"))
        .bearer_auth(token.unwrap_or_default())
        .timeout(Duration::from_secs(1))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false);
    Ok(ProbeView {
        running,
        addr: env_addr(),
        token_found: read_token().is_some(),
    })
}

#[tauri::command]
pub async fn api(
    state: State<'_, GatewayState>,
    method: String,
    path: String,
    body: Option<Value>,
    request_id: Option<String>,
) -> Result<ApiOk, String> {
    api_request(&state, method, path, body, request_id).await
}
/// Long-lived deployment stream; credentials stay in Rust.
#[tauri::command]
pub async fn enroll_compute(
    state: State<'_, GatewayState>,
    request: Value,
    on_progress: tauri::ipc::Channel<Value>,
) -> Result<Value, String> {
    let base = lock(&state.base).clone();
    let token = lock(&state.token).clone();
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post(endpoint(&base, "compute/enroll/stream"))
        .bearer_auth(token.as_deref().unwrap_or_default())
        .json(&request)
        .send()
        .await
        .map_err(|error| format!("{error:?}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let message = response.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {message}"));
    }
    let mut stream = response.bytes_stream();
    let mut buffered = Vec::new();
    while let Some(chunk) = stream.next().await {
        buffered.extend_from_slice(
            &chunk.map_err(|error| format!("安装进度连接已断开，后台可能仍在执行：{error}"))?,
        );
        loop {
            let lf = buffered
                .windows(2)
                .position(|bytes| bytes == b"\n\n")
                .map(|index| (index, 2));
            let crlf = buffered
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
                .map(|index| (index, 4));
            let boundary = match (lf, crlf) {
                (Some(a), Some(b)) => Some(if a.0 < b.0 { a } else { b }),
                (a, b) => a.or(b),
            };
            let Some((index, size)) = boundary else { break };
            let frame = String::from_utf8(buffered.drain(..index + size).collect())
                .map_err(|error| error.to_string())?;
            let data = frame
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("data:")
                        .map(|data| data.strip_prefix(' ').unwrap_or(data))
                })
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() {
                continue;
            }
            let event: Value = serde_json::from_str(&data).map_err(|error| error.to_string())?;
            match event["type"].as_str() {
                Some("progress") => {
                    let _ = on_progress.send(event);
                }
                Some("result") => return Ok(event["state"].clone()),
                Some("error") => {
                    return Err(event["message"].as_str().unwrap_or("安装失败").into());
                }
                _ => return Err("无法识别的安装进度消息".into()),
            }
        }
        if buffered.len() > 1024 * 1024 {
            return Err("安装进度消息超过大小限制".into());
        }
    }
    Err("安装进度连接已断开，后台可能仍在执行。请刷新计算目标状态，确认结果后再重试。".into())
}

async fn api_request(
    state: &GatewayState,
    method: String,
    path: String,
    body: Option<Value>,
    request_id: Option<String>,
) -> Result<ApiOk, String> {
    let registration = request_id
        .as_deref()
        .map(|id| state.requests.start(id))
        .transpose()?;
    let operation = async {
        let base = lock(&state.base).clone();
        let token = lock(&state.token).clone();
        let path = path.trim_start_matches('/');
        let normalized_path = if path.starts_with("agents/") {
            match path.split_once('/') {
                Some(("agents", rest)) => match rest.split_once('/') {
                    Some((agent, tail)) => {
                        format!("agents/{}/{}", encoded_segment(agent), tail)
                    }
                    None => format!("agents/{}", encoded_segment(rest)),
                },
                _ => path.to_string(),
            }
        } else {
            path.to_string()
        };

        let verb = reqwest::Method::from_bytes(method.to_uppercase().as_bytes())
            .map_err(|error| format!("invalid HTTP method: {error}"))?;
        let mut request = state
            .http
            .request(verb, endpoint(&base, &normalized_path))
            .bearer_auth(token.unwrap_or_default());
        if let Some(id) = request_id.as_deref() {
            request = request.header("x-search-request-id", id);
            if method.eq_ignore_ascii_case("POST") && normalized_path == "bib/search/sessions" {
                request = request.header("x-discovery-session-id", id);
            }
        }
        // Structured PDF extraction runs the local Zotero layout models. Keep the
        // normal API deadline short while allowing a first Reading Mode conversion.
        if method.eq_ignore_ascii_case("POST") && normalized_path.ends_with("/fulltext/structured")
        {
            request = request.timeout(Duration::from_secs(210));
        }
        // Opening a project may pull its development image. Match the
        // gateway's 120-second startup budget instead of timing out at 30s.
        if method.eq_ignore_ascii_case("POST")
            && normalized_path.starts_with("plugin-projects/")
            && normalized_path
                .split('?')
                .next()
                .unwrap_or("")
                .ends_with("/container")
        {
            // Image build, startup, and up to five container lifecycle stages.
            request = request.timeout(Duration::from_secs(1500));
        }
        if matches!(&body, Some(value) if !value.is_null()) {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        let status = response.status();
        let payload = response.json::<Value>().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let fallback = status.to_string();
            return Err(payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or(fallback.as_str())
                .to_string());
        }
        Ok(ApiOk {
            ok: true,
            data: payload,
        })
    };
    if let Some((_guard, cancelled)) = registration {
        tokio::select! {biased;_ = cancelled=>Err("Request cancelled".into()),result=operation=>result}
    } else {
        operation.await
    }
}

#[tauri::command]
pub async fn cancel_api_request(
    state: State<'_, GatewayState>,
    request_id: String,
) -> Result<(), String> {
    state.requests.cancel(&request_id)?;
    let base = lock(&state.base).clone();
    let token = lock(&state.token).clone();
    let response = state
        .http
        .post(endpoint(&base, "bib/search/cancel"))
        .bearer_auth(token.unwrap_or_default())
        .json(&serde_json::json!({"request_id":request_id}))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Server cancellation rejected: {}",
            response.status()
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct WireFrame {
    seq: u64,
    kind: String,
    data: Value,
}

#[tauri::command]
pub async fn start_events(
    app: AppHandle,
    state: State<'_, GatewayState>,
    last_seq: u64,
) -> Result<bool, String> {
    let mut guard = state.event_task.lock().await;
    if let Some(task) = guard.as_ref() {
        if !task.is_finished() {
            return Ok(false);
        }
    }
    let base = lock(&state.base).clone();
    let token = lock(&state.token).clone();
    let sse_client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())?;
    let url = endpoint(&base, "events");
    let task = tokio::spawn(async move {
        let mut last_seq = last_seq;
        let mut announced_disconnect = false;
        loop {
            let response = match sse_client
                .get(&url)
                .header("last-event-id", last_seq.to_string())
                .bearer_auth(token.as_deref().unwrap_or_default())
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => response,
                _ => {
                    if !announced_disconnect {
                        let _ = app.emit("gateway-status", "disconnected");
                        announced_disconnect = true;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };
            if announced_disconnect {
                let _ = app.emit("gateway-status", "reconnected");
                announced_disconnect = false;
            }

            let mut stream = response.bytes_stream();
            let mut buffered = String::new();
            loop {
                match stream.next().await {
                    Some(Ok(chunk)) => {
                        buffered.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some((boundary, skip)) = event_boundary(&buffered) {
                            let raw_event = buffered[..boundary].to_string();
                            buffered.drain(..skip);
                            if let Some(frame) =
                                parse_sse(&raw_event).and_then(|(seq, kind, data)| {
                                    (seq > last_seq).then_some(WireFrame { seq, kind, data })
                                })
                            {
                                last_seq = frame.seq;
                                if app.emit("gateway-frame", frame).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    _ => break,
                }
            }
            if !announced_disconnect {
                let _ = app.emit("gateway-status", "disconnected");
                announced_disconnect = true;
            }
        }
    });
    *guard = Some(task);
    Ok(true)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BibUploadRequest {
    pub article_id: String,
    pub path: String,
}

#[tauri::command]
pub async fn upload_bib_fulltext(
    state: State<'_, GatewayState>,
    request: BibUploadRequest,
) -> Result<Value, String> {
    let path = PathBuf::from(request.path);
    let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("selected full text is not a file".to_string());
    }
    if metadata.len() > MAX_BIB_UPLOAD_BYTES {
        return Err(format!(
            "selected file exceeds the {} MiB upload limit",
            MAX_BIB_UPLOAD_BYTES / (1024 * 1024)
        ));
    }
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("selected file has an invalid name")?
        .to_owned();

    let base = lock(&state.base).clone();
    let token = lock(&state.token).clone();
    let url = endpoint(
        &base,
        &format!(
            "bib/articles/{}/fulltext",
            encoded_segment(&request.article_id)
        ),
    );
    let part = reqwest::multipart::Part::file(&path)
        .await
        .map_err(|error| format!("read selected file: {error}"))?
        .file_name(filename);
    let form = reqwest::multipart::Form::new().part("file", part);
    let response = state
        .http
        .post(url)
        .bearer_auth(token.as_deref().unwrap_or_default())
        .multipart(form)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let payload = response.json::<Value>().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let fallback = status.to_string();
        return Err(payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(fallback.as_str())
            .to_string());
    }
    Ok(payload)
}

#[derive(Debug, Serialize)]
pub struct BibBinaryView {
    pub data_base64: String,
    pub content_type: String,
}

#[tauri::command]
pub async fn bib_fulltext_raw(
    state: State<'_, GatewayState>,
    article_id: String,
    attachment_key: Option<String>,
    expected_hash: Option<String>,
) -> Result<BibBinaryView, String> {
    fulltext_request(&state, article_id, attachment_key, expected_hash).await
}
async fn fulltext_request(
    state: &GatewayState,
    article_id: String,
    attachment_key: Option<String>,
    expected_hash: Option<String>,
) -> Result<BibBinaryView, String> {
    let base = lock(&state.base).clone();
    let token = lock(&state.token).clone();
    let url = endpoint(
        &base,
        &match attachment_key {
            Some(key) => format!(
                "bib/articles/{}/attachments/{}/raw",
                encoded_segment(&article_id),
                encoded_segment(&key)
            ),
            None => format!("bib/articles/{}/fulltext/raw", encoded_segment(&article_id)),
        },
    );
    let mut request = state
        .http
        .get(url)
        .bearer_auth(token.as_deref().unwrap_or_default());
    if let Some(hash) = expected_hash {
        request = request.query(&[("expected_hash", hash)]);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let bytes = if status.is_success() {
        response.bytes().await.map_err(|error| error.to_string())?
    } else {
        let payload = response.json::<Value>().await.unwrap_or(Value::Null);
        let fallback = status.to_string();
        return Err(payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(fallback.as_str())
            .to_string());
    };
    Ok(BibBinaryView {
        data_base64: BASE64.encode(bytes),
        content_type,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BibExportSaveRequest {
    pub path: String,
    pub contents: String,
}

#[tauri::command]
pub fn save_bib_export(request: BibExportSaveRequest) -> Result<usize, String> {
    if request.path.trim().is_empty() {
        return Err("export path is required".to_string());
    }
    let size = request.contents.len();
    std::fs::write(&request.path, request.contents)
        .map_err(|error| format!("write export: {error}"))?;
    Ok(size)
}

fn parse_sse(raw: &str) -> Option<(u64, String, Value)> {
    let mut id = None;
    let mut kind = None;
    let mut data = String::new();
    for line in raw.lines() {
        if let Some(value) = line.strip_prefix("id: ") {
            id = value.trim().parse::<u64>().ok();
        } else if let Some(value) = line.strip_prefix("event: ") {
            kind = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data: ") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value);
        }
    }
    let kind = kind?;
    let data = serde_json::from_str::<Value>(&data).ok()?;
    Some((id?, kind, data))
}

fn event_boundary(value: &str) -> Option<(usize, usize)> {
    let bytes = value.as_bytes();
    for index in (0..bytes.len()).filter(|index| bytes[*index] == b'\n' || bytes[*index] == b'\r') {
        if bytes[index] == b'\n' && bytes.get(index + 1) == Some(&b'\n') {
            return Some((index, index + 2));
        }
        if bytes[index] == b'\r'
            && bytes.get(index + 1) == Some(&b'\n')
            && bytes.get(index + 2) == Some(&b'\r')
            && bytes.get(index + 3) == Some(&b'\n')
        {
            return Some((index, index + 4));
        }
    }
    None
}

pub fn install(app: &mut tauri::App) {
    app.manage(GatewayState::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sequenced_agent_frames() {
        let raw = "id: 17\nevent: agent\ndata: {\"agent\":\"/root/a\",\"event\":\"Done\"}";
        let (seq, kind, data) = parse_sse(raw).expect("frame parses");
        assert_eq!(seq, 17);
        assert_eq!(kind, "agent");
        assert_eq!(data["agent"], "/root/a");
        assert_eq!(data["event"], "Done");
    }

    #[test]
    fn rejects_unsequenced_frames() {
        assert!(parse_sse("event: ping\ndata: {}").is_none());
    }

    #[test]
    fn finds_lf_and_crlf_boundaries() {
        assert_eq!(event_boundary("id: 1\n\n"), Some((5, 7)));
        assert_eq!(event_boundary("id: 1\r\n\r\n"), Some((5, 9)));
        assert_eq!(event_boundary("id: 1\n"), None);
    }
}

#[cfg(test)]
mod search_transport_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn request_head(stream: &mut tokio::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let count = stream.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8(bytes).unwrap()
    }
    fn state(addr: std::net::SocketAddr) -> GatewayState {
        let state = GatewayState::new();
        *state.base.write().unwrap() = format!("http://{addr}/api/v1");
        *state.token.write().unwrap() = Some("fixture-token".into());
        state
    }
    #[tokio::test]
    async fn discovery_creation_forwards_client_known_session_id() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let state = state(listener.local_addr().unwrap());
        let session_id = "88f1c823-35ab-45e8-a283-545919c2f619";
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let head = request_head(&mut stream).await;
            assert!(head.starts_with("POST /api/v1/bib/search/sessions HTTP/1.1"));
            assert!(
                head.to_ascii_lowercase()
                    .contains(&format!("x-discovery-session-id: {session_id}"))
            );
            let body = "{\"session\":{\"id\":\"88f1c823-35ab-45e8-a283-545919c2f619\"}}";
            stream.write_all(format!("HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        });
        let result = api_request(
            &state,
            "POST".into(),
            "/bib/search/sessions".into(),
            Some(serde_json::json!({"query":"gene"})),
            Some(session_id.into()),
        )
        .await
        .unwrap();
        assert_eq!(result.data["session"]["id"], session_id);
        server.await.unwrap();
    }
    #[tokio::test]
    async fn native_search_cancellation_drops_the_real_http_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let state = std::sync::Arc::new(state(listener.local_addr().unwrap()));
        let (started, ready) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let head = request_head(&mut stream).await;
            assert!(head.starts_with("GET /api/v1/bib/search/status HTTP/1.1"));
            assert!(
                head.to_ascii_lowercase()
                    .contains("authorization: bearer fixture-token")
            );
            assert!(
                head.to_ascii_lowercase()
                    .contains("x-search-request-id: native-slow")
            );
            let _ = started.send(());
            let mut byte = [0];
            let count = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut byte))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(count, 0, "cancel must close the pending HTTP request");
        });
        let client_state = state.clone();
        let client = tokio::spawn(async move {
            api_request(
                &client_state,
                "GET".into(),
                "/bib/search/status".into(),
                None,
                Some("native-slow".into()),
            )
            .await
        });
        ready.await.unwrap();
        state.requests.cancel("native-slow").unwrap();
        assert_eq!(client.await.unwrap().unwrap_err(), "Request cancelled");
        server.await.unwrap();
    }
    #[tokio::test]
    async fn native_binary_fetch_targets_the_requested_attachment_and_hash() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let state = state(listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let head = request_head(&mut stream).await;
            assert!(head.starts_with("GET /api/v1/bib/articles/doi:10%2Fitem/attachments/SECOND/raw?expected_hash=expected HTTP/1.1"));
            assert!(
                head.to_ascii_lowercase()
                    .contains("authorization: bearer fixture-token")
            );
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbody").await.unwrap();
        });
        let result = fulltext_request(
            &state,
            "doi:10/item".into(),
            Some("SECOND".into()),
            Some("expected".into()),
        )
        .await
        .unwrap();
        assert_eq!(result.content_type, "text/plain");
        assert_eq!(BASE64.decode(result.data_base64).unwrap(), b"body");
        server.await.unwrap();
    }
}

// ── VFS browser binary bridge ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VfsDownloadRequest {
    pub path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VfsUploadRequest {
    /// Destination virtual path (dir + file name).
    pub path: String,
    /// Local host file picked through the dialog plugin.
    pub local_path: String,
}

/// Stream a VFS file down as base64 (desktop `downloadVfsFile`).
#[tauri::command]
pub async fn vfs_download_raw(
    state: State<'_, GatewayState>,
    request: VfsDownloadRequest,
) -> Result<BibBinaryView, String> {
    let base = lock(&state.base).clone();
    let token = lock(&state.token).clone();
    let url = endpoint(&base, "api/v1/vfs/file");
    let response = state
        .http
        .get(url)
        .query(&[("path", request.path.as_str())])
        .bearer_auth(token.as_deref().unwrap_or_default())
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    if !status.is_success() {
        let payload = response.json::<Value>().await.unwrap_or(Value::Null);
        let fallback = status.to_string();
        return Err(payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(fallback.as_str())
            .to_string());
    }
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    Ok(BibBinaryView {
        data_base64: BASE64.encode(bytes),
        content_type,
    })
}

/// Upload a locally picked file into the VFS (desktop `uploadToDir`).
#[tauri::command]
pub async fn vfs_upload(
    state: State<'_, GatewayState>,
    request: VfsUploadRequest,
) -> Result<Value, String> {
    let metadata = std::fs::metadata(&request.local_path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("selected upload is not a file".into());
    }
    if metadata.len() > MAX_BIB_UPLOAD_BYTES {
        return Err(format!(
            "selected file exceeds the {} MiB upload limit",
            MAX_BIB_UPLOAD_BYTES / (1024 * 1024)
        ));
    }
    let bytes = tokio::fs::read(&request.local_path)
        .await
        .map_err(|error| error.to_string())?;
    let base = lock(&state.base).clone();
    let token = lock(&state.token).clone();
    let url = endpoint(&base, "api/v1/vfs/file");
    let response = state
        .http
        .put(url)
        .query(&[("path", request.path.as_str())])
        .bearer_auth(token.as_deref().unwrap_or_default())
        .body(bytes)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let payload = response.json::<Value>().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let fallback = status.to_string();
        return Err(payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(fallback.as_str())
            .to_string());
    }
    Ok(payload)
}

/// Wiki binary bridge: IDs are path segments, and credentials stay in the native host.
#[tauri::command]
pub async fn upload_wiki_image(
    state: State<'_, GatewayState>,
    entry_id: String,
    filename: String,
    data_base64: String,
) -> Result<Value, String> {
    upload_wiki_image_request(&state, entry_id, filename, data_base64).await
}
async fn upload_wiki_image_request(
    state: &GatewayState,
    entry_id: String,
    filename: String,
    data_base64: String,
) -> Result<Value, String> {
    if data_base64.len() > (MAX_BIB_UPLOAD_BYTES as usize).div_ceil(3) * 4 {
        return Err("图片超过 50 MiB".into());
    }
    let bytes = BASE64
        .decode(data_base64)
        .map_err(|error| error.to_string())?;
    let base = lock(&state.base).clone();
    let token = lock(&state.token).clone();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(bytes).file_name(filename),
    );
    let response = state
        .http
        .post(endpoint(
            &base,
            &format!("wiki/entries/{}/attachments", encoded_segment(&entry_id)),
        ))
        .bearer_auth(token.as_deref().unwrap_or_default())
        .multipart(form)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let payload = response.json::<Value>().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("图片上传失败")
            .to_owned());
    }
    Ok(payload)
}

#[tauri::command]
pub async fn wiki_image_raw(
    state: State<'_, GatewayState>,
    entry_id: String,
    attachment_id: String,
) -> Result<BibBinaryView, String> {
    wiki_image_request(&state, entry_id, attachment_id).await
}
async fn wiki_image_request(
    state: &GatewayState,
    entry_id: String,
    attachment_id: String,
) -> Result<BibBinaryView, String> {
    let base = lock(&state.base).clone();
    let token = lock(&state.token).clone();
    let response = state
        .http
        .get(endpoint(
            &base,
            &format!(
                "wiki/entries/{}/attachments/{}/raw",
                encoded_segment(&entry_id),
                encoded_segment(&attachment_id)
            ),
        ))
        .bearer_auth(token.as_deref().unwrap_or_default())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("图片无法访问：{}", response.status()));
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if !mime.starts_with("image/") {
        return Err("附件不是图片".into());
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BIB_UPLOAD_BYTES)
    {
        return Err("图片超过 50 MiB".into());
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if bytes.len() + chunk.len() > MAX_BIB_UPLOAD_BYTES as usize {
            return Err("图片超过 50 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(BibBinaryView {
        data_base64: BASE64.encode(bytes),
        content_type: mime,
    })
}

#[cfg(test)]
mod wiki_image_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn wiki_native_binary_bridge_uploads_and_downloads_with_authentication() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let state = GatewayState::for_endpoint(
            &listener.local_addr().unwrap().to_string(),
            Some("wiki-token".into()),
        );
        let server = tokio::spawn(async move {
            for upload in [true, false] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 4096];
                let header_end;
                loop {
                    let size = stream.read(&mut buffer).await.unwrap();
                    assert!(size > 0);
                    bytes.extend_from_slice(&buffer[..size]);
                    if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        header_end = end + 4;
                        break;
                    }
                }
                let header = String::from_utf8(bytes[..header_end].to_vec())
                    .unwrap()
                    .to_ascii_lowercase();
                assert!(header.contains("authorization: bearer wiki-token"));
                if upload {
                    assert!(header.starts_with("post /api/v1/wiki/entries/entry/attachments "));
                    assert!(header.contains("multipart/form-data; boundary="));
                    let length: usize = header
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    while bytes.len() < header_end + length {
                        let size = stream.read(&mut buffer).await.unwrap();
                        assert!(size > 0);
                        bytes.extend_from_slice(&buffer[..size]);
                    }
                    let body = String::from_utf8_lossy(&bytes[header_end..]);
                    assert!(body.contains("filename=\"plot.png\""));
                    assert!(body.contains("PNG-bytes"));
                    let payload = br#"{"attachment_id":"image","filename":"plot.png"}"#;
                    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len()).as_bytes()).await.unwrap();
                    stream.write_all(payload).await.unwrap();
                } else {
                    assert!(
                        header.starts_with("get /api/v1/wiki/entries/entry/attachments/image/raw ")
                    );
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 9\r\nConnection: close\r\n\r\nPNG-bytes").await.unwrap();
                }
            }
        });
        let meta = upload_wiki_image_request(
            &state,
            "entry".into(),
            "plot.png".into(),
            BASE64.encode(b"PNG-bytes"),
        )
        .await
        .unwrap();
        assert_eq!(meta["attachment_id"], "image");
        let raw = wiki_image_request(&state, "entry".into(), "image".into())
            .await
            .unwrap();
        assert_eq!(raw.content_type, "image/png");
        assert_eq!(BASE64.decode(raw.data_base64).unwrap(), b"PNG-bytes");
        server.await.unwrap();
    }
}
