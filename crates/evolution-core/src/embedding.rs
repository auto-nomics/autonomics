//! Observation embeddings: the bridge from canonical observation
//! records to the domain-neutral `text-embedding` crate.
//!
//! Embedding-model invocation itself lives in `text-embedding` —
//! provider contract, the deterministic hashing implementation, and
//! vector math. This module owns only what is observation-specific:
//! rendering an [`Observation`] to plain text, and the
//! observation-keyed [`ObservationEmbedder`] contract consumers
//! cluster against. The default embedder is deliberately local and
//! deterministic, keeping evolution loops usable offline while
//! preserving the provider boundary needed for a remote
//! text-embedding model later.

use std::collections::BTreeMap;

use text_embedding::{EmbeddingInput, HashingTextEmbedder, TextEmbedder};

use crate::error::{Error, Result};
use crate::observation::Observation;

/// Default width of [`HashingObservationEmbedder`] vectors.
pub use text_embedding::DEFAULT_EMBEDDING_DIM;

/// Provider contract for turning observations into dense vectors.
///
/// The API is synchronous because observation distillation runs as a
/// short batch operation. A remote provider can precompute and cache
/// vectors behind this trait without changing consumer-side clustering.
pub trait ObservationEmbedder: Send + Sync {
    /// Return one vector per input observation, keyed by observation id.
    fn embed(&self, observations: &[Observation]) -> Result<BTreeMap<String, Vec<f32>>>;
}

/// A deterministic feature-hashing embedder, over the shared
/// text-level implementation.
#[derive(Debug, Clone)]
pub struct HashingObservationEmbedder {
    inner: HashingTextEmbedder,
}

impl HashingObservationEmbedder {
    pub fn new() -> Self {
        Self::with_dim(DEFAULT_EMBEDDING_DIM)
    }

    pub fn with_dim(dim: usize) -> Self {
        Self {
            inner: HashingTextEmbedder::with_dim(dim),
        }
    }
}

impl Default for HashingObservationEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl ObservationEmbedder for HashingObservationEmbedder {
    fn embed(&self, observations: &[Observation]) -> Result<BTreeMap<String, Vec<f32>>> {
        let inputs = observations
            .iter()
            .map(|observation| EmbeddingInput {
                id: observation.id.clone(),
                text: observation_text(observation),
            })
            .collect::<Vec<_>>();
        into_core(self.inner.embed(&inputs))
    }
}

/// Wrap precomputed vectors so callers can substitute a remote model.
///
/// This is pure plumbing — ids in, vectors out — so it lives here
/// rather than in `text-embedding`, where it would pretend to look at
/// text it never reads.
#[derive(Debug, Clone)]
pub struct VectorMapEmbedder {
    vectors: BTreeMap<String, Vec<f32>>,
}

impl VectorMapEmbedder {
    pub fn new(vectors: BTreeMap<String, Vec<f32>>) -> Self {
        Self { vectors }
    }
}

impl ObservationEmbedder for VectorMapEmbedder {
    fn embed(&self, observations: &[Observation]) -> Result<BTreeMap<String, Vec<f32>>> {
        self.vectors
            .iter()
            .filter(|(id, _)| observations.iter().any(|o| &o.id == *id))
            .map(|(id, vector)| Ok((id.clone(), vector.clone())))
            .collect()
    }
}

/// The observation text an embedder sees: the structural anchors
/// (kind, node kind) plus the free-form signal (summary, body, error).
fn observation_text(observation: &Observation) -> String {
    let mut parts = vec![
        observation.kind.label(),
        observation.node_kind.as_deref().unwrap_or(""),
        &observation.summary,
        &observation.body,
    ];
    if let Some(error) = observation.error.as_deref() {
        parts.push(error);
    }
    parts.join("\n")
}

/// Lift a text-level embedding result into the crate error type.
fn into_core<T>(result: text_embedding::Result<T>) -> Result<T> {
    result.map_err(|e| Error::Embedding(e.to_string()))
}

/// Cosine similarity for two equal-length vectors.
pub fn cosine(left: &[f32], right: &[f32]) -> Result<f32> {
    into_core(text_embedding::cosine(left, right))
}

/// Average equal-length vectors. Clustering uses this for centroids and
/// for choosing a stable semantic representative.
pub fn mean_vector(vectors: &[&[f32]]) -> Result<Vec<f32>> {
    into_core(text_embedding::mean_vector(vectors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{ObservationKind, ObservationSource};

    fn observation(id: &str, summary: &str) -> Observation {
        Observation {
            id: id.to_string(),
            created_at: 0,
            kind: ObservationKind::Failure,
            source: ObservationSource::Agent,
            summary: summary.to_string(),
            body: String::new(),
            node_kind: None,
            error: None,
        }
    }

    #[test]
    fn hashing_embedder_keys_vectors_by_observation_id() {
        let observations = [
            observation("o1", "restart the daemon"),
            observation("o2", "bump the timeout"),
        ];
        let vectors = HashingObservationEmbedder::new()
            .embed(&observations)
            .unwrap();
        assert_eq!(vectors.len(), 2);
        assert!(vectors.contains_key("o1"));
        assert!(vectors.contains_key("o2"));
    }

    /// Same text under different ids and fresh embedder instances must
    /// land on the same vector — distillation's cluster hashes rely on
    /// this determinism across runs.
    #[test]
    fn same_text_embeds_identically_across_instances() {
        let first = HashingObservationEmbedder::new()
            .embed(&[observation("x", "identical text")])
            .unwrap();
        let second = HashingObservationEmbedder::new()
            .embed(&[observation("y", "identical text")])
            .unwrap();
        assert_eq!(first["x"], second["y"]);
    }

    #[test]
    fn vector_map_filters_by_observation_id() {
        let embedder = VectorMapEmbedder::new(BTreeMap::from([(
            "o1".to_string(),
            vec![1.0, 0.0],
        )]));
        let out = embedder
            .embed(&[observation("o1", "a"), observation("o2", "b")])
            .unwrap();
        assert_eq!(out.keys().collect::<Vec<_>>(), ["o1"]);
    }

    #[test]
    fn cosine_detects_dimension_mismatch() {
        assert!(cosine(&[1.0], &[1.0, 0.0]).is_err());
    }

    #[test]
    fn mean_vector_requires_equal_dimensions() {
        assert!(mean_vector(&[&[1.0], &[1.0, 0.0]]).is_err());
    }
}
