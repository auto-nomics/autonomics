//! The record embedders return: a dense vector stamped with its model.

/// One embedding result: the vector and the signature of the model
/// that produced it.
///
/// Embeddings are tightly bound to their model — vectors from
/// different signatures have different geometries and must never be
/// compared or mixed. The signature therefore travels on every
/// result, so caches and stores can detect mixing at the record
/// level instead of trusting surrounding context.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingOutput {
    pub model_signature: String,
    pub vector: Vec<f32>,
}

impl EmbeddingOutput {
    pub fn new(model_signature: impl Into<String>, vector: Vec<f32>) -> Self {
        Self {
            model_signature: model_signature.into(),
            vector,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_accepts_into_signatures() {
        let output = EmbeddingOutput::new(String::from("hashing-256"), vec![1.0]);
        assert_eq!(output.model_signature, "hashing-256");
        assert_eq!(output.vector, vec![1.0]);
    }
}
