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
use std::sync::Arc;

use text_embedding::{EmbeddingInput, HashingTextEmbedder, TextEmbedder};

use crate::error::{Error, Result};
use crate::observation::Observation;

/// Default width of the hashing embedder's vectors.
pub use text_embedding::DEFAULT_EMBEDDING_DIM;

/// Provider contract for turning observations into dense vectors.
///
/// The API is synchronous because observation distillation runs as a
/// short batch operation. A remote provider can precompute and cache
/// vectors behind this trait without changing consumer-side clustering.
pub trait ObservationEmbedder: Send + Sync {
    /// The identity of the model this embedder serves. Vector caches
    /// key on it: a signature change invalidates every cached vector.
    fn signature(&self) -> String;

    /// Return one vector per input observation, keyed by observation id.
    fn embed(&self, observations: &[Observation]) -> Result<BTreeMap<String, Vec<f32>>>;
}

/// Wrap any shared text-level embedder behind the observation-level
/// contract, so a host-installed model (local ONNX, remote API)
/// drops into distillation without touching consumer code. This is
/// the ONE bridge: concrete embedders are `TextEmbedder`
/// implementations in `text-embedding`; everything
/// observation-specific (rendering, keying, error lifting) lives
/// here exactly once.
#[derive(Clone)]
pub struct SharedObservationEmbedder {
    inner: Arc<dyn TextEmbedder>,
}

impl std::fmt::Debug for SharedObservationEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedObservationEmbedder")
            .finish_non_exhaustive()
    }
}

impl SharedObservationEmbedder {
    pub fn new(inner: Arc<dyn TextEmbedder>) -> Self {
        Self { inner }
    }

    /// The offline deterministic default: the feature-hashing text
    /// embedder behind the shared bridge. Distillation falls back to
    /// this when no model is installed. Same vectors as the retired
    /// dedicated hashing embedder — one bridge means one bridging
    /// implementation, not two that can drift.
    pub fn hashing() -> Self {
        Self::new(Arc::new(HashingTextEmbedder::new()))
    }
}

impl ObservationEmbedder for SharedObservationEmbedder {
    fn signature(&self) -> String {
        self.inner.signature()
    }

    fn embed(&self, observations: &[Observation]) -> Result<BTreeMap<String, Vec<f32>>> {
        let inputs = observations
            .iter()
            .map(|observation| EmbeddingInput {
                id: observation.id.clone(),
                text: observation_text(observation),
            })
            .collect::<Vec<_>>();
        // The observation-level contract carries bare vectors: one
        // distillation pass runs exactly one embedder, so the model
        // signature cannot mix within a run — it stays a text-layer
        // concern.
        let outputs = into_core(self.inner.batch_embed(&inputs))?;
        Ok(outputs
            .into_iter()
            .map(|(id, output)| (id, output.vector))
            .collect())
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

/// An embedder with a warm cache in front of it.
///
/// Distillation hydrates this from
/// [`ObservationStorage::load_embedding`](crate::ObservationStorage)
/// before clustering: cache hits skip the model entirely, misses go
/// to the inner embedder once and are remembered in `computed` so the
/// caller can persist them afterwards. The cache view itself is
/// synchronous and pure — all database I/O stays at the hydration and
/// writeback edges, in the async caller.
pub struct CachedObservationEmbedder {
    inner: Arc<dyn ObservationEmbedder>,
    cache: BTreeMap<String, Vec<f32>>,
    computed: std::sync::Mutex<BTreeMap<String, Vec<f32>>>,
}

impl CachedObservationEmbedder {
    /// Wrap `inner` with preloaded vectors, keyed by observation id.
    pub fn new(inner: Arc<dyn ObservationEmbedder>, cache: BTreeMap<String, Vec<f32>>) -> Self {
        Self {
            inner,
            cache,
            computed: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    /// Drain the vectors computed since construction — the writeback
    /// set for [`ObservationStorage::save_embedding`].
    pub fn take_computed(&self) -> BTreeMap<String, Vec<f32>> {
        std::mem::take(&mut self.computed.lock().expect("computed lock poisoned"))
    }
}

impl ObservationEmbedder for CachedObservationEmbedder {
    fn signature(&self) -> String {
        self.inner.signature()
    }

    fn embed(&self, observations: &[Observation]) -> Result<BTreeMap<String, Vec<f32>>> {
        let missing: Vec<&Observation> = observations
            .iter()
            .filter(|observation| !self.cache.contains_key(&observation.id))
            .collect();
        let missing_refs: Vec<Observation> =
            missing.iter().map(|observation| (*observation).clone()).collect();
        let fresh = if missing_refs.is_empty() {
            BTreeMap::new()
        } else {
            self.inner.embed(&missing_refs)?
        };
        if let Ok(mut computed) = self.computed.lock() {
            computed.extend(fresh.iter().map(|(id, vector)| (id.clone(), vector.clone())));
        }
        let mut out = BTreeMap::new();
        for observation in observations {
            if let Some(vector) = self
                .cache
                .get(&observation.id)
                .or_else(|| fresh.get(&observation.id))
            {
                out.insert(observation.id.clone(), vector.clone());
            }
        }
        Ok(out)
    }
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
        let vectors = SharedObservationEmbedder::hashing()
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
        let first = SharedObservationEmbedder::hashing()
            .embed(&[observation("x", "identical text")])
            .unwrap();
        let second = SharedObservationEmbedder::hashing()
            .embed(&[observation("y", "identical text")])
            .unwrap();
        assert_eq!(first["x"], second["y"]);
    }

    #[test]
    fn cosine_detects_dimension_mismatch() {
        assert!(cosine(&[1.0], &[1.0, 0.0]).is_err());
    }

    #[test]
    fn mean_vector_requires_equal_dimensions() {
        assert!(mean_vector(&[&[1.0], &[1.0, 0.0]]).is_err());
    }

    /// The shared bridge works with any text-level provider — proven
    /// here with a precomputed vector map, no model required.
    #[test]
    fn shared_bridge_keys_vectors_by_observation_id() {
        use std::collections::BTreeMap as Map;
        let vectors = Map::from([
            ("o1".to_string(), vec![1.0, 0.0]),
            ("o2".to_string(), vec![0.0, 1.0]),
        ]);
        let embedder = SharedObservationEmbedder::new(Arc::new(
            text_embedding::VectorMapTextEmbedder::new("test-model", vectors),
        ));
        let out = embedder
            .embed(&[observation("o1", "a"), observation("o2", "b")])
            .unwrap();
        assert_eq!(out["o1"], vec![1.0, 0.0]);
        assert_eq!(out["o2"], vec![0.0, 1.0]);
    }
}
