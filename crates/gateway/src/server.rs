//! The gateway's axum server: agent control surface, hydration queries,
//! storage access, model-config API, and the SSE event stream.

mod agents;
mod auth;
mod docs;
mod error;
mod events;
mod hydration;
mod lifecycle;
mod model_config;
mod plugins;
mod router;
mod sessions;
mod settings;
mod skills;
mod state;
mod storage;

pub use error::GatewayError;
pub use router::{api_router, router_with_bib};
pub use state::GatewayState;
