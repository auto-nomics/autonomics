//! Domain-neutral text embedding providers.
//!
//! An embedding model turns text into dense vectors; this crate owns
//! that boundary and nothing else. It does not know about
//! observations, skills, or any other domain record — callers map
//! their records into [`EmbeddingInput`] pairs and cluster the
//! returned vectors themselves.
//!
//! The default [`HashingTextEmbedder`] is deliberately local and
//! deterministic: it maps words and character n-grams into a
//! fixed-width feature vector. That keeps consumers usable offline
//! while the [`TextEmbedder`] contract preserves the provider
//! boundary needed for a remote text-embedding model later.
//!
//! Layout:
//!
//! - [`provider`] — the [`TextEmbedder`] contract every embedder
//!   implements; a new provider (remote API, local ONNX) is a new
//!   module implementing it
//! - [`hashing`] — the offline deterministic default
//! - [`vector_map`] — precomputed-vector wrapper for substitution
//! - [`vector`] — dimension-checked vector math used by clustering
//! - [`input`] / [`output`] / [`error`] — the input record, the
//!   model-stamped output record, and the error type

pub mod error;
pub mod hashing;
pub mod input;
pub mod output;
pub mod provider;
pub mod vector;
pub mod vector_map;

pub use error::{EmbeddingError, Result};
pub use hashing::{HashingTextEmbedder, DEFAULT_EMBEDDING_DIM};
pub use input::EmbeddingInput;
pub use output::EmbeddingOutput;
#[cfg(feature = "onnx")]
pub use provider::OnnxTextEmbedder;
pub use provider::TextEmbedder;
pub use vector::{cosine, mean_vector};
pub use vector_map::VectorMapTextEmbedder;
