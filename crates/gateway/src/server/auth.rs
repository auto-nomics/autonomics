//! Authentication for browser-facing HTTP surfaces.
//!
//! Two credentials are accepted:
//!
//! - the long-lived gateway bearer token — used by the desktop app and
//!   by CLI clients that read `~/.autonomics/gateway.token` off the
//!   machine the daemon runs on;
//! - an opaque browser session id, carried in an `HttpOnly` cookie.
//!
//! The browser session is minted by `POST /api/session`: the caller
//! posts the bearer token once, the daemon answers with a `Set-Cookie`
//! header, and the token itself never reaches the browser again — not
//! in the HTML bootstrap, not in a JS global, not in the console. A
//! leaked page (a shared screenshot, a browser history entry, a proxy
//! access log) therefore leaks a revocable session rather than the
//! daemon's master credential.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Name of the cookie carrying a browser session id.
pub const SESSION_COOKIE: &str = "autonomics_session";

/// How long a browser session stays valid.
pub const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// The credentials a request may present.
#[derive(Clone)]
pub struct AuthContext {
    /// The gateway bearer token.
    pub token: Arc<String>,
    /// Live browser sessions.
    pub sessions: SessionStore,
}

impl AuthContext {
    /// Build a context around the daemon's bearer token.
    pub fn new(token: String) -> Self {
        Self {
            token: Arc::new(token),
            sessions: SessionStore::new(),
        }
    }
}

/// In-memory registry of live browser sessions.
///
/// Sessions are opaque random ids, not credentials derived from the
/// bearer token: revoking one (or letting it expire) cannot be used to
/// recover the daemon's master credential.
#[derive(Clone, Default)]
pub struct SessionStore {
    inner: Arc<Mutex<HashMap<String, Instant>>>,
}

impl SessionStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Instant>> {
        // A poisoned lock only means some other request panicked while
        // holding it; the map itself is still consistent, so recover
        // instead of poisoning every later request.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Mint a session id and register it.
    pub fn create(&self) -> String {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut sessions = self.lock();
        let now = Instant::now();
        sessions.retain(|_, expiry| *expiry > now);
        sessions.insert(id.clone(), now + SESSION_TTL);
        id
    }

    /// Whether `id` names a live session.
    pub fn is_valid(&self, id: &str) -> bool {
        self.remaining(id).is_some()
    }

    /// Remaining lifetime of `id`, or `None` when it is unknown or expired.
    pub fn remaining(&self, id: &str) -> Option<Duration> {
        let mut sessions = self.lock();
        let now = Instant::now();
        match sessions.get(id) {
            Some(expiry) if *expiry > now => Some(expiry.duration_since(now)),
            Some(_) => {
                sessions.remove(id);
                None
            }
            None => None,
        }
    }

    /// Drop a session.
    pub fn revoke(&self, id: &str) {
        self.lock().remove(id);
    }
}

/// Body of `POST /api/session`.
#[derive(Debug, serde::Deserialize)]
pub struct LoginRequest {
    /// The gateway bearer token.
    pub token: String,
}

/// `POST /api/session` — exchange the bearer token for a session cookie.
///
/// The token is compared in constant time and never echoed back; the
/// response carries only the cookie and a boolean.
pub async fn login(
    State(auth): State<Arc<AuthContext>>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Response {
    if !constant_time_eq(body.token.as_bytes(), auth.token.as_bytes()) {
        let body = Json(json!({ "error": "invalid token" })).into_response();
        return (StatusCode::UNAUTHORIZED, body).into_response();
    }
    let id = auth.sessions.create();
    let cookie = session_cookie(&id, SESSION_TTL, secure_request(&headers));
    let body = Json(json!({
        "authenticated": true,
        "expires_in_secs": SESSION_TTL.as_secs(),
    }));
    (StatusCode::OK, [(header::SET_COOKIE, cookie)], body).into_response()
}

/// `GET /api/session` — report whether the caller's cookie is live.
pub async fn session_status(
    State(auth): State<Arc<AuthContext>>,
    headers: HeaderMap,
) -> Response {
    let Some(id) = read_session_cookie(&headers) else {
        let body = Json(json!({ "authenticated": false })).into_response();
        return (StatusCode::UNAUTHORIZED, body).into_response();
    };
    match auth.sessions.remaining(&id) {
        Some(remaining) => {
            let body = Json(json!({
                "authenticated": true,
                "expires_in_secs": remaining.as_secs(),
            }));
            (StatusCode::OK, body).into_response()
        }
        None => {
            let body = Json(json!({ "authenticated": false })).into_response();
            (StatusCode::UNAUTHORIZED, body).into_response()
        }
    }
}

/// `DELETE /api/session` — drop the caller's session.
pub async fn logout(State(auth): State<Arc<AuthContext>>, headers: HeaderMap) -> Response {
    if let Some(id) = read_session_cookie(&headers) {
        auth.sessions.revoke(&id);
    }
    let cookie = session_cookie("", Duration::ZERO, secure_request(&headers));
    let body = Json(json!({ "authenticated": false }));
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        body,
    )
        .into_response()
}

/// Guard the protected routes: a valid bearer token or a live session
/// cookie both pass.
pub(crate) async fn bearer_or_session(
    State(auth): State<Arc<AuthContext>>,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    if authorized(&auth, &request) {
        return Ok(next.run(request).await);
    }
    let body = Json(json!({ "error": "missing or invalid credentials" })).into_response();
    Err((
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        body,
    )
        .into_response())
}

/// Whether the request presents a valid credential.
fn authorized(auth: &AuthContext, request: &Request) -> bool {
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if let Some(supplied) = bearer {
        if constant_time_eq(supplied.as_bytes(), auth.token.as_bytes()) {
            return true;
        }
    }
    match read_session_cookie(request.headers()) {
        Some(id) => auth.sessions.is_valid(&id),
        None => false,
    }
}

/// Read the session id out of the `Cookie` header.
fn read_session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        let part = part.trim();
        let (name, value) = part.split_once('=')?;
        (name.trim() == SESSION_COOKIE && !value.trim().is_empty())
            .then(|| value.trim().to_owned())
    })
}

/// Build the `Set-Cookie` value for a session (or for clearing one).
fn session_cookie(id: &str, ttl: Duration, secure: bool) -> String {
    let mut cookie = format!("{SESSION_COOKIE}={id}; Path=/; HttpOnly; SameSite=Strict");
    if ttl.is_zero() {
        // Expire the cookie right away.
        cookie.push_str("; Max-Age=0");
    } else {
        cookie.push_str(&format!("; Max-Age={}", ttl.as_secs()));
    }
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Whether the request reached us over TLS.
///
/// The daemon itself speaks plain HTTP on the loopback interface; the
/// scheme is decided by the reverse proxy in front of it, which reports
/// it in `X-Forwarded-Proto`.
fn secure_request(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("https"))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0, |difference, (left, right)| difference | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> AuthContext {
        AuthContext::new("secret-token".to_owned())
    }

    #[test]
    fn session_round_trip() {
        let auth = context();
        assert!(auth.sessions.remaining("never-issued").is_none());
        let id = auth.sessions.create();
        assert!(auth.sessions.is_valid(&id));
        assert!(auth.sessions.remaining(&id).is_some());
        auth.sessions.revoke(&id);
        assert!(!auth.sessions.is_valid(&id));
    }

    #[test]
    fn unknown_session_is_invalid() {
        let auth = context();
        assert!(!auth.sessions.is_valid("nope"));
    }

    #[test]
    fn cookie_is_parsed_and_scoped() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "other=1; autonomics_session=abc123; trailing=2"
                .parse()
                .unwrap(),
        );
        assert_eq!(read_session_cookie(&headers).as_deref(), Some("abc123"));

        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "autonomics_session=".parse().unwrap());
        assert_eq!(read_session_cookie(&headers), None);
    }

    #[test]
    fn cookie_carries_httponly_and_samesite() {
        let cookie = session_cookie("abc", SESSION_TTL, false);
        assert!(cookie.contains("autonomics_session=abc"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(!cookie.contains("Secure"));
        assert!(cookie.contains(&format!("Max-Age={}", SESSION_TTL.as_secs())));

        let cleared = session_cookie("", Duration::ZERO, true);
        assert!(cleared.contains("Max-Age=0"));
        assert!(cleared.contains("Secure"));
    }

    #[test]
    fn forwarded_proto_selects_secure_flag() {
        let mut headers = HeaderMap::new();
        assert!(!secure_request(&headers));
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(secure_request(&headers));
        headers.insert("x-forwarded-proto", "http".parse().unwrap());
        assert!(!secure_request(&headers));
    }

    #[test]
    fn constant_time_eq_matches() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
