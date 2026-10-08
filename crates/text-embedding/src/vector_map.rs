//! The precomputed-vector wrapper: substitution without a model.

use std::collections::BTreeMap;

use crate::error::{EmbeddingError, Result};
use crate::input::EmbeddingInput;
use crate::output::EmbeddingOutput;
use crate::provider::TextEmbedder;

/// Wrap precomputed vectors so callers can substitute a remote model.
///
/// The caller names the model that produced the vectors — the
/// wrapper only stamps that signature on every result. Lookups key on
/// id only; the text of an [`EmbeddingInput`] is never read. Single
/// lookups are strict — a missing id is an error — while batches are
/// lenient: a precomputed map may cover a superset of what the
/// caller requests, so unknown ids are skipped rather than failing
/// the whole batch.
#[derive(Debug, Clone)]
pub struct VectorMapTextEmbedder {
    model_signature: String,
    vectors: BTreeMap<String, Vec<f32>>,
}

impl VectorMapTextEmbedder {
    pub fn new(model_signature: impl Into<String>, vectors: BTreeMap<String, Vec<f32>>) -> Self {
        Self {
            model_signature: model_signature.into(),
            vectors,
        }
    }

    /// The wrapped vectors, keyed by id. Providers that precompute
    /// elsewhere can read back what they seeded.
    pub fn vectors(&self) -> &BTreeMap<String, Vec<f32>> {
        &self.vectors
    }
}

impl TextEmbedder for VectorMapTextEmbedder {
    fn embed(&self, input: &EmbeddingInput) -> Result<EmbeddingOutput> {
        self.vectors
            .get(&input.id)
            .cloned()
            .map(|vector| EmbeddingOutput::new(&self.model_signature, vector))
            .ok_or_else(|| EmbeddingError::MissingVector {
                id: input.id.clone(),
            })
    }

    fn batch_embed(&self, inputs: &[EmbeddingInput]) -> Result<BTreeMap<String, EmbeddingOutput>> {
        self.vectors
            .iter()
            .filter(|(id, _)| inputs.iter().any(|input| &input.id == *id))
            .map(|(id, vector)| {
                Ok((
                    id.clone(),
                    EmbeddingOutput::new(&self.model_signature, vector.clone()),
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedder() -> VectorMapTextEmbedder {
        VectorMapTextEmbedder::new(
            "precomputed-v1",
            BTreeMap::from([
                ("a".to_string(), vec![1.0, 0.0]),
                ("b".to_string(), vec![0.0, 1.0]),
            ]),
        )
    }

    #[test]
    fn vector_map_only_returns_requested_ids() {
        let out = embedder()
            .batch_embed(&[EmbeddingInput::new("b", "text is ignored")])
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out["b"].vector, vec![0.0, 1.0]);
        assert_eq!(embedder().vectors().len(), 2);
    }

    #[test]
    fn vector_map_returns_nothing_for_unknown_ids() {
        let out = embedder()
            .batch_embed(&[EmbeddingInput::new("missing", "text")])
            .unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn single_lookup_is_strict_on_unknown_ids() {
        assert!(matches!(
            embedder().embed(&EmbeddingInput::new("missing", "text")),
            Err(EmbeddingError::MissingVector { id }) if id == "missing"
        ));
    }

    /// The signature named at construction rides on every result —
    /// the wrapper's whole job is stamping provenance.
    #[test]
    fn every_output_carries_the_constructor_signature() {
        let embedder = embedder();
        let single = embedder.embed(&EmbeddingInput::new("a", "text")).unwrap();
        assert_eq!(single.model_signature, "precomputed-v1");
        let batch = embedder
            .batch_embed(&[EmbeddingInput::new("a", "x"), EmbeddingInput::new("b", "y")])
            .unwrap();
        assert!(batch.values().all(|o| o.model_signature == "precomputed-v1"));
    }
}
