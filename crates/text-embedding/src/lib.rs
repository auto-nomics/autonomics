//! Domain-neutral text embedding providers.
//!
//! An embedding model turns text into dense vectors; this crate owns
//! that boundary and nothing else. It does not know about
//! observations, skills, or any other domain record — callers map
//! their records into [`EmbeddingInput`] pairs and cluster the
//! returned vectors themselves.
//!
//! The default [`Bm25TextEmbedder`] is deliberately local and
//! deterministic: unfitted it maps words and character n-grams into
//! a fixed-width feature histogram, and a host that has a corpus
//! can freeze BM25 statistics into it for weighted vectors. That
//! keeps consumers usable offline while the [`TextEmbedder`]
//! contract preserves the provider boundary needed for a remote
//! text-embedding model later.
//!
//! Layout:
//!
//! - [`provider`] — the [`TextEmbedder`] contract every embedder
//!   implements, plus the providers under it as modules:
//!   [`Bm25TextEmbedder`] (the offline deterministic default) and
//!   [`OnnxTextEmbedder`] (the local ONNX model). A new provider
//!   (remote API) is a new module under it implementing the contract
//! - [`service`] — install the default ONNX model from the Hub and
//!   serve it as a [`LocalEmbeddingService`]
//! - [`vector_map`] — precomputed-vector wrapper for substitution
//! - [`vector`] — dimension-checked vector math used by clustering
//! - [`input`] / [`output`] / [`error`] — the input record, the
//!   model-stamped output record, and the error type

pub mod error;
pub mod input;
pub mod output;
pub mod provider;
pub mod service;
pub mod vector;
pub mod vector_map;

pub use error::{Error, Result};
pub use input::EmbeddingInput;
pub use output::EmbeddingOutput;
pub use provider::{Bm25TextEmbedder, DEFAULT_EMBEDDING_DIM, OnnxTextEmbedder, TextEmbedder};
pub use service::local_embedding_service::{LocalEmbeddingService, default_model_dir};
pub use vector::{cosine, mean_vector};
pub use vector_map::VectorMapTextEmbedder;
