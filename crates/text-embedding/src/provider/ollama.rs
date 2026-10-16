//! Ollama `/api/embed` provider.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::input::EmbeddingInput;
use crate::output::EmbeddingOutput;
use crate::provider::TextEmbedder;
use crate::provider::error::{Error, Result};
use crate::vector::normalize;

#[derive(Debug, Clone)]
pub struct OllamaEmbedder {
    base_url: String,
    model: String,
    batch_size: usize,
    http: reqwest::Client,
}

impl OllamaEmbedder {
    pub fn new(base_url: impl AsRef<str>, model: impl Into<String>) -> Self {
        Self::with_options(base_url, model, 32, Duration::from_secs(30))
    }

    pub fn with_options(
        base_url: impl AsRef<str>,
        model: impl Into<String>,
        batch_size: usize,
        timeout: Duration,
    ) -> Self {
        Self {
            base_url: base_url.as_ref().trim_end_matches('/').to_string(),
            model: model.into(),
            batch_size: batch_size.max(1),
            http: reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .expect("valid reqwest client"),
        }
    }

    pub fn signature(&self) -> String {
        format!("ollama-{}-{}", self.base_url, self.model)
    }

    async fn request(&self, inputs: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let response = self
            .http
            .post(format!("{}/api/embed", self.base_url))
            .json(&EmbedRequest {
                model: &self.model,
                input: inputs,
            })
            .send()
            .await
            .map_err(|e| Error::Provider(format!("ollama request failed: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Provider(format!("ollama returned {status}: {body}")));
        }
        let payload: EmbedResponse = response
            .json()
            .await
            .map_err(|e| Error::Provider(format!("cannot decode ollama response: {e}")))?;
        let mut vectors = payload.embeddings;
        for vector in &mut vectors {
            if vector.is_empty() || vector.iter().any(|v| !v.is_finite()) {
                return Err(Error::Provider("ollama returned an invalid vector".into()));
            }
            normalize(vector);
        }
        Ok(vectors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EmbeddingInput;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn signature_includes_endpoint_and_model() {
        let embedder = OllamaEmbedder::new("http://localhost:11434/", "test-model");
        assert_eq!(
            embedder.signature(),
            "ollama-http://localhost:11434-test-model"
        );
    }

    #[tokio::test]
    async fn empty_batch_does_not_call_the_service() {
        let embedder = OllamaEmbedder::new("http://127.0.0.1:9", "test-model");
        assert!(embedder.batch_embed(&[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn batch_vectors_are_normalized_and_keyed_by_input() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0_u8; 4096];
            let mut length = 0;
            loop {
                let read = socket.read(&mut buffer[length..]).await.unwrap();
                if read == 0 {
                    break;
                }
                length += read;
                if std::str::from_utf8(&buffer[..length])
                    .unwrap()
                    .contains("\r\n\r\n")
                {
                    break;
                }
            }
            let body = br#"{"embeddings":[[2.0,0.0],[0.0,0.0]]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
        });

        let embedder = OllamaEmbedder::new(format!("http://{addr}"), "test-model");
        let outputs = embedder
            .batch_embed(&[
                EmbeddingInput::new("a", "one"),
                EmbeddingInput::new("b", "two"),
            ])
            .await
            .unwrap();
        assert_eq!(outputs["a"].vector, vec![1.0, 0.0]);
        assert_eq!(outputs["b"].vector, vec![0.0, 0.0]);
        assert!(outputs.values().all(|out| out.model_signature
            == "ollama-http://127.0.0.1-test-model"
            || out.model_signature.starts_with("ollama-http://127.0.0.1:")));
    }
}

#[derive(Debug, Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[async_trait]
impl TextEmbedder for OllamaEmbedder {
    fn signature(&self) -> String {
        OllamaEmbedder::signature(self)
    }

    async fn embed(&self, input: &EmbeddingInput) -> Result<EmbeddingOutput> {
        let mut vector = self.request(vec![input.text.clone()]).await?.remove(0);
        normalize(&mut vector);
        Ok(EmbeddingOutput::new(self.signature(), vector))
    }

    async fn batch_embed(
        &self,
        inputs: &[EmbeddingInput],
    ) -> Result<BTreeMap<String, EmbeddingOutput>> {
        if inputs.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut outputs = BTreeMap::new();
        for chunk in inputs.chunks(self.batch_size) {
            let texts = chunk.iter().map(|i| i.text.clone()).collect();
            let vectors = self.request(texts).await?;
            if vectors.len() != chunk.len() {
                return Err(Error::Provider(format!(
                    "ollama returned {} vectors for {} inputs",
                    vectors.len(),
                    chunk.len()
                )));
            }
            for (input, vector) in chunk.iter().zip(vectors) {
                outputs.insert(
                    input.id.clone(),
                    EmbeddingOutput::new(self.signature(), vector),
                );
            }
        }
        Ok(outputs)
    }
}
