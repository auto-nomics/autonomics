use std::net::SocketAddr;

use axum::serve;
use axum::{Json, Router, routing::get};
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_HTTP_API_ADDR: &str = "127.0.0.1:8765";

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
        format!("http://{}/", self.addr)
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

/// Build the aggregate TUI API router.
///
/// Feature modules expose private route tables. This is the single place where
/// future REST modules are mounted under the public `/api` namespace.
pub fn api_router(shared: bib_base::BibShared) -> Router {
    Router::new()
        .route("/api/v1", get(api_index))
        .route("/api/health", get(health))
        .nest("/api/v1/bib", crate::bib::router(shared))
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn api_index() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "name": "autonomics-tui",
        "version": 1,
        "modules": ["bib"],
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
