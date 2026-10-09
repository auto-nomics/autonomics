//! The offline deterministic default: feature hashing.
//!
//! Words and character n-grams are hashed into a fixed-width vector
//! and L2-normalized. No model, no network, no state — the same text
//! yields the same vector on every machine and every run, which is
//! what downstream cluster hashes rely on.

use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::input::EmbeddingInput;
use crate::output::EmbeddingOutput;
use crate::provider::TextEmbedder;
use crate::vector::normalize;

use std::collections::BTreeMap;

/// Default width of [`HashingTextEmbedder`] vectors.
pub const DEFAULT_EMBEDDING_DIM: usize = 256;

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

    /// The model signature stamped on every vector this embedder
    /// produces. Width is part of the identity: different widths
    /// yield incomparable vectors.
    pub fn signature(&self) -> String {
        format!("hashing-{}", self.dim)
    }
}

impl Default for HashingTextEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEmbedder for HashingTextEmbedder {
    fn embed(&self, input: &EmbeddingInput) -> Result<EmbeddingOutput> {
        let mut vector = vec![0.0_f32; self.dim];
        for token in tokens(&input.text) {
            add_feature(&mut vector, &token, 1.0);
            for ngram in char_ngrams(&token, 3) {
                add_feature(&mut vector, &ngram, 0.25);
            }
        }
        normalize(&mut vector);
        Ok(EmbeddingOutput::new(self.signature(), vector))
    }

    // batch_embed: the default loop is exact for a stateless local
    // function — there is no batch API whose cost to amortize.
}

fn add_feature(vector: &mut [f32], feature: &str, weight: f32) {
    let mut hasher = Sha256::new();
    hasher.update(feature.as_bytes());
    let digest = hasher.finalize();
    let slot =
        u64::from_be_bytes(digest[..8].try_into().expect("sha256 prefix")) as usize % vector.len();
    vector[slot] += weight;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashing_embedder_is_deterministic_and_normalized() {
        let embedder = HashingTextEmbedder::new();
        let text = "restart the daemon after config change";
        let first = embedder.embed(&EmbeddingInput::new("a", text)).unwrap();
        // Same text under a different id, fresh embedder instance —
        // the vector must not move.
        let second = embedder.embed(&EmbeddingInput::new("b", text)).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.model_signature, "hashing-256");
        let norm = first.vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);
    }

    #[test]
    fn different_texts_embed_differently() {
        let embedder = HashingTextEmbedder::new();
        let out = embedder
            .embed(&EmbeddingInput::new("a", "restart the daemon"))
            .unwrap();
        let other = embedder
            .embed(&EmbeddingInput::new(
                "b",
                "quote mixed-case column names",
            ))
            .unwrap();
        assert_ne!(out.vector, other.vector);
    }

    /// The batch entry must agree with the single entry — clustering
    /// mixes both (bridge passes batches, remote providers may embed
    /// one at a time through the default loop).
    #[test]
    fn batch_embed_matches_single_embeds() {
        let embedder = HashingTextEmbedder::new();
        let inputs = [
            EmbeddingInput::new("a", "restart the daemon"),
            EmbeddingInput::new("b", "bump the timeout"),
        ];
        let batch = embedder.batch_embed(&inputs).unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch["a"], embedder.embed(&inputs[0]).unwrap());
        assert_eq!(batch["b"], embedder.embed(&inputs[1]).unwrap());
    }

    #[test]
    fn with_dim_clamps_to_at_least_one_slot() {
        let embedder = HashingTextEmbedder::with_dim(0);
        let out = embedder.embed(&EmbeddingInput::new("a", "text")).unwrap();
        assert_eq!(out.vector.len(), 1);
    }

    /// The width is part of the model identity — vectors of different
    /// widths cannot be compared, so the signature must say which.
    #[test]
    fn signature_includes_the_width() {
        assert_eq!(
            HashingTextEmbedder::with_dim(64).signature(),
            "hashing-64"
        );
        let out = HashingTextEmbedder::with_dim(64)
            .embed(&EmbeddingInput::new("a", "text"))
            .unwrap();
        assert_eq!(out.model_signature, "hashing-64");
    }
}
