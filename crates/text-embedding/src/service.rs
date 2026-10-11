//! Local embedding-model service: install from the Hub, then serve.
//!
//! [`LocalEmbeddingService::init`] is the one-call bootstrap — it
//! downloads the default model on first use and opens the ONNX
//! embedder over it. Installs are idempotent: an existing model
//! directory is opened as-is, and [`install_default_model`] only
//! fetches files the Hub snapshot is missing.

pub mod error;
pub mod local_embedding_service;
