//! The provider contract every embedder implements.

mod onnx;

use std::collections::BTreeMap;

use crate::error::Result;
use crate::input::EmbeddingInput;
use crate::output::EmbeddingOutput;

/// Provider contract for turning texts into dense vectors.
///
/// The API is synchronous because consumers run it as a short batch
/// operation. A remote provider can precompute and cache vectors
/// behind this trait without changing consumer-side clustering.
///
/// Every result carries the model signature of the model that
/// produced it: embeddings are tightly bound to their model, and
/// vectors from different signatures must never be mixed or compared.
pub trait TextEmbedder: Send + Sync {
    /// Embed one text and return its vector stamped with the model
    /// signature.
    ///
    /// The required primitive — a new provider starts here.
    fn embed(&self, input: &EmbeddingInput) -> Result<EmbeddingOutput>;

    /// Embed a batch and return one output per input, keyed by input
    /// id; every output carries the same model signature.
    ///
    /// The default loops [`TextEmbedder::embed`]. Providers with a
    /// true batch API — one remote call, one model pass — should
    /// override this to amortize per-request cost; the loop is only
    /// correct-by-default, never fast.
    fn batch_embed(&self, inputs: &[EmbeddingInput]) -> Result<BTreeMap<String, EmbeddingOutput>> {
        inputs
            .iter()
            .map(|input| self.embed(input).map(|output| (input.id.clone(), output)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider implementing only `embed` gets a working batch for
    /// free — the contract new providers start from.
    struct SingleOnly;

    impl TextEmbedder for SingleOnly {
        fn embed(&self, input: &EmbeddingInput) -> Result<EmbeddingOutput> {
            Ok(EmbeddingOutput::new(
                "single-only",
                vec![input.text.len() as f32],
            ))
        }
    }

    #[test]
    fn default_batch_loops_single_embeds_and_propagates_signatures() {
        let out = SingleOnly
            .batch_embed(&[
                EmbeddingInput::new("a", "abc"),
                EmbeddingInput::new("b", "de"),
            ])
            .unwrap();
        assert_eq!(out["a"].vector, vec![3.0]);
        assert_eq!(out["b"].vector, vec![2.0]);
        assert_eq!(out["a"].model_signature, "single-only");
        assert_eq!(out["b"].model_signature, "single-only");
    }

    #[test]
    fn default_batch_of_nothing_is_an_empty_map() {
        assert!(SingleOnly.batch_embed(&[]).unwrap().is_empty());
    }
}
