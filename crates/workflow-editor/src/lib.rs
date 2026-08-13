//! # workflow-editor
//!
//! A visual workflow editor where:
//!
//! - **Skills** are reusable DAG subgraphs (nodes + edges + SOP prompt + tool
//!   references). Skills can be nested — a skill's surface ports become a
//!   single node's ports in the outer graph.
//! - **Workflows** are DAGs persisted to SQLite with git-like parent chains
//!   and content-addressed snapshots (blake3).
//! - **TUI editor** (ratatui) provides an `n8n`-for-the-terminal experience:
//!   palette / canvas / inspector three-pane layout, modal editing, undo /
//!   redo, snapshot history.
//! - **Unified Rust API** (`WorkflowManager` + per-session `WorkflowClient`)
//!   is the only interface — the TUI, CLI, and any library consumer all
//!   call into the same async actor with `oneshot` reply channels.
//!
//! ## SOP enforcement
//!
//! Per user decision, SOP is enforced at two layers and *not* via a heavy
//! state machine:
//!
//! 1. **Prompt injection** — a `SopContext` stack prepends the workflow SOP,
//!    then each enclosing skill's `sop_text` in nested order, before any
//!    node's system prompt.
//! 2. **Node dependency graph** — every save runs `petgraph::toposort` and
//!    rejects cycles; at runtime, a node may only consume outputs from
//!    upstream connected nodes.
//!
//! See `plan` in the repository root for the full design.

#![warn(missing_docs)]
#![warn(rust_2018_idioms)]

pub mod api;
pub mod error;
pub mod executor;
pub mod model;
pub mod registry;
pub mod store;

#[cfg(feature = "tui")]
pub mod tui;

pub use error::{Result as WfResult, WorkflowError};

pub use api::{WorkflowClient, WorkflowCmd, WorkflowManager};
pub use executor::{
    NodeCtx, NodeExecutor, NodeReporter, PortInputs, PortOutputs, Scheduler, SopContext, SopGuard,
    SubgraphNode, ValidationReport, WorkflowResult,
};
pub use model::{
    CURRENT_SCHEMA_VERSION, EdgeEntry, NodeEntry, NodeKindInfo, PortSpec, Skill, SkillInfo,
    SnapshotInfo, Tool, Viewport, WorkflowManifest,
};
pub use registry::{NodeFactory, NodeRegistry, SkillFactory, SkillRegistry};
pub use store::{NodeKindRepo, SkillRepo, WorkflowRepo, WorkflowSummary};
