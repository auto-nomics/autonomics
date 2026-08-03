//! Turso (libSQL) storage layer for the bibliography management system.
//!
//! [`BibBase`] owns a single Turso [`Connection`] and provides:
//!
//! - **Article CRUD** — metadata, authors, identifiers.
//! - **Collections** — task-oriented grouping with rich per-article
//!   semantics (role, fetch status, provenance).
//! - **Full-text storage** — extracted plain text alongside binary files.
//! - **Search** — LIKE-based search across titles, abstracts, and
//!   full-text content.
//!
//! Depends on [`bib_types`] for the canonical data model. This crate
//! never depends on any SDK crate (eutils, gwascatalog-sdk, …) —
//! orchestration ("fetch then store") belongs in a higher-level crate.

pub mod bib_base;
pub mod collections;
pub mod error;
pub mod fulltext;

pub use bib_base::BibBase;
pub use error::{Error, Result};

// Re-export bib-types for convenience so downstream code can import
// types and storage from a single crate.
pub use bib_types::*;
