//! Local embedding-model service: install from the Hub, then serve.
//!
//! [`LocalEmbeddingService::init_at`] opens an already-installed model
//! — installing is `autonomics prepare`'s job, never the daemon's.
//! [`ensure_installed`] is the install policy: verify (completion
//! marker + file sizes), heal a legacy marker-less install by opening
//! it once, otherwise download atomically — the snapshot lands in a
//! sibling staging directory that is renamed into place only after
//! the full file set verified, so an interrupted download never looks
//! installed.

pub mod error;
pub mod local_embedding_service;
