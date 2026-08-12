//! Unified Rust API — `WorkflowManager` + per-session `WorkflowClient` + the
//! `WorkflowCmd` actor pattern.
//!
//! Three-layer isolation follows the [`data-engine`](https://docs.rs/data-engine)
//! pattern:
//!
//! 1. **`WorkflowManager`** owns the [`DbPool`](crate::store::DbPool),
//!    [`NodeRegistry`](crate::registry::NodeRegistry) and
//!    [`SkillRegistry`](crate::registry::SkillRegistry) for the whole app.
//! 2. **`WorkflowClient`** is a per-session handle: a single `mpsc` channel
//!    to the per-session actor. Cheap to clone, never blocks.
//! 3. **`WorkflowCmd`** is the command enum sent over that channel. Each
//!    variant carries a `oneshot::Sender<WfResult<T>>` reply.

pub mod client;
pub mod commands;
pub mod manager;

pub use client::WorkflowClient;
pub use commands::WorkflowCmd;
pub use manager::WorkflowManager;
