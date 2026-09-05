use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::serve;
use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    http::header,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_HTTP_API_ADDR: &str = "127.0.0.1:8765";

#[derive(Clone)]
struct BearerAuthState {
    token: Arc<Option<String>>,
}

/// A running local HTTP API server.
pub struct HttpServerHandle {
    addr: SocketAddr,
    shutdown: CancellationToken,
    task: JoinHandle<std::io::Result<()>>,
}

impl HttpServerHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn url(&self) -> String {
        let host = match self.addr.ip() {
            IpAddr::V4(address) if address.is_unspecified() => primary_local_ip("8.8.8.8:80")
                .unwrap_or(IpAddr::from([127, 0, 0, 1]))
                .to_string(),
            IpAddr::V6(address) if address.is_unspecified() => {
                primary_local_ip("[2001:4860:4860::8888]:80")
                    .map(|ip| {
                        IpAddr::V6(match ip {
                            IpAddr::V6(address) => address,
                            IpAddr::V4(address) => address.to_ipv6_mapped(),
                        })
                    })
                    .unwrap_or(IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1]))
                    .to_string()
            }
            IpAddr::V4(address) => address.to_string(),
            IpAddr::V6(address) => format!("[{address}]"),
        };
        format!("http://{host}:{}/", self.addr.port())
    }

    /// Signal graceful shutdown and wait for the accept loop to stop.
    pub async fn shutdown(self) -> std::io::Result<()> {
        self.shutdown.cancel();
        match self.task.await {
            Ok(result) => result,
            Err(error) => Err(std::io::Error::other(format!(
                "TUI HTTP API server task failed: {error}"
            ))),
        }
    }
}

/// Determine the local address that the operating system would route through
/// for a public destination. UDP `connect` only selects a route; it does not
/// send a packet.
fn primary_local_ip(destination: &str) -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind(match destination.starts_with('[') {
        true => "[::]:0",
        false => "0.0.0.0:0",
    })
    .ok()?;
    socket.connect(destination).ok()?;
    socket.local_addr().ok().map(|addr| addr.ip())
}

/// Build the aggregate TUI API router.
///
/// Feature modules expose private route tables. This is the single place where
/// future REST modules are mounted under the public `/api` namespace.
pub fn api_router(shared: bib_base::BibShared) -> Router {
    api_router_with_auth(shared, None)
}

/// Builder for the aggregate router.
///
/// The default constructors cover the classic bibliography-only host; the
/// builder additionally wires the agentik model slot so `/api/v1/agent` can
/// serve chat from the same process.
pub struct ApiRouterBuilder {
    shared: bib_base::BibShared,
    bearer_token: Option<String>,
    model: Option<Arc<arc_swap::ArcSwapOption<agentik_sdk::model::Model>>>,
    /// Wiring for the resident-agent module (`/api/v1/agent/threads`).
    #[cfg(feature = "runtime-host")]
    infra: Option<runtime::host::SharedInfra>,
}

impl ApiRouterBuilder {
    #[must_use]
    pub fn new(shared: bib_base::BibShared) -> Self {
        Self {
            shared,
            bearer_token: None,
            model: None,
            #[cfg(feature = "runtime-host")]
            infra: None,
        }
    }

    #[must_use]
    pub fn bearer_token(mut self, token: Option<String>) -> Self {
        self.bearer_token = token;
        self
    }

    /// Share the TUI's active-model slot with the chat endpoint. Without it
    /// the agent module is not mounted and chat returns 404.
    #[must_use]
    pub fn model(mut self, model: Arc<arc_swap::ArcSwapOption<agentik_sdk::model::Model>>) -> Self {
        self.model = Some(model);
        self
    }

    /// Serve resident runtime-hosted agents (`/api/v1/agent/threads`)
    /// alongside the ephemeral endpoint. Only exists with the
    /// `runtime-host` feature; the minimal embeddable host stays
    /// ephemeral-only.
    #[cfg(feature = "runtime-host")]
    #[must_use]
    pub fn host(mut self, infra: runtime::host::SharedInfra) -> Self {
        self.infra = Some(infra);
        self
    }

    pub fn build(self) -> Router {
        let Self {
            shared,
            bearer_token,
            model,
            #[cfg(feature = "runtime-host")]
            infra,
        } = self;
        let auth_state = BearerAuthState {
            token: Arc::new(bearer_token.filter(|token| !token.trim().is_empty())),
        };

        let mut api = Router::new()
            .route("/api/v1", get(api_index))
            .route("/api/health", get(health))
            .nest("/api/v1/bib", crate::bib::router(shared.clone()));
        if let Some(model) = model {
            let agent_api =
                crate::agent::router(crate::agent::AgentState { shared, model: model.clone() });
            #[cfg(feature = "runtime-host")]
            let (agent_api, runtime_mounted) = match infra {
                Some(infra) => (
                    agent_api.merge(crate::agent_runtime::router(
                        crate::agent_runtime::RuntimeAgentState::new(infra, model),
                    )),
                    true,
                ),
                None => (agent_api, false),
            };
            #[cfg(not(feature = "runtime-host"))]
            let runtime_mounted = false;
            // Capability index: the frontend probes this to pick between the
            // session-based (`/threads`) and legacy (`/chat`) protocols.
            let mode = if runtime_mounted { "runtime" } else { "ephemeral" };
            api = api.nest(
                "/api/v1/agent",
                agent_api.route("/", get(move || async move {
                    Json(serde_json::json!({ "mode": mode }))
                })),
            );
        }

        let api = api.layer(axum::middleware::from_fn_with_state(
            auth_state,
            bearer_auth,
        ));

        // The SPA host mounts outside the auth layer: static assets carry no
        // secrets and the shell has to load before it could prompt for a token.
        api.merge(crate::frontend::router())
    }
}

/// Build the aggregate router with an optional bearer token for `/api` routes.
pub fn api_router_with_auth(shared: bib_base::BibShared, bearer_token: Option<String>) -> Router {
    ApiRouterBuilder::new(shared)
        .bearer_token(bearer_token)
        .build()
}

async fn bearer_auth(
    State(state): State<BearerAuthState>,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    let Some(expected) = state.token.as_ref().as_deref() else {
        return Ok(next.run(request).await);
    };
    if !request.uri().path().starts_with("/api/") {
        return Ok(next.run(request).await);
    }

    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let valid =
        supplied.is_some_and(|supplied| constant_time_eq(supplied.as_bytes(), expected.as_bytes()));
    if !valid {
        let body = axum::Json(serde_json::json!({
            "error": "missing or invalid bearer token"
        }))
        .into_response();
        return Err((
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            body,
        )
            .into_response());
    }

    Ok(next.run(request).await)
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

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn api_index() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "name": "autonomics-tui",
        "version": 1,
        "modules": ["bib", "agent"],
    }))
}

/// Bind `addr` and start the API server on the current Tokio runtime.
///
/// Binding happens before spawning, so callers know synchronously whether the
/// configured address is available. Serving itself remains a background task.
pub async fn start(
    router: Router,
    addr: impl tokio::net::ToSocketAddrs,
) -> std::io::Result<HttpServerHandle> {
    let listener = TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;
    let shutdown = CancellationToken::new();
    let server_shutdown = shutdown.clone();

    let task = Handle::current().spawn(async move {
        serve(listener, router)
            .with_graceful_shutdown(async move {
                server_shutdown.cancelled().await;
            })
            .await
    });

    Ok(HttpServerHandle {
        addr: bound_addr,
        shutdown,
        task,
    })
}
