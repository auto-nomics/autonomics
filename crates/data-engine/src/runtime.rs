//! Runtime of Data Engine based on tokio runtime.
//!
//! # Architecture (three-layer isolation)
//!
//! **Layer 1 — Metadata bypass.** Read-only node-registry queries
//! (`list_node_factories`, `get_node_spec`, `get_node_ports`, `get_node_doc`)
//! are served directly from `Arc<NodeRegistry>` on the client side and never
//! enter the actor channel, so they cannot be blocked by a running DAG.
//!
//! **Layer 2 — RunFlow fire-and-forget.** `RunFlow` is spawned as a background
//! task inside the engine actor; the actor loop returns immediately and keeps
//! processing subsequent commands. A per-actor `AtomicBool` guards against
//! concurrent runs; mutation commands during a run return an immediate error
//! instead of blocking.
//!
//! **Layer 3 — Per-agent actors.** Each agent gets its own tokio task
//! and its own mpsc channel, managed by `DataEngineManager`. A long DAG run in
//! agent-A's actor never blocks agent-B's commands. Heavy infrastructure
//! (`NodeRegistry`, `DagHistory`, `RuntimeEnv`) is shared via `Arc`.
//!
//! # Module layout
//!
//! - [`actor`] — the per-agent command loop (`AgentEngineActor`), including
//!   workspace/cache/report restore and the flow-run task.
//! - [`client`] — `DataEngineClient` and the run-cancel-on-drop reply
//!   wrapper.
//! - [`manager`] — `DataEngineManager`: lazy per-agent actor spawn, idle
//!   eviction, and storage maintenance.
//! - [`storage`], [`types`], [`error`] — storage coordination, command
//!   types, and client error types.
//!
//! Each submodule carries its own tests; this facade holds no logic.

mod actor;
mod client;
mod manager;

pub mod error;
pub mod storage;
pub mod types;

pub use client::{CancelOnDropReceiver, DataEngineClient};
pub use manager::{DataEngineManager, spawn_with_engine};
pub use storage::{
    AgentActivityTracker, ArtifactEntry, MaintenancePolicy, RunAwareCleanReport, StorageCoordinator,
};
