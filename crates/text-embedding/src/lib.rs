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

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

pub mod error;

pub use error::{EmbeddingError, Result};

/// Default width of [`HashingTextEmbedder`] vectors.
pub const DEFAULT_EMBEDDING_DIM: usize = 256;

/// One text to embed: a caller-chosen id and the text itself.
///
/// The id flows through unchanged so callers can key the returned
/// vectors back to their records without positional bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingInput {
    pub id: String,
    pub text: String,
}

impl EmbeddingInput {
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }
}

/// Provider contract for turning texts into dense vectors.
///
/// The API is synchronous because consumers run it as a short batch
/// operation. A remote provider can precompute and cache vectors
/// behind this trait without changing consumer-side clustering.
pub trait TextEmbedder: Send + Sync {
    /// Return one vector per input, keyed by input id.
    fn embed(&self, inputs: &[EmbeddingInput]) -> Result<BTreeMap<String, Vec<f32>>>;
}

/// A deterministic feature-hashing embedder.
#[derive(Debug, Clone)]
pub struct HashingTextEmbedder {
    dim: usize,
}

impl HashingTextEmbedder {
    pub fn new() -> Self {
        Self::with_dim(DEFAULT_EMBEDDING_DIM)
    }

    pub fn with_dim(dim: usize) -> Self {
        Self { dim: dim.max(1) }
    }
}

impl Default for HashingTextEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEmbedder for HashingTextEmbedder {
    fn embed(&self, inputs: &[EmbeddingInput]) -> Result<BTreeMap<String, Vec<f32>>> {
        let mut out = BTreeMap::new();
        for input in inputs {
            let mut vector = vec![0.0_f32; self.dim];
            for token in tokens(&input.text) {
                add_feature(&mut vector, &token, 1.0);
                for ngram in char_ngrams(&token, 3) {
                    add_feature(&mut vector, &ngram, 0.25);
                }
            }
            normalize(&mut vector);
            out.insert(input.id.clone(), vector);
        }
        Ok(out)
    }
}

/// Wrap precomputed vectors so callers can substitute a remote model.
#[derive(Debug, Clone)]
pub struct VectorMapTextEmbedder {
    vectors: BTreeMap<String, Vec<f32>>,
}

impl VectorMapTextEmbedder {
    pub fn new(vectors: BTreeMap<String, Vec<f32>>) -> Self {
        Self { vectors }
    }

    /// The wrapped vectors, keyed by id. Providers that precompute
    /// elsewhere can read back what they seeded.
    pub fn vectors(&self) -> &BTreeMap<String, Vec<f32>> {
        &self.vectors
    }
}

impl TextEmbedder for VectorMapTextEmbedder {
    fn embed(&self, inputs: &[EmbeddingInput]) -> Result<BTreeMap<String, Vec<f32>>> {
        self.vectors
            .iter()
            .filter(|(id, _)| inputs.iter().any(|input| &input.id == *id))
            .map(|(id, vector)| Ok((id.clone(), vector.clone())))
            .collect()
    }
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
        return Err(EmbeddingError::DimensionMismatch {
            expected: left.len(),
            actual: right.len(),
        });
    }
    let dot = left.iter().zip(right).map(|(a, b)| a * b).sum::<f32>();
    let norm = left.iter().map(|v| v * v).sum::<f32>().sqrt()
        * right.iter().map(|v| v * v).sum::<f32>().sqrt();
    Ok(if norm > f32::EPSILON { dot / norm } else { 0.0 })
}

/// Average equal-length vectors. Clustering uses this for centroids
/// and for choosing a stable representative.
pub fn mean_vector(vectors: &[&[f32]]) -> Result<Vec<f32>> {
    let Some((first, rest)) = vectors.split_first() else {
        return Err(EmbeddingError::NoVectors);
    };
    let mut mean = first.to_vec();
    for vector in rest.iter().copied() {
        if vector.len() != mean.len() {
            return Err(EmbeddingError::DimensionMismatch {
                expected: mean.len(),
                actual: vector.len(),
            });
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
    fn hashing_embedder_is_deterministic_and_normalized() {
        let embedder = HashingTextEmbedder::new();
        let text = "restart the daemon after config change";
        let first = embedder.embed(&[EmbeddingInput::new("a", text)]).unwrap();
        // Same text under a different id, fresh embedder instance —
        // the vector must not move.
        let second = embedder.embed(&[EmbeddingInput::new("b", text)]).unwrap();
        assert_eq!(first["a"], second["b"]);
        let norm = first["a"].iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);
    }

    #[test]
    fn different_texts_embed_differently() {
        let embedder = HashingTextEmbedder::new();
        let out = embedder
            .embed(&[
                EmbeddingInput::new("a", "restart the daemon"),
                EmbeddingInput::new("b", "quote mixed-case column names"),
            ])
            .unwrap();
        assert_ne!(out["a"], out["b"]);
    }

    #[test]
    fn vector_map_only_returns_requested_ids() {
        let embedder = VectorMapTextEmbedder::new(BTreeMap::from([
            ("a".to_string(), vec![1.0, 0.0]),
            ("b".to_string(), vec![0.0, 1.0]),
        ]));
        let out = embedder
            .embed(&[EmbeddingInput::new("b", "text is ignored")])
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out["b"], vec![0.0, 1.0]);
        assert_eq!(embedder.vectors().len(), 2);
    }

    #[test]
    fn cosine_detects_dimension_mismatch() {
        assert!(matches!(
            cosine(&[1.0], &[1.0, 0.0]),
            Err(EmbeddingError::DimensionMismatch { .. })
        ));
    }

    #[test]
    fn cosine_of_zero_vectors_is_zero_not_nan() {
        assert_eq!(cosine(&[0.0, 0.0], &[0.0, 0.0]).unwrap(), 0.0);
    }

    #[test]
    fn mean_vector_requires_equal_dimensions() {
        assert!(mean_vector(&[&[1.0], &[1.0, 0.0]]).is_err());
    }

    #[test]
    fn mean_vector_rejects_an_empty_batch() {
        assert!(matches!(
            mean_vector(&[]),
            Err(EmbeddingError::NoVectors)
        ));
    }

    #[test]
    fn mean_vector_averages_members() {
        assert_eq!(mean_vector(&[&[1.0, 3.0], &[3.0, 1.0]]).unwrap(), [2.0, 2.0]);
    }
}
