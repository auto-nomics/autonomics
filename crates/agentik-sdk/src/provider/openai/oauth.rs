//! ChatGPT 订阅 OAuth 登录（Codex CLI 同款流程）。
//!
//! 链路：生成 PKCE + state → 起本机回调服务器（`127.0.0.1:{1455|1457}`）
//! → 用户浏览器完成授权 → 回调收 `code` → `POST {issuer}/oauth/token`
//! 换 `{id_token, access_token, refresh_token}` → 解析 id_token claims 得
//! `chatgpt_account_id` / 订阅计划 → 产出可入库的 [`TokenBlob`]。
//!
//! token 过期后用 [`refresh_blob`] 刷新（JSON body，可选字段缺省保留旧值）。

use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::wire::openai::chatgpt::ORIGINATOR;

/// 授权服务器。Codex CLI 同款。
pub const AUTH_ISSUER: &str = "https://auth.openai.com";
/// ⚠️ Codex CLI 的公开 OAuth client_id（第三方复用的通行做法）。
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// 回调端口（Hydra 回调白名单仅 1455/1457 两档）。
pub const CALLBACK_PORT: u16 = 1455;
pub const CALLBACK_PORT_FALLBACK: u16 = 1457;
/// 授权 scope —— 与 Codex CLI 一致。
pub const AUTH_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
/// 刷新时申请的 scope（Codex CLI 缩窄到三项）。
pub const REFRESH_SCOPE: &str = "openid profile email";
/// 等待用户在浏览器完成授权的总时限。
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
/// 距上次刷新超过该天数应主动刷新（Codex CLI 同款阈值）。
pub const REFRESH_INTERVAL_DAYS: i64 = 8;
/// access token 距过期不足该时长应主动刷新。
pub const REFRESH_AHEAD: Duration = Duration::from_secs(24 * 3600);

// ─── PKCE ───────────────────────────────────────────────────────────────────

/// PKCE S256 码对。
#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// 生成 PKCE 码对：verifier = 32 随机字节的 base64url（43 字符，符合 RFC
/// 7636 的 43..=128 区间），challenge = SHA256(verifier) 的 base64url。
pub fn generate_pkce() -> Result<Pkce, String> {
    let verifier = random_base64url(32)?;
    let challenge = pkce_challenge(&verifier);
    Ok(Pkce {
        verifier,
        challenge,
    })
}

/// 由 verifier 计算 S256 challenge（纯函数，便于用 RFC 7636 向量测试）。
#[must_use]
pub fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// OAuth state：32 随机字节的 base64url-nopad。
pub fn random_state() -> Result<String, String> {
    random_base64url(32)
}

fn random_base64url(n: usize) -> Result<String, String> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).map_err(|e| format!("random source unavailable: {e}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

// ─── Authorize URL ──────────────────────────────────────────────────────────

/// 构造授权 URL（纯函数）。查询参数与 Codex CLI 逐项一致。
#[must_use]
pub fn build_authorize_url(issuer: &str, port: u16, pkce: &Pkce, state: &str) -> String {
    let redirect_uri = format!("http://localhost:{port}/auth/callback");
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in [
        ("response_type", "code".to_string()),
        ("client_id", CLIENT_ID.to_string()),
        ("redirect_uri", redirect_uri),
        ("scope", AUTH_SCOPE.to_string()),
        ("code_challenge", pkce.challenge.clone()),
        ("code_challenge_method", "S256".to_string()),
        ("id_token_add_organizations", "true".to_string()),
        ("codex_cli_simplified_flow", "true".to_string()),
        ("state", state.to_string()),
        ("originator", ORIGINATOR.to_string()),
    ] {
        ser.append_pair(k, &v);
    }
    format!(
        "{}/oauth/authorize?{}",
        issuer.trim_end_matches('/'),
        ser.finish()
    )
}

// ─── JWT claims ─────────────────────────────────────────────────────────────

/// id_token 的 claims。组织/账号信息在 `https://api.openai.com/auth` 命名空间下。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Claims {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(rename = "https://api.openai.com/auth", default)]
    pub auth: AuthClaims,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthClaims {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chatgpt_plan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chatgpt_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chatgpt_account_id: Option<String>,
}

/// 解析 JWT 负载（不验签——id_token 直接来自与签发方的 TLS 连接，
/// Codex CLI 同样不验签）。
pub fn parse_id_token_claims(id_token: &str) -> Result<Claims, String> {
    let payload = jwt_payload(id_token)?;
    serde_json::from_slice(&payload).map_err(|e| format!("id_token claims 解析失败: {e}"))
}

/// 取 JWT（`header.payload.signature`）三段式的负载并 base64url 解码。
fn jwt_payload(jwt: &str) -> Result<Vec<u8>, String> {
    let b64 = jwt
        .split('.')
        .nth(1)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "token 不是 JWT 形态（缺少 payload 段）".to_string())?;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64)
        .map_err(|e| format!("JWT payload base64url 解码失败: {e}"))
}

// ─── TokenBlob ──────────────────────────────────────────────────────────────

/// 可入库的 token 快照。JSON 序列化后存 `providers.api_key` 列。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenBlob {
    pub access_token: String,
    pub refresh_token: String,
    /// `chatgpt_account_id` claim —— 请求头的 `chatgpt-account-id`。
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    pub last_refresh: chrono::DateTime<chrono::Utc>,
}

impl TokenBlob {
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| format!("token blob 序列化失败: {e}"))
    }

    pub fn from_json(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| format!("token blob 解析失败: {e}"))
    }

    /// access_token（JWT）的 `exp` 剩余有效期。已过期或无 `exp` → `None`。
    #[must_use]
    pub fn access_token_expires_in(&self) -> Option<Duration> {
        (self.access_token_exp()? - chrono::Utc::now())
            .to_std()
            .ok()
    }

    /// access token 的 `exp` 时间点。非 JWT / 无 `exp` → `None`。
    fn access_token_exp(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        let payload: serde_json::Value =
            serde_json::from_slice(&jwt_payload(&self.access_token).ok()?).ok()?;
        let exp = payload.get("exp")?.as_i64()?;
        chrono::DateTime::from_timestamp(exp, 0)
    }

    /// access token 是否已过期（无 `exp` 字段视为未过期——无法判断时不阻塞）。
    #[must_use]
    pub fn access_token_expired(&self) -> bool {
        self.access_token_exp()
            .is_some_and(|exp| exp <= chrono::Utc::now())
    }

    /// 是否应主动刷新：距上次刷新超过 8 天，或将在 24h 内过期。
    #[must_use]
    pub fn should_refresh(&self) -> bool {
        let stale =
            chrono::Utc::now() - self.last_refresh > chrono::Duration::days(REFRESH_INTERVAL_DAYS);
        let expiring = self
            .access_token_expires_in()
            .is_some_and(|d| d < REFRESH_AHEAD);
        stale || expiring
    }
}

// ─── Token 交换 / 刷新 ──────────────────────────────────────────────────────

/// `/oauth/token` 授权码交换的响应。
#[derive(Debug, Clone, Deserialize)]
pub struct TokenExchange {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
}

/// 用授权码换 token（form-urlencoded，与 Codex CLI 一致）。
pub async fn exchange_code(
    issuer: &str,
    port: u16,
    code: &str,
    verifier: &str,
) -> Result<TokenExchange, String> {
    let redirect_uri = format!("http://localhost:{port}/auth/callback");
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("client_id", CLIENT_ID)
        .append_pair("code_verifier", verifier)
        .finish();
    let url = format!("{}/oauth/token", issuer.trim_end_matches('/'));
    let text = http_post_form(&url, &body).await?;
    serde_json::from_str(&text).map_err(|e| format!("token 响应解析失败: {e}"))
}

async fn http_post_form(url: &str, body: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    let resp = client
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| format!("token 端点请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "token 端点返回 {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    Ok(text)
}

/// 刷新 token（默认 issuer）。
pub async fn refresh_blob(blob: &TokenBlob) -> Result<TokenBlob, String> {
    refresh_blob_with(AUTH_ISSUER, blob).await
}

/// 刷新 token（JSON body；`id_token`/`access_token`/`refresh_token` 均为
/// 可选字段，缺省保留旧值——与 Codex CLI 一致）。
pub async fn refresh_blob_with(issuer: &str, blob: &TokenBlob) -> Result<TokenBlob, String> {
    let body = build_refresh_body(blob);
    let url = format!("{}/oauth/token", issuer.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("刷新请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "token 刷新失败（{status}）：请重新登录。{}",
            text.chars().take(300).collect::<String>()
        ));
    }
    let parsed: RefreshResponse =
        serde_json::from_str(&text).map_err(|e| format!("刷新响应解析失败: {e}"))?;
    Ok(merge_refresh(blob, parsed))
}

fn build_refresh_body(blob: &TokenBlob) -> String {
    serde_json::json!({
        "client_id": CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": blob.refresh_token,
        "scope": REFRESH_SCOPE,
    })
    .to_string()
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RefreshResponse {
    pub id_token: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

/// 把刷新响应合并进旧 blob（纯函数）：可选字段缺省保留旧值；带新
/// id_token 时顺带更新 email/plan/account_id；`last_refresh` 刷新为当前。
#[must_use]
pub fn merge_refresh(blob: &TokenBlob, resp: RefreshResponse) -> TokenBlob {
    let mut out = blob.clone();
    if let Some(claims) = resp
        .id_token
        .as_deref()
        .and_then(|t| parse_id_token_claims(t).ok())
    {
        out.email = claims.email.or(out.email);
        out.plan_type = claims.auth.chatgpt_plan_type.or(out.plan_type);
        if let Some(acc) = claims.auth.chatgpt_account_id.filter(|acc| !acc.is_empty()) {
            out.account_id = acc;
        }
    }
    if let Some(access) = resp.access_token {
        out.access_token = access;
    }
    if let Some(refresh) = resp.refresh_token {
        out.refresh_token = refresh;
    }
    out.last_refresh = chrono::Utc::now();
    out
}

/// 交换结果 → 入库 blob：解析 claims 并要求 `chatgpt_account_id` 非空
/// （授权 URL 带 `id_token_add_organizations=true` 时正常必有）。
pub fn blob_from_exchange(exchange: TokenExchange) -> Result<TokenBlob, String> {
    let claims = parse_id_token_claims(&exchange.id_token)?;
    let account_id = claims
        .auth
        .chatgpt_account_id
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "id_token 缺少 chatgpt_account_id（请重试登录；若持续出现说明该账号无 Codex 权限）"
                .to_string()
        })?;
    Ok(TokenBlob {
        access_token: exchange.access_token,
        refresh_token: exchange.refresh_token,
        account_id,
        email: claims.email,
        plan_type: claims.auth.chatgpt_plan_type,
        last_refresh: chrono::Utc::now(),
    })
}

// ─── 本机回调服务器 ─────────────────────────────────────────────────────────

/// 绑定回调端口：先试 [`CALLBACK_PORT`]，被占则用 [`CALLBACK_PORT_FALLBACK`]。
pub fn bind_callback_server() -> Result<(tiny_http::Server, u16), String> {
    for port in [CALLBACK_PORT, CALLBACK_PORT_FALLBACK] {
        if let Ok(server) = tiny_http::Server::http(format!("127.0.0.1:{port}")) {
            return Ok((server, port));
        }
    }
    Err(format!(
        "回调端口 {CALLBACK_PORT}/{CALLBACK_PORT_FALLBACK} 均被占用；请关闭占用这些端口的进程后重试"
    ))
}

/// 回调处理结果（发给 [`LoginWaiter`] 的消息）。
type CallbackOutcome = std::result::Result<String, String>;

/// 阻塞式回调循环：`/auth/callback`（校验 state，收 code）、`/cancel`
/// （取消登录）、其余 404。拿到 code / 出错 / 取消后退出并通过 channel 上报。
fn callback_loop(
    server: tiny_http::Server,
    state: String,
    tx: std::sync::mpsc::Sender<CallbackOutcome>,
) {
    let outcome = loop {
        let Ok(request) = server.recv() else {
            break Err("回调服务器异常退出".to_string());
        };
        let url = request.url().to_string();
        let (path, query) = match url.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (url.clone(), String::new()),
        };
        if path == "/auth/callback" {
            let params: std::collections::HashMap<String, String> =
                url::form_urlencoded::parse(query.as_bytes())
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
            if let Some(err) = params.get("error") {
                let detail = params
                    .get("error_description")
                    .map(|d| format!("（{d}）"))
                    .unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(format!("登录失败: {err}"))
                        .with_status_code(400),
                );
                break Err(format!("授权失败: {err}{detail}"));
            }
            // state 不符（陈旧标签页/串扰回调）：拒绝但不退出，继续等。
            if params.get("state").map(String::as_str) != Some(state.as_str()) {
                let _ = request.respond(
                    tiny_http::Response::from_string("state mismatch").with_status_code(400),
                );
                continue;
            }
            match params.get("code") {
                Some(code) if !code.is_empty() => {
                    let _ = request.respond(tiny_http::Response::from_string(
                        "登录成功：可以关闭此页面，回到终端继续。",
                    ));
                    break Ok(code.clone());
                }
                _ => {
                    let _ = request.respond(
                        tiny_http::Response::from_string("missing authorization code")
                            .with_status_code(400),
                    );
                    continue;
                }
            }
        } else if path == "/cancel" {
            let _ = request.respond(tiny_http::Response::from_string("已取消登录。"));
            break Err("登录已取消".to_string());
        } else {
            let _ = request
                .respond(tiny_http::Response::from_string("Not Found").with_status_code(404));
        }
    };
    let _ = tx.send(outcome);
}

/// 登录等待句柄：等浏览器回调并完成 token 交换。
pub struct LoginWaiter {
    rx: std::sync::mpsc::Receiver<CallbackOutcome>,
    port: u16,
    pkce: Pkce,
    deadline: Instant,
    issuer: String,
}

/// 一步式编排（默认 issuer [`AUTH_ISSUER`]）：见 [`login_flow_with_issuer`]。
pub fn login_flow() -> Result<(String, LoginWaiter), String> {
    login_flow_with_issuer(AUTH_ISSUER)
}

/// 一步式编排（issuer 可注入，e2e 测试用 mock）：生成 PKCE/state →
/// 绑定回调端口 → 起监听线程 → 返回 `(authorize_url, waiter)`。调用方
/// 把 URL 交给用户（打开浏览器/复制剪贴板），随后 `waiter.wait().await`
/// 得到入库用的 [`TokenBlob`]。
pub fn login_flow_with_issuer(issuer: &str) -> Result<(String, LoginWaiter), String> {
    let issuer = issuer.to_string();
    let pkce = generate_pkce()?;
    let state = random_state()?;
    let (server, port) = bind_callback_server()?;
    let authorize_url = build_authorize_url(&issuer, port, &pkce, &state);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || callback_loop(server, state, tx));
    Ok((
        authorize_url,
        LoginWaiter {
            rx,
            port,
            pkce,
            deadline: Instant::now() + LOGIN_TIMEOUT,
            issuer,
        },
    ))
}

impl LoginWaiter {
    /// 等待回调（总时限 [`LOGIN_TIMEOUT`]）→ 换 token → 解析 claims →
    /// 产出 [`TokenBlob`]。超时会向回调端口发 `/cancel` 停掉监听线程。
    ///
    /// # Errors
    ///
    /// 用户取消 / 超时 / state 校验失败后端口异常 / token 交换失败 /
    /// claims 缺 `chatgpt_account_id` —— 均返回可直接展示的中文原因。
    pub async fn wait(self) -> Result<TokenBlob, String> {
        let Self {
            rx,
            port,
            pkce,
            deadline,
            issuer,
        } = self;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            send_cancel(port);
            return Err("登录超时（10 分钟）未完成授权".to_string());
        }
        let cancel_port = port;
        let code = tokio::task::spawn_blocking(move || rx.recv_timeout(remaining))
            .await
            .map_err(|e| format!("登录任务异常: {e}"))?
            .map_err(|e| match e {
                std::sync::mpsc::RecvTimeoutError::Timeout => {
                    send_cancel(cancel_port);
                    "登录超时（10 分钟）未完成授权".to_string()
                }
                std::sync::mpsc::RecvTimeoutError::Disconnected => "回调服务器异常退出".to_string(),
            })??;
        let exchange = exchange_code(&issuer, port, &code, &pkce.verifier).await?;
        blob_from_exchange(exchange)
    }
}

/// 向回调端口发 `/cancel`，让监听线程退出（超时兜底）。
fn send_cancel(port: u16) {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    let Ok(addr) = format!("127.0.0.1:{port}").parse() else {
        return;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) else {
        return;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.write_all(
        format!("GET /cancel HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    );
    let mut buf = [0u8; 64];
    let _ = stream.read(&mut buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_s256_known_vector() {
        // RFC 7636 Appendix B 的工作向量（期望值经 openssl + python 双向核对）。
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p8URWbuGJSstw-cM";
        assert_eq!(
            pkce_challenge(verifier),
            "BhNtkMZsCfl6TpN4-TfqLSM_YWclIgSfcTKKXgUvZWs"
        );
    }

    #[test]
    fn generated_pkce_shapes() {
        let pkce = generate_pkce().unwrap();
        // 32 字节 base64url-nopad = 43 字符，且不含填充与 url 不安全字符。
        assert_eq!(pkce.verifier.len(), 43);
        assert!(!pkce.verifier.contains(['+', '/', '=']));
        assert_eq!(pkce.challenge, pkce_challenge(&pkce.verifier));
        let state = random_state().unwrap();
        assert_eq!(state.len(), 43);
        assert!(!state.contains(['+', '/', '=']));
    }

    #[test]
    fn authorize_url_shape() {
        let pkce = Pkce {
            verifier: "v".into(),
            challenge: "c".into(),
        };
        for port in [1455u16, 1457] {
            let url = build_authorize_url("https://auth.test/", port, &pkce, "st-ate=");
            let parsed = url::Url::parse(&url).unwrap();
            assert_eq!(parsed.host_str(), Some("auth.test"));
            assert_eq!(parsed.path(), "/oauth/authorize");
            let q: std::collections::HashMap<String, String> = parsed
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            assert_eq!(q["response_type"], "code");
            assert_eq!(q["client_id"], CLIENT_ID);
            assert_eq!(
                q["redirect_uri"],
                format!("http://localhost:{port}/auth/callback")
            );
            assert_eq!(q["scope"], AUTH_SCOPE);
            assert_eq!(q["code_challenge"], "c");
            assert_eq!(q["code_challenge_method"], "S256");
            assert_eq!(q["id_token_add_organizations"], "true");
            assert_eq!(q["codex_cli_simplified_flow"], "true");
            assert_eq!(q["state"], "st-ate=");
            assert_eq!(q["originator"], ORIGINATOR);
        }
    }

    fn synthetic_jwt(payload: &serde_json::Value) -> String {
        let enc = |v: &serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap())
        };
        let header = serde_json::json!({"alg": "none", "typ": "JWT"});
        format!("{}.{}.sig", enc(&header), enc(payload))
    }

    #[test]
    fn claims_parse_synthetic_jwt() {
        let jwt = synthetic_jwt(&serde_json::json!({
            "email": "user@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "plus",
                "chatgpt_user_id": "u-1",
                "chatgpt_account_id": "org-9"
            }
        }));
        let claims = parse_id_token_claims(&jwt).unwrap();
        assert_eq!(claims.email.as_deref(), Some("user@example.com"));
        assert_eq!(claims.auth.chatgpt_plan_type.as_deref(), Some("plus"));
        assert_eq!(claims.auth.chatgpt_account_id.as_deref(), Some("org-9"));

        // 缺 auth 命名空间：可解析，字段为空（不报错）。
        let bare = synthetic_jwt(&serde_json::json!({"email": "x@y.z"}));
        let claims = parse_id_token_claims(&bare).unwrap();
        assert!(claims.auth.chatgpt_account_id.is_none());

        // 非 JWT → 报错。
        assert!(parse_id_token_claims("not-a-jwt").is_err());
    }

    fn blob_fixture() -> TokenBlob {
        TokenBlob {
            access_token: "old-access".into(),
            refresh_token: "old-refresh".into(),
            account_id: "org-1".into(),
            email: Some("a@b.c".into()),
            plan_type: Some("plus".into()),
            last_refresh: chrono::Utc::now(),
        }
    }

    #[test]
    fn blob_json_roundtrip() {
        let blob = blob_fixture();
        let json = blob.to_json().unwrap();
        assert_eq!(TokenBlob::from_json(&json).unwrap(), blob);
        assert!(TokenBlob::from_json("garbage").is_err());
    }

    #[test]
    fn expires_in_reads_exp_claim() {
        let exp = chrono::Utc::now() + chrono::Duration::hours(1);
        let jwt = synthetic_jwt(&serde_json::json!({"exp": exp.timestamp()}));
        let mut blob = blob_fixture();
        blob.access_token = jwt;
        let d = blob.access_token_expires_in().unwrap();
        assert!(d > Duration::from_secs(3500) && d <= Duration::from_secs(3600));
        assert!(!blob.access_token_expired());

        let past = synthetic_jwt(&serde_json::json!({"exp": 1_000_000_000}));
        blob.access_token = past;
        assert!(blob.access_token_expired());
    }

    #[test]
    fn refresh_body_and_merge_semantics() {
        let blob = blob_fixture();
        let body: serde_json::Value = serde_json::from_str(&build_refresh_body(&blob)).unwrap();
        assert_eq!(body["client_id"], CLIENT_ID);
        assert_eq!(body["grant_type"], "refresh_token");
        assert_eq!(body["refresh_token"], "old-refresh");
        assert_eq!(body["scope"], REFRESH_SCOPE);

        // 缺 access_token/refresh_token：保留旧值；新 id_token 更新 claims。
        let new_id = synthetic_jwt(&serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "pro",
                "chatgpt_account_id": "org-2"
            }
        }));
        let merged = merge_refresh(
            &blob,
            RefreshResponse {
                id_token: Some(new_id),
                access_token: None,
                refresh_token: Some("new-refresh".into()),
            },
        );
        assert_eq!(merged.access_token, "old-access");
        assert_eq!(merged.refresh_token, "new-refresh");
        assert_eq!(merged.account_id, "org-2");
        assert_eq!(merged.plan_type.as_deref(), Some("pro"));
        assert!(merged.last_refresh > blob.last_refresh);
    }

    #[test]
    fn blob_from_exchange_requires_account_id() {
        let with_acc = synthetic_jwt(&serde_json::json!({
            "email": "u@x.y",
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "pro",
                "chatgpt_account_id": "org-a"
            }
        }));
        let blob = blob_from_exchange(TokenExchange {
            id_token: with_acc,
            access_token: "acc".into(),
            refresh_token: "ref".into(),
        })
        .unwrap();
        assert_eq!(blob.account_id, "org-a");
        assert_eq!(blob.email.as_deref(), Some("u@x.y"));

        let without = synthetic_jwt(&serde_json::json!({"email": "u@x.y"}));
        assert!(
            blob_from_exchange(TokenExchange {
                id_token: without,
                access_token: "acc".into(),
                refresh_token: "ref".into(),
            })
            .is_err()
        );
    }

    #[test]
    fn callback_loop_accepts_valid_state_and_rejects_mismatch() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || callback_loop(server, "expected-state".into(), tx));

        let http_get = |port: u16, path: &str| -> u16 {
            let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).expect("connect");
            stream
                .write_all(
                    format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .unwrap();
            let mut buf = String::new();
            stream.read_to_string(&mut buf).unwrap();
            buf.split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or_default()
        };

        // state 不符 → 400，且循环仍在等（随后正确请求仍能拿到 code）。
        assert_eq!(http_get(port, "/auth/callback?code=evil&state=wrong"), 400);
        // 非回调路径 → 404。
        assert_eq!(http_get(port, "/favicon.ico"), 404);
        // 正确 state → 200 + 上报 code；此后循环退出、server 关闭。
        assert_eq!(
            http_get(port, "/auth/callback?code=the-code&state=expected-state"),
            200
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(),
            "the-code"
        );

        // 取消路径：在独立 server 上验证（成功收 code 后循环已退出）。
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || callback_loop(server, "expected-state".into(), tx));
        assert_eq!(http_get(port, "/cancel"), 200);
        // 取消 → Err("登录已取消")。
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap_err(),
            "登录已取消"
        );

        // 授权页报错（error 参数）→ Err 上报。
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || callback_loop(server, "expected-state".into(), tx));
        assert_eq!(
            http_get(
                port,
                "/auth/callback?error=access_denied&state=expected-state"
            ),
            400
        );
        assert!(
            rx.recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap_err()
                .contains("access_denied")
        );
    }
}
