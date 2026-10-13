//! The observation hub: the handle every consumer shares for
//! observation evidence.
//!
//! Observations are shared evidence — skills and plugins are both
//! downstream consumers. Neither owns the ledger; this hub does. It
//! is constructed once per process by the host assembly and passed by
//! reference (the skill manager and plugin RSI each keep their own
//! `Arc`); the state directory remains the cross-process truth, so a
//! CLI and a daemon pointed at the same directory observe the same
//! ledger without any in-process singleton.
//!
//! The hub is backed by `<state_dir>/evolution.db` (records plus
//! cached vectors) via [`TursoObservationStorage`].
//!
//! Embedding happens HERE, at the moment an observation is created:
//! [`ObservationHub::record`] renders the observation to text, embeds
//! it with the installed [`TextEmbedder`], and persists the vector
//! next to the record. Consumers never invoke a model — they read
//! vectors back through [`ObservationHub::load_embeddings`]. The
//! default embedder is the offline deterministic
//! [`Bm25TextEmbedder`]; the host swaps in a local model — or a
//! corpus-fitted BM25 — via [`ObservationHub::set_embedder`], which
//! also backfills vectors for observations recorded under a previous
//! signature so a model change never blinds downstream clustering.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use text_embedding::{Bm25TextEmbedder, EmbeddingInput, TextEmbedder};

use super::storage::{ObservationStorage, TursoObservationStorage, load_embedding_map};
use crate::error::Result;
use crate::observation::{Observation, ObservationInput};
use crate::routing::ObservationRouteStore;

/// The shared handle for observation evidence and its embeddings.
#[derive(Clone)]
pub struct ObservationHub {
    state_dir: PathBuf,
    storage: Arc<dyn ObservationStorage>,
    /// The embedder every new observation is vectorized with. Swapped
    /// by the host when a local model comes up; readers take the arc
    /// for the length of one embed call.
    embedder: Arc<std::sync::RwLock<Arc<dyn TextEmbedder>>>,
}

impl ObservationHub {
    /// Hub over `<state_dir>/evolution.db`. No I/O happens here — the
    /// database is built lazily on first use — and the embedder
    /// starts as the unfitted BM25 default until the host installs
    /// a model with [`ObservationHub::set_embedder`].
    #[must_use]
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        let state_dir = state_dir.into();
        let storage = Arc::new(TursoObservationStorage::new(state_dir.clone()));
        Self {
            state_dir,
            storage,
            embedder: default_embedder(),
        }
    }

    /// The state directory every store below is rooted at.
    pub fn state_dir(&self) -> &std::path::Path {
        &self.state_dir
    }

    /// The durable storage behind this hub.
    pub fn storage(&self) -> &Arc<dyn ObservationStorage> {
        &self.storage
    }

    /// The route ledger under this hub's state dir. Routing decisions
    /// are still file-backed; see the storage module for the database
    /// migration of observation records.
    pub fn routes(&self) -> ObservationRouteStore {
        ObservationRouteStore::open(&self.state_dir)
    }

    /// The signature of the embedder new observations are vectorized
    /// with. Vectors are only comparable within one signature;
    /// [`load_embeddings`](Self::load_embeddings) selects by it.
    pub fn embedding_signature(&self) -> String {
        self.embedder
            .read()
            .expect("embedder lock poisoned")
            .signature()
    }

    /// The observation texts a corpus-fitted fallback embedder sees —
    /// the same rendering [`ObservationHub::record`] embeds. The host
    /// fits BM25 statistics over these when no model is available.
    pub async fn embedding_texts(&self) -> Vec<String> {
        self.storage
            .list()
            .await
            .iter()
            .map(observation_text)
            .collect()
    }

    /// Replace the embedder used at record time, then backfill: every
    /// stored observation whose vector is missing under the new
    /// signature is re-embedded and persisted, so observations recorded
    /// under the previous embedder (or the offline default) rejoin the
    /// clustering pool instead of being silently stranded.
    pub async fn set_embedder(&self, embedder: Arc<dyn TextEmbedder>) -> Result<()> {
        {
            let mut slot = self.embedder.write().expect("embedder lock poisoned");
            *slot = embedder;
        }
        let signature = self.embedding_signature();
        let observations = self.storage.list().await;
        let mut to_embed = Vec::new();
        for observation in observations {
            if self
                .storage
                .load_embedding(&observation.id, &signature)
                .await?
                .is_none()
            {
                to_embed.push(observation);
            }
        }
        if to_embed.is_empty() {
            return Ok(());
        }
        tracing::info!(
            signature = %signature,
            count = to_embed.len(),
            "backfilling observation embeddings under a new embedder"
        );
        let inputs = to_embed
            .iter()
            .map(|observation| EmbeddingInput {
                id: observation.id.clone(),
                text: observation_text(observation),
            })
            .collect::<Vec<_>>();
        let outputs = self
            .embedder
            .read()
            .expect("embedder lock poisoned")
            .batch_embed(&inputs)
            .map_err(|e| crate::Error::Embedding(e.to_string()))?;
        for (id, output) in outputs {
            self.storage
                .save_embedding(&id, &signature, &output.vector)
                .await?;
        }
        Ok(())
    }

    /// Load the vectors for `ids` as persisted at record time, under
    /// the current embedder's signature. Ids recorded under a previous
    /// signature (and never backfilled) are absent from the map —
    /// callers skip them.
    pub async fn load_embeddings(&self, ids: &[String]) -> Result<HashMap<String, Vec<f32>>> {
        load_embedding_map(self.storage.as_ref(), ids, &self.embedding_signature()).await
    }

    /// Record one observation (idempotent on content) and embed it on
    /// the spot. This is the canonical entry every producer lands on —
    /// agent tools, automatic failure capture, eval capture, and
    /// plugin feedback.
    ///
    /// A persistence failure fails the record; an embedding failure
    /// does not — the evidence ledger outranks the vector cache, and a
    /// later idempotent re-record retries the embedding.
    ///
    /// Downstream notification is the caller's concern: the hub is a
    /// pure ledger and knows nothing about its consumers.
    pub async fn record(&self, input: ObservationInput) -> Result<Observation> {
        let observation = self.storage.record(input).await?;
        if let Err(error) = self.embed_observation(&observation).await {
            tracing::warn!(
                id = %observation.id,
                %error,
                "embedding failed at record time; the observation is kept without a vector"
            );
        }
        Ok(observation)
    }

    /// Vectorize one stored observation unless a vector already exists
    /// under the current signature (idempotent re-records skip the
    /// model), then persist it.
    async fn embed_observation(&self, observation: &Observation) -> Result<()> {
        let embedder = Arc::clone(&*self.embedder.read().expect("embedder lock poisoned"));
        let signature = embedder.signature();
        if self
            .storage
            .load_embedding(&observation.id, &signature)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let output = embedder
            .embed(&EmbeddingInput {
                id: observation.id.clone(),
                text: observation_text(observation),
            })
            .map_err(|e| crate::Error::Embedding(e.to_string()))?;
        self.storage
            .save_embedding(&observation.id, &signature, &output.vector)
            .await
    }
}

/// The observation text an embedder sees: the structural anchors
/// (kind, node kind) plus the free-form signal (summary, body, error).
fn observation_text(observation: &Observation) -> String {
    let mut parts = vec![
        observation.kind.label(),
        observation.node_kind.as_deref().unwrap_or(""),
        &observation.summary,
        &observation.body,
    ];
    if let Some(error) = observation.error.as_deref() {
        parts.push(error);
    }
    parts.join("\n")
}

/// The offline deterministic default every hub starts with.
fn default_embedder() -> Arc<std::sync::RwLock<Arc<dyn TextEmbedder>>> {
    Arc::new(std::sync::RwLock::new(Arc::new(
        Bm25TextEmbedder::unfitted(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{ObservationKind, ObservationSource};
    use std::collections::BTreeMap;

    fn input(summary: &str) -> ObservationInput {
        ObservationInput {
            kind: ObservationKind::Failure,
            source: ObservationSource::Agent,
            summary: summary.into(),
            body: "body".into(),
            node_kind: Some("sql".into()),
            error: None,
        }
    }

    #[tokio::test]
    async fn record_is_visible_through_the_shared_storage() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        let observation = hub.record(input("summary")).await.unwrap();
        let listed = hub.storage().list().await;
        assert!(listed.iter().any(|o| o.id == observation.id));
    }

    /// The DB-backed hub materializes evolution.db under the state
    /// dir on first use — the state directory remains the portable
    /// unit of deployment.
    #[tokio::test]
    async fn default_backend_is_the_database() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        hub.storage().list().await.iter().for_each(drop);
        assert!(tmp.path().join(crate::EVOLUTION_DB).is_file());
    }

    /// Recording a vector and loading it back through the hub keeps
    /// the creation-time embedding observable — same signature in and
    /// out, no consumer-side model.
    #[tokio::test]
    async fn record_embeds_and_load_embeddings_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        let observation = hub.record(input("restart the daemon")).await.unwrap();

        let embeddings = hub
            .load_embeddings(&[observation.id.clone()])
            .await
            .unwrap();
        let vector = embeddings.get(&observation.id).expect("vector persisted");
        assert_eq!(vector.len(), text_embedding::DEFAULT_EMBEDDING_DIM);
    }

    /// Same text, fresh hubs: the unfitted BM25 default is
    /// deterministic across instances, so clustering hashes stay
    /// stable across runs.
    #[tokio::test]
    async fn same_text_embeds_identically_across_hubs() {
        let first = ObservationHub::new(tempfile::tempdir().unwrap().path());
        let second = ObservationHub::new(tempfile::tempdir().unwrap().path());
        let a = first.record(input("identical text")).await.unwrap();
        let b = second.record(input("identical text")).await.unwrap();
        let va = first.load_embeddings(&[a.id]).await.unwrap();
        let vb = second.load_embeddings(&[b.id]).await.unwrap();
        let va = va.values().next().unwrap();
        let vb = vb.values().next().unwrap();
        assert_eq!(va, vb);
    }

    /// A model swap backfills observations recorded under the previous
    /// embedder — they must rejoin the pool under the new signature.
    #[tokio::test]
    async fn set_embedder_backfills_vectors_for_prior_observations() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        let first = hub.record(input("one")).await.unwrap();
        let second = hub.record(input("two")).await.unwrap();

        let vectors = BTreeMap::from([
            (first.id.clone(), vec![1.0, 0.0]),
            (second.id.clone(), vec![0.0, 1.0]),
        ]);
        hub.set_embedder(Arc::new(text_embedding::VectorMapTextEmbedder::new(
            "swapped-model",
            vectors,
        )))
        .await
        .unwrap();

        assert_eq!(hub.embedding_signature(), "swapped-model");
        let embeddings = hub
            .load_embeddings(&[first.id.clone(), second.id.clone()])
            .await
            .unwrap();
        assert_eq!(embeddings[&first.id], vec![1.0, 0.0]);
        assert_eq!(embeddings[&second.id], vec![0.0, 1.0]);
    }
}
