//! The resident backend gateway: one daemon owning the `RuntimeHost`,
//! many thin frontends over HTTP + SSE.
//!
//! Layout:
//! - [`proto`]: the wire contract (shared by server and clients);
//! - [`hub`]: the event fan-out hub (seq, replay, lag semantics);
//! - [`driver`]: the daemon's host pump — the single consumer of
//!   `RuntimeHost::recv_next`;
//! - [`server`]: the axum API surface;
//! - [`daemon`]: `autonomics serve` bootstrap + graceful shutdown;
//! - [`client`]: the Rust frontend SDK (REST + SSE pump);
//! - [`manager`]: probe / auto-spawn / stop for frontends;
//! - [`model_store`]: the daemon-owned app DB (model catalogue,
//!   credentials, ChatGPT OAuth).

pub mod client;
pub mod daemon;
pub mod driver;
pub mod hub;
pub mod manager;
pub mod model_store;
pub mod proto;
pub mod server;

#[cfg(feature = "test-util")]
pub mod testing;

pub use client::{ClientError, GatewayClient, GatewayFrame};
pub use daemon::{DaemonError, DaemonOptions, GATEWAY_VERSION, run_daemon};
pub use hub::EventHub;
pub use proto::HostEventView;

// Re-exports so frontends depend on `gateway` alone — the runtime types
// frontends legitimately need (config paths for the standalone CLI
// subcommands, model bootstrap for `autonomics run`) without a direct runtime
// dependency.
/// Daemon startup error type — re-exported so frontends can pattern-match
/// (e.g. `autonomics run`'s lock-conflict hint) without a direct runtime dep.
pub use runtime::Error as RuntimeError;
pub use runtime::control::AgentInfo;
pub use runtime::{RuntimeConfig, RuntimeConfigBuilder};
pub use runtime::{bibliography_file_storage, model_bootstrap};
