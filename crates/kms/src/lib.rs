//! Knowledge Management System (KMS) — tree-shaped knowledge index on Turso.
//!
//! Ported from the [dendrite](https://github.com/wjixiang/dendrite) project.
//! The KMS uses a three-layer data model:
//!
//! - **Entity** — the subject being discussed (has ≥1 nomenclature).
//! - **Knowledge** — a record about entities (`aspect` or `relation`).
//! - **Index** — tree-shaped organization (group nodes + knowledge links).
//!
//! A diagnostic system acts like a compiler type-checker: after every
//! mutation, 13 rules are re-run and structural violations are fed back
//! to the agent.

pub mod diagnostics;
mod language;
pub mod service;
pub mod storage;
pub mod view;

pub use diagnostics::{CodeDescription, Diagnostic, Severity};
pub use language::Language;
pub use service::{
    BatchKnowledgeResult, BatchStatus, EntityFilter, KmsDocumentStore, KmsService,
    KnowledgeContentHit, KnowledgeView,
};
pub use storage::Storage;
pub use storage::error::StorageError;
pub use storage::types::{Entity, Index, Knowledge, KnowledgeType, Nomenclature, TargetType};
pub use view::{IndexView, LocalView, SUBTREE_TITLES_LIMIT, SubtreeSummary};

#[cfg(test)]
mod tests;
