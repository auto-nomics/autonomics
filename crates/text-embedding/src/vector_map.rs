//! The precomputed-vector wrapper: substitution without a model.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::input::EmbeddingInput;
use crate::provider::TextEmbedder;

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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn vector_map_returns_nothing_for_unknown_ids() {
        let embedder = VectorMapTextEmbedder::new(BTreeMap::new());
        let out = embedder
            .embed(&[EmbeddingInput::new("missing", "text")])
            .unwrap();
        assert!(out.is_empty());
    }
}
