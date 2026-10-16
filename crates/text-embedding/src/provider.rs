//! The provider contract every embedder implements, plus the
//! providers that ship with it: the offline deterministic
//! [`Bm25TextEmbedder`] and the remote Ollama model.

mod bm25;
pub mod error;
mod ollama;

use std::collections::BTreeMap;

use crate::input::EmbeddingInput;
use crate::output::EmbeddingOutput;
use crate::provider::error::Result;

pub use bm25::{Bm25TextEmbedder, DEFAULT_EMBEDDING_DIM};
pub use ollama::OllamaEmbedder;

use async_trait::async_trait;

/// Provider contract for turning texts into dense vectors.
///
/// The API is synchronous because consumers run it as a short batch
/// operation. A remote provider can precompute and cache vectors
/// behind this trait without changing consumer-side clustering.
///
/// Every result carries the model signature of the model that
/// produced it: embeddings are tightly bound to their model, and
/// vectors from different signatures must never be mixed or compared.
#[async_trait]
pub trait TextEmbedder: Send + Sync {
    /// The identity of the model this embedder serves. Vectors from
    /// different signatures are not comparable; caches and stores
    /// key on it to detect mixing.
    fn signature(&self) -> String;

    /// Embed one text and return its vector stamped with the model
    /// signature.
    ///
    /// The required primitive — a new provider starts here.
    async fn embed(&self, input: &EmbeddingInput) -> Result<EmbeddingOutput>;

    /// Embed a batch and return one output per input, keyed by input
    /// id; every output carries the same model signature.
    ///
    /// The default loops [`TextEmbedder::embed`]. Providers with a
    /// true batch API — one remote call, one model pass — should
    /// override this to amortize per-request cost; the loop is only
    /// correct-by-default, never fast.
    async fn batch_embed(
        &self,
        inputs: &[EmbeddingInput],
    ) -> Result<BTreeMap<String, EmbeddingOutput>> {
        let mut outputs = BTreeMap::new();
        for input in inputs {
            let output = self.embed(input).await?;
            outputs.insert(input.id.clone(), output);
        }
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    /// A provider implementing only `embed` gets a working batch for
    /// free — the contract new providers start from.
    struct SingleOnly;

    #[async_trait]
    impl TextEmbedder for SingleOnly {
        fn signature(&self) -> String {
            "single-only".into()
        }

        async fn embed(&self, input: &EmbeddingInput) -> Result<EmbeddingOutput> {
            Ok(EmbeddingOutput::new(
                "single-only",
                vec![input.text.len() as f32],
            ))
        }
    }

    #[tokio::test]
    async fn default_batch_loops_single_embeds_and_propagates_signatures() {
        let out = SingleOnly
            .batch_embed(&[
                EmbeddingInput::new("a", "abc"),
                EmbeddingInput::new("b", "de"),
            ])
            .await
            .unwrap();
        assert_eq!(out["a"].vector, vec![3.0]);
        assert_eq!(out["b"].vector, vec![2.0]);
        assert_eq!(out["a"].model_signature, "single-only");
        assert_eq!(out["b"].model_signature, "single-only");
    }

    #[tokio::test]
    async fn default_batch_of_nothing_is_an_empty_map() {
        assert!(SingleOnly.batch_embed(&[]).await.unwrap().is_empty());
    }
}
