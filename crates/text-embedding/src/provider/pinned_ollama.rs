//! Digest-bound Ollama provider. Pin an already installed model without pulling
//! or overwriting the user's model tag. Cache users must also version their
//! text preprocessing and query instructions alongside this signature.
use std::{
    collections::{BTreeMap, HashSet},
    time::Duration,
};

use async_trait::async_trait;
use serde::Deserialize;

use super::{
    OllamaEmbedder, OllamaInferenceOptions, TextEmbedder,
    error::{Error, Result},
};
use crate::{EmbeddingInput, EmbeddingOutput};

#[derive(Clone)]
pub struct PinnedOllamaEmbedder {
    base_url: String,
    alias: String,
    digest: String,
    dimensions: usize,
    batch_size: usize,
    http: reqwest::Client,
    inner: OllamaEmbedder,
    inference: OllamaInferenceOptions,
}

impl std::fmt::Debug for PinnedOllamaEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedOllamaEmbedder")
            .field("digest", &self.digest)
            .field("dimensions", &self.dimensions)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct Tags {
    models: Vec<Model>,
}
#[derive(Deserialize)]
struct Model {
    name: String,
    digest: String,
}

impl PinnedOllamaEmbedder {
    /// An expected digest is required: mutable model tags are not cache keys.
    /// Creates a separate digest-named alias if absent and validates its digest
    /// before and after every embedding batch. A mismatch fails the whole call.
    pub async fn connect(
        base_url: impl AsRef<str>,
        source_model: &str,
        expected_digest: &str,
        dimensions: usize,
        batch_size: usize,
        timeout: Duration,
    ) -> Result<Self> {
        Self::connect_with_inference(
            base_url,
            source_model,
            expected_digest,
            dimensions,
            batch_size,
            timeout,
            OllamaInferenceOptions::default(),
        )
        .await
    }

    pub async fn connect_with_inference(
        base_url: impl AsRef<str>,
        source_model: &str,
        expected_digest: &str,
        dimensions: usize,
        batch_size: usize,
        timeout: Duration,
        inference: OllamaInferenceOptions,
    ) -> Result<Self> {
        inference.validate()?;
        if expected_digest.len() != 64
            || !expected_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || dimensions == 0
            || dimensions > 65536
            || batch_size == 0
            || batch_size > 64
        {
            return Err(Error::Provider(
                "invalid model digest, dimensions, or batch size".into(),
            ));
        }
        let mut url = reqwest::Url::parse(base_url.as_ref())
            .map_err(|_| Error::Provider("invalid Ollama endpoint".into()))?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err(Error::Provider(
                "Ollama endpoint must be an HTTP origin without credentials".into(),
            ));
        }
        url.set_path("");
        let base_url = url.as_str().trim_end_matches('/').to_owned();
        let alias = format!("autonomics-embedding:{expected_digest}");
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::Provider("cannot construct Ollama client".into()))?;
        let provider = Self {
            inner: OllamaEmbedder::with_options(&base_url, &alias, batch_size, timeout)
                .with_inference_options(inference.clone())?,
            inference,
            base_url,
            alias,
            digest: expected_digest.into(),
            dimensions,
            batch_size,
            http,
        };
        let tags = provider.tags().await?;
        if provider.alias_matches(&tags)? {
            return Ok(provider);
        }
        let source = if source_model.contains(':') {
            source_model.to_owned()
        } else {
            format!("{source_model}:latest")
        };
        let matches = tags
            .models
            .iter()
            .filter(|model| model.name == source)
            .collect::<Vec<_>>();
        if matches.len() != 1 || matches[0].digest != expected_digest {
            return Err(Error::Provider(
                "installed source model does not match expected digest".into(),
            ));
        }
        let response = provider
            .http
            .post(format!("{}/api/copy", provider.base_url))
            .json(&serde_json::json!({"source":source,"destination":provider.alias}))
            .send()
            .await
            .map_err(|_| Error::Provider("Ollama model pin request failed".into()))?;
        if !response.status().is_success() {
            return Err(Error::Provider(format!(
                "Ollama model pin returned {}",
                response.status()
            )));
        }
        provider.verify().await?;
        Ok(provider)
    }

    async fn tags(&self) -> Result<Tags> {
        let mut response = self
            .http
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
            .map_err(|_| Error::Provider("Ollama model verification request failed".into()))?;
        if !response.status().is_success() {
            return Err(Error::Provider(format!(
                "Ollama model verification returned {}",
                response.status()
            )));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Provider("cannot read Ollama model registry".into()))?
        {
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err(Error::Provider(
                    "Ollama model registry exceeds 4 MiB".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| Error::Provider("invalid Ollama model registry".into()))
    }

    fn alias_matches(&self, tags: &Tags) -> Result<bool> {
        let matches = tags
            .models
            .iter()
            .filter(|model| model.name == self.alias)
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return Ok(false);
        }
        if matches.len() == 1 && matches[0].digest == self.digest {
            return Ok(true);
        }
        Err(Error::Provider("pinned Ollama model digest changed".into()))
    }

    async fn verify(&self) -> Result<()> {
        if !self.alias_matches(&self.tags().await?)? {
            return Err(Error::Provider(
                "pinned Ollama model is no longer installed".into(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl TextEmbedder for PinnedOllamaEmbedder {
    fn signature(&self) -> String {
        format!(
            "ollama-pinned-v1:sha256:{}:dim:{}:truncate-false:unit-l2{}",
            self.digest,
            self.dimensions,
            self.inference.signature_suffix()
        )
    }

    async fn embed(&self, input: &EmbeddingInput) -> Result<EmbeddingOutput> {
        self.batch_embed(std::slice::from_ref(input))
            .await?
            .remove(&input.id)
            .ok_or_else(|| Error::Provider("missing pinned embedding".into()))
    }

    async fn batch_embed(
        &self,
        inputs: &[EmbeddingInput],
    ) -> Result<BTreeMap<String, EmbeddingOutput>> {
        let mut ids = HashSet::new();
        if inputs.iter().any(|input| !ids.insert(&input.id)) {
            return Err(Error::Provider("duplicate embedding input IDs".into()));
        }
        let mut outputs = BTreeMap::new();
        for batch in inputs.chunks(self.batch_size) {
            self.verify().await?;
            let vectors = self.inner.batch_embed(batch).await?;
            self.verify().await?;
            for (id, output) in vectors {
                if output.vector.len() != self.dimensions {
                    return Err(Error::DimensionMismatch {
                        expected: self.dimensions,
                        actual: output.vector.len(),
                    });
                }
                let norm = output
                    .vector
                    .iter()
                    .map(|v| f64::from(*v).powi(2))
                    .sum::<f64>();
                if !norm.is_finite() || norm < 1e-12 {
                    return Err(Error::Provider(
                        "pinned Ollama returned a zero or invalid vector".into(),
                    ));
                }
                outputs.insert(id, EmbeddingOutput::new(self.signature(), output.vector));
            }
        }
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    fn tags(digest: &str) -> String {
        serde_json::json!({"models":[{"name":format!("autonomics-embedding:{DIGEST}"),"digest":digest}]}).to_string()
    }

    async fn server(
        responses: Vec<(&'static str, String)>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        server_with_options(responses, None).await
    }

    async fn server_with_options(
        responses: Vec<(&'static str, String)>,
        options: Option<serde_json::Value>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            for (path, body) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                let header_end;
                loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(at) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        header_end = at + 4;
                        break;
                    }
                }
                let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
                assert!(headers.lines().next().unwrap().contains(path));
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|s| s.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while bytes.len() < header_end + length {
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                if path == "/api/embed" {
                    let input: serde_json::Value =
                        serde_json::from_slice(&bytes[header_end..]).unwrap();
                    assert_eq!(input["truncate"], false);
                    assert_eq!(input["model"], format!("autonomics-embedding:{DIGEST}"));
                    assert_eq!(input.get("options"), options.as_ref());
                }
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(body.as_bytes()).await.unwrap();
            }
        });
        (endpoint, task)
    }

    async fn connect(endpoint: &str) -> Result<PinnedOllamaEmbedder> {
        PinnedOllamaEmbedder::connect(endpoint, "source", DIGEST, 2, 1, Duration::from_secs(2))
            .await
    }

    #[tokio::test]
    async fn inference_options_are_forwarded_and_isolate_signatures() {
        let (endpoint, task) = server_with_options(
            vec![
                ("/api/tags", tags(DIGEST)),
                ("/api/tags", tags(DIGEST)),
                ("/api/tags", tags(DIGEST)),
                ("/api/tags", tags(DIGEST)),
                (
                    "/api/embed",
                    serde_json::json!({"embeddings":[[3.0,4.0]]}).to_string(),
                ),
                ("/api/tags", tags(DIGEST)),
            ],
            Some(serde_json::json!({"num_ctx":8192,"num_batch":8192})),
        )
        .await;
        let default = connect(&endpoint).await.unwrap();
        assert_eq!(
            default.signature(),
            format!("ollama-pinned-v1:sha256:{DIGEST}:dim:2:truncate-false:unit-l2")
        );
        let inference = OllamaInferenceOptions {
            num_ctx: Some(8192),
            num_batch: Some(8192),
        };
        let explicit = PinnedOllamaEmbedder::connect_with_inference(
            &endpoint,
            "source",
            DIGEST,
            2,
            1,
            Duration::from_secs(2),
            inference.clone(),
        )
        .await
        .unwrap();
        let changed = PinnedOllamaEmbedder::connect_with_inference(
            &endpoint,
            "source",
            DIGEST,
            2,
            1,
            Duration::from_secs(2),
            OllamaInferenceOptions {
                num_ctx: Some(4096),
                ..inference
            },
        )
        .await
        .unwrap();
        assert_ne!(default.signature(), explicit.signature());
        assert_ne!(changed.signature(), explicit.signature());
        let output = explicit
            .embed(&EmbeddingInput::new("a", "paper text"))
            .await
            .unwrap();
        assert_eq!(output.model_signature, explicit.signature());
        task.await.unwrap();
    }

    #[tokio::test]
    async fn invalid_inference_options_fail_before_connecting() {
        let result = PinnedOllamaEmbedder::connect_with_inference(
            "http://127.0.0.1:9",
            "source",
            DIGEST,
            2,
            1,
            Duration::from_secs(2),
            OllamaInferenceOptions {
                num_ctx: Some(0),
                num_batch: None,
            },
        )
        .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("context or token batch")
        );
    }

    #[tokio::test]
    async fn correct_source_is_copied_to_separate_alias_and_verified() {
        let (endpoint, task) = server(vec![
            (
                "/api/tags",
                serde_json::json!({"models":[{"name":"source:latest","digest":DIGEST}]})
                    .to_string(),
            ),
            ("/api/copy", "{}".into()),
            ("/api/tags", tags(DIGEST)),
        ])
        .await;
        let provider = connect(&endpoint).await.unwrap();
        assert!(provider.signature().contains(DIGEST));
        assert!(!provider.signature().contains(&endpoint));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn existing_wrong_alias_is_never_overwritten() {
        let (endpoint, task) = server(vec![("/api/tags", tags(&"b".repeat(64)))]).await;
        assert!(
            connect(&endpoint)
                .await
                .unwrap_err()
                .to_string()
                .contains("digest changed")
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn source_tag_must_match_expected_revision() {
        let (endpoint, task) = server(vec![("/api/tags", "{\"models\":[]}".into())]).await;
        assert!(
            connect(&endpoint)
                .await
                .unwrap_err()
                .to_string()
                .contains("source model")
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_digest_change_after_embedding_discards_results() {
        let (endpoint, task) = server(vec![
            ("/api/tags", tags(DIGEST)),
            ("/api/tags", tags(DIGEST)),
            ("/api/embed", "{\"embeddings\":[[1,0]]}".into()),
            ("/api/tags", tags(&"b".repeat(64))),
        ])
        .await;
        let provider = connect(&endpoint).await.unwrap();
        assert!(
            provider
                .embed(&EmbeddingInput::new("id", "private text"))
                .await
                .unwrap_err()
                .to_string()
                .contains("digest changed")
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn missing_extra_dimension_and_zero_vectors_fail_without_panics() {
        for (response, verify_after) in [
            ("{\"embeddings\":[]}", false),
            ("{\"embeddings\":[[1,0],[0,1]]}", false),
            ("{\"embeddings\":[[1,0,0]]}", true),
            ("{\"embeddings\":[[0,0]]}", true),
        ] {
            let mut steps = vec![
                ("/api/tags", tags(DIGEST)),
                ("/api/tags", tags(DIGEST)),
                ("/api/embed", response.into()),
            ];
            if verify_after {
                steps.push(("/api/tags", tags(DIGEST)));
            }
            let (endpoint, task) = server(steps).await;
            assert!(
                connect(&endpoint)
                    .await
                    .unwrap()
                    .embed(&EmbeddingInput::new("id", "text"))
                    .await
                    .is_err()
            );
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn batches_reverify_and_return_digest_bound_vectors() {
        let (endpoint, task) = server(vec![
            ("/api/tags", tags(DIGEST)),
            ("/api/tags", tags(DIGEST)),
            ("/api/embed", "{\"embeddings\":[[2,0]]}".into()),
            ("/api/tags", tags(DIGEST)),
            ("/api/tags", tags(DIGEST)),
            ("/api/embed", "{\"embeddings\":[[0,3]]}".into()),
            ("/api/tags", tags(DIGEST)),
        ])
        .await;
        let provider = connect(&endpoint).await.unwrap();
        let outputs = provider
            .batch_embed(&[EmbeddingInput::new("a", "A"), EmbeddingInput::new("b", "B")])
            .await
            .unwrap();
        assert_eq!(outputs["a"].vector, vec![1., 0.]);
        assert_eq!(outputs["b"].vector, vec![0., 1.]);
        assert!(
            outputs
                .values()
                .all(|row| row.model_signature == provider.signature())
        );
        assert!(provider.batch_embed(&[]).await.unwrap().is_empty());
        assert!(
            provider
                .batch_embed(&[EmbeddingInput::new("a", "A"), EmbeddingInput::new("a", "B")])
                .await
                .is_err()
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn invalid_revisions_and_credentialed_endpoints_fail_before_network() {
        for digest in ["", "tag:latest", &"A".repeat(64)] {
            assert!(
                PinnedOllamaEmbedder::connect(
                    "http://127.0.0.1:9",
                    "source",
                    digest,
                    2,
                    1,
                    Duration::from_secs(1)
                )
                .await
                .is_err()
            );
        }
        let error = connect("https://secret-token@localhost:9")
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("secret-token"));
    }
}
