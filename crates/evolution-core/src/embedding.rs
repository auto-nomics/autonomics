//! Observation embeddings shared by evolution consumers.
//!
//! The default embedder is deliberately local and deterministic: it
//! maps words and character n-grams into a fixed-width feature vector.
//! That keeps evolution loops usable offline while preserving the
//! provider boundary needed for a remote text-embedding model later.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::observation::Observation;

/// Default width of [`HashingObservationEmbedder`] vectors.
pub const DEFAULT_EMBEDDING_DIM: usize = 256;

/// Provider contract for turning observations into dense vectors.
///
/// The API is synchronous because observation distillation runs as a
/// short batch operation. A remote provider can precompute and cache
/// vectors behind this trait without changing consumer-side clustering.
pub trait ObservationEmbedder: Send + Sync {
    /// Return one vector per input observation, keyed by observation id.
    fn embed(&self, observations: &[Observation]) -> Result<BTreeMap<String, Vec<f32>>>;
}

/// A deterministic feature-hashing embedder.
#[derive(Debug, Clone)]
pub struct HashingObservationEmbedder {
    dim: usize,
}

impl HashingObservationEmbedder {
    pub fn new() -> Self {
        Self::with_dim(DEFAULT_EMBEDDING_DIM)
    }

    pub fn with_dim(dim: usize) -> Self {
        Self { dim: dim.max(1) }
    }
}

impl Default for HashingObservationEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl ObservationEmbedder for HashingObservationEmbedder {
    fn embed(&self, observations: &[Observation]) -> Result<BTreeMap<String, Vec<f32>>> {
        let mut out = BTreeMap::new();
        for observation in observations {
            let mut vector = vec![0.0_f32; self.dim];
            let text = observation_text(observation);
            for token in tokens(&text) {
                add_feature(&mut vector, &token, 1.0);
                for ngram in char_ngrams(&token, 3) {
                    add_feature(&mut vector, &ngram, 0.25);
                }
            }
            normalize(&mut vector);
            out.insert(observation.id.clone(), vector);
        }
        Ok(out)
    }
}

/// Wrap precomputed vectors so callers can substitute a remote model.
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

fn add_feature(vector: &mut [f32], feature: &str, weight: f32) {
    let mut hasher = Sha256::new();
    hasher.update(feature.as_bytes());
    let digest = hasher.finalize();
    let slot =
        u64::from_be_bytes(digest[..8].try_into().expect("sha256 prefix")) as usize % vector.len();
    vector[slot] += weight;
}

fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in vector {
            *value /= norm;
        }
    }
}

fn tokens(text: &str) -> impl Iterator<Item = String> {
    const STOP_WORDS: [&str; 18] = [
        "a", "an", "and", "are", "as", "at", "be", "for", "from", "in", "is", "of", "on", "or",
        "the", "this", "to", "when",
    ];
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| token.len() > 1)
        .map(str::to_ascii_lowercase)
        .filter(move |token| !STOP_WORDS.contains(&token.as_str()))
}

fn char_ngrams(token: &str, n: usize) -> impl Iterator<Item = String> {
    let chars: Vec<char> = token.chars().collect();
    let n = n.max(1);
    (0..chars.len().saturating_sub(n.saturating_sub(1)))
        .map(move |start| chars[start..start + n].iter().collect())
}

/// Cosine similarity for two equal-length vectors.
pub fn cosine(left: &[f32], right: &[f32]) -> Result<f32> {
    if left.len() != right.len() {
        return Err(crate::Error::Embedding(format!(
            "embedding dimension mismatch: {} != {}",
            left.len(),
            right.len()
        )));
    }
    let dot = left.iter().zip(right).map(|(a, b)| a * b).sum::<f32>();
    let norm = left.iter().map(|v| v * v).sum::<f32>().sqrt()
        * right.iter().map(|v| v * v).sum::<f32>().sqrt();
    Ok(if norm > f32::EPSILON { dot / norm } else { 0.0 })
}

/// Average equal-length vectors. Clustering uses this for centroids and
/// for choosing a stable semantic representative.
pub fn mean_vector(vectors: &[&[f32]]) -> Result<Vec<f32>> {
    let Some((first, rest)) = vectors.split_first() else {
        return Err(crate::Error::Embedding("cannot average no vectors".into()));
    };
    let mut mean = first.to_vec();
    for vector in rest.iter().copied() {
        if vector.len() != mean.len() {
            return Err(crate::Error::Embedding(
                "embedding dimension mismatch".into(),
            ));
        }
        for (sum, value) in mean.iter_mut().zip(vector) {
            *sum += value;
        }
    }
    let count = vectors.len() as f32;
    for value in &mut mean {
        *value /= count;
    }
    Ok(mean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_detects_dimension_mismatch() {
        assert!(cosine(&[1.0], &[1.0, 0.0]).is_err());
    }

    #[test]
    fn mean_vector_requires_equal_dimensions() {
        assert!(mean_vector(&[&[1.0], &[1.0, 0.0]]).is_err());
    }
}
