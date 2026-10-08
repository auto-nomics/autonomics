//! The provider contract every embedder implements.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::input::EmbeddingInput;

/// Provider contract for turning texts into dense vectors.
///
/// The API is synchronous because consumers run it as a short batch
/// operation. A remote provider can precompute and cache vectors
/// behind this trait without changing consumer-side clustering.
pub trait TextEmbedder: Send + Sync {
    /// Return one vector per input, keyed by input id.
    fn embed(&self, inputs: &[EmbeddingInput]) -> Result<BTreeMap<String, Vec<f32>>>;
}
