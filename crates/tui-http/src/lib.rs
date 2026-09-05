//! Local HTTP API backend for the Autonomics TUI.
//!
//! The server is a small composable Axum host. Feature modules provide route
//! tables, while this crate owns aggregation and the TUI process lifecycle.
//! Bibliography management is the first module; future REST modules can be
//! mounted without changing startup behavior.

pub mod agent;
#[cfg(feature = "runtime-host")]
pub mod agent_runtime;
pub mod bib;
pub mod server;

mod frontend;

pub use bib::load_stored_easyscholar_key;
pub use server::{
    ApiRouterBuilder, DEFAULT_HTTP_API_ADDR, HttpServerHandle, api_router, api_router_with_auth,
    start,
};
