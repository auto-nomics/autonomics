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
        }
        // Structured PDF extraction runs the local Zotero layout models. Keep the
        // normal API deadline short while allowing a first Reading Mode conversion.
        if method.eq_ignore_ascii_case("POST") && normalized_path.ends_with("/fulltext/structured")
        {
            request = request.timeout(Duration::from_secs(210));
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
