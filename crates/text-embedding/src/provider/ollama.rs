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

/// Explicit runtime overrides. None preserves the installed model/server defaults.
/// Limits bound this client's supported configuration, not the model's capacity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OllamaInferenceOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_ctx: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_batch: Option<u32>,
}

impl OllamaInferenceOptions {
    pub fn validate(&self) -> Result<()> {
        if self.num_ctx.is_some_and(|n| !(1..=131072).contains(&n))
            || self.num_batch.is_some_and(|n| !(1..=8192).contains(&n))
        {
            return Err(Error::Provider(
                "invalid embedding context or token batch size".into(),
            ));
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.num_ctx.is_none() && self.num_batch.is_none()
    }

    pub(crate) fn signature_suffix(&self) -> String {
        if self.is_empty() {
            String::new()
        } else {
            format!(
                ":ctx:{}:token-batch:{}",
                self.num_ctx
                    .map_or_else(|| "default".into(), |n| n.to_string()),
                self.num_batch
                    .map_or_else(|| "default".into(), |n| n.to_string())
            )
        }
    }
}

#[derive(Debug, Clone)]
pub struct OllamaEmbedder {
    base_url: String,
    model: String,
    batch_size: usize,
    http: reqwest::Client,
    inference: OllamaInferenceOptions,
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
            inference: OllamaInferenceOptions::default(),
            http: reqwest::Client::builder()
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("valid reqwest client"),
        }
    }

    pub fn signature(&self) -> String {
        format!(
            "ollama-{}-{}{}",
            self.base_url,
            self.model,
            self.inference.signature_suffix()
        )
    }

    pub fn with_inference_options(mut self, inference: OllamaInferenceOptions) -> Result<Self> {
        inference.validate()?;
        self.inference = inference;
        Ok(self)
    }

    async fn request(&self, inputs: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let expected = inputs.len();
        let mut response = self
            .http
            .post(format!("{}/api/embed", self.base_url))
            .json(&EmbedRequest {
                model: &self.model,
                input: inputs,
                truncate: false,
                options: &self.inference,
            })
            .send()
            .await
            .map_err(|_| Error::Provider("ollama request failed".into()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Provider(format!("ollama returned {status}")));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Provider("cannot read ollama response".into()))?
        {
            if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
                return Err(Error::Provider("ollama response exceeds 32 MiB".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        let payload: EmbedResponse = serde_json::from_slice(&bytes)
            .map_err(|_| Error::Provider("cannot decode ollama response".into()))?;
        let mut vectors = payload.embeddings;
        if vectors.len() != expected {
            return Err(Error::Provider(format!(
                "ollama returned {} vectors for {expected} inputs",
                vectors.len()
            )));
        }
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

    async fn reply(status: &str, body: &'static str, extra_header: &str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n{extra_header}\r\n{body}",
            body.len()
        );
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            let _ = socket.read(&mut buffer).await.unwrap();
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        endpoint
    }

    #[tokio::test]
    async fn single_empty_response_is_an_error_instead_of_a_panic() {
        let endpoint = reply("200 OK", "{\"embeddings\":[]}", "").await;
        let error = OllamaEmbedder::new(endpoint, "test")
            .embed(&EmbeddingInput::new("a", "text"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("0 vectors for 1 inputs"));
    }

    #[tokio::test]
    async fn failures_do_not_echo_server_body_or_follow_redirects() {
        for (status, body, header) in [
            ("503 Unavailable", "private-input-secret", ""),
            ("200 OK", "private-input-secret", ""),
            (
                "302 Found",
                "private-input-secret",
                "location: http://127.0.0.1:9/\r\n",
            ),
        ] {
            let endpoint = reply(status, body, header).await;
            let error = OllamaEmbedder::new(endpoint, "test")
                .embed(&EmbeddingInput::new("a", "text"))
                .await
                .unwrap_err();
            let text = error.to_string();
            assert!(!text.contains("private-input-secret"));
            if status.starts_with("302") {
                assert!(text.contains("302"));
            }
        }
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
    truncate: bool,
    #[serde(skip_serializing_if = "OllamaInferenceOptions::is_empty")]
    options: &'a OllamaInferenceOptions,
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
