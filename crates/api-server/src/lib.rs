//! Local HTTP API server for the Autonomics gateway daemon.
//!
//! The server is a small composable Axum host. Feature modules provide route
//! tables, while this crate owns aggregation and the daemon process lifecycle.
//! Bibliography management is the first module; future REST modules can be
//! mounted without changing startup behavior.

pub mod bib;
mod discovery;
pub mod server;
mod snapshots;
mod subscriptions;

pub use server::{
    DEFAULT_HTTP_API_ADDR, HttpServerHandle, api_router, api_router_with_auth, frontend_router,
    start,
};
