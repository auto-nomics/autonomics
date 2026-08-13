//! Persistence layer — SQLite + rusqlite + r2d2 migrations.
//!
//! Three repositories live here, all wrapping a shared [`pool::DbPool`]:
//!
//! - [`WorkflowRepo`] — mutable workflow manifests + git-style snapshot chain.
//! - [`SkillRepo`] — versioned reusable DAG subgraphs.
//! - [`NodeKindRepo`] — registry cache for node kinds (powers the TUI palette).
//!
//! All repo methods are **synchronous**. The async API surface in
//! [`crate::api`] wraps these in `tokio::task::spawn_blocking` so callers
//! see a consistent async view.

pub mod migrations;
pub mod node_kind_repo;
pub mod pool;
pub mod skill_repo;
pub mod workflow_repo;

pub use migrations::run;
pub use node_kind_repo::NodeKindRepo;
pub use pool::{DbPool, open, open_file};
pub use skill_repo::SkillRepo;
pub use workflow_repo::{WorkflowRepo, WorkflowSummary};
