//! LaTeX writing system: storage, AST operations, and LaTeX serialisation.
//!
//! This crate provides three layers:
//!
//! - **Storage** ([`store::WritingStore`]) — Turso (libSQL) CRUD for
//!   documents, versions, and templates.
//! - **AST operations** ([`ast`]) — apply [`EditOp`](writing_types::EditOp)
//!   sequences to the document tree with atomic rollback.
//! - **LaTeX serialisation** ([`serialize`]) — render the document AST into
//!   compilable `.tex` source.
//!
//! Depends on [`writing_types`] for the canonical data model.

pub mod ast;
pub mod citation;
pub mod compile;
pub mod error;
pub mod serialize;
pub mod shared;
pub mod store;
pub mod tools;

pub use citation::{CitationReport, CitationResolver, CiteKeyStatus};
pub use compile::{
    CompileInput, CompileIssue, CompileOutput, IssueSeverity, LatexEngine, NullEngine,
    XelatexEngine, compile_document, default_engine,
};
pub use error::{Error, Result};
pub use serialize::render_document;
pub use shared::WritingShared;
pub use store::{VersionSummary, WritingStore};
pub use tools::writing_all_registrations;
