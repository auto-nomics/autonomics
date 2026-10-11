//! Deterministic semantic clustering of observations.
//!
//! Clustering is the observation-domain half of every evolution
//! loop: it turns the stored ledger plus its record-time vectors
//! into stable, reviewable candidates — one per recurring pattern.
//! The algorithm is pure code, no model calls: vectors drive
//! similarity, structural anchors drive partitioning.
//!
//! 1. **Partition** observations by `(node_kind, kind)` — a hard
//!    boundary: different anchors or different signal kinds never
//!    merge, so free-form notes stay searchable without leaking
//!    into anchored patterns.
//! 2. **Cluster** within each partition by greedy nearest-centroid
//!    assignment over cosine similarity — paraphrased summaries,
//!    errors, and bodies group without requiring exact string
//!    equality.
//! 3. **Threshold**: a cluster needs `MIN_CLUSTER` anchored
//!    observations before it is worth surfacing.
//! 4. **Name** each surviving cluster by its semantic center: the
//!    member closest to the centroid lends it a stable, human-
//!    readable signature.
//!
//! Everything downstream of a [`Candidate`] — what it becomes, how
//! it is reviewed, where it is written — belongs to the consuming
//! subsystem, not to this module.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use text_embedding::{cosine, mean_vector};

use crate::error::{Error, Result};
use crate::observation::{Observation, ObservationKind};

/// Anchored observations required before a cluster is surfaced.
pub const MIN_CLUSTER: usize = 3;

/// Minimum cosine similarity for an observation to join a cluster.
pub const SIMILARITY_THRESHOLD: f32 = 0.72;

/// Stable idempotency key over a set of observation ids: the same
/// members always hash to the same key, so consumers can mark a
/// cluster processed exactly once.
pub fn cluster_hash(observation_ids: &BTreeSet<String>) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    for id in observation_ids {
        hasher.update(id.as_bytes());
        hasher.update(b"\0");
    }
    let digest = hex(&hasher.finalize());
    format!("c-{}", &digest[..16])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One recurring pattern: a semantic cluster of observations that
/// survived the threshold, with a stable signature derived from its
/// semantic center.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub kind: ObservationKind,
    pub node_kind: String,
    /// A human-readable summary of the semantic cluster. Failures
    /// prefer normalized error text; other kinds derive keywords from
    /// the cluster's representative summary.
    pub signature: String,
    pub observations: Vec<Observation>,
}

impl Candidate {
    /// The stable idempotency key over the member observation ids.
    pub fn hash(&self) -> String {
        let ids: BTreeSet<String> = self.observations.iter().map(|o| o.id.clone()).collect();
        cluster_hash(&ids)
    }
}

/// Group anchored observations into candidate clusters meeting the
/// threshold, in a deterministic order. The vectors the hub
/// persisted at record time drive similarity, while `(node_kind,
/// kind)` remains a hard partition: observations for different node
/// kinds or observation kinds never merge. Observations without a
/// vector under the current embedder signature are skipped, not
/// failed — they sit out this pass and rejoin once backfilled.
pub fn candidates(
    observations: &[Observation],
    embeddings: &HashMap<String, Vec<f32>>,
) -> Result<Vec<Candidate>> {
    let mut eligible: Vec<Observation> = observations
        .iter()
        .filter(|o| o.node_kind.is_some() && embeddings.contains_key(&o.id))
        .cloned()
        .collect();
    eligible.sort_by(|a, b| a.id.cmp(&b.id));

    let mut partitions: BTreeMap<(String, ObservationKind), Vec<EmbeddingCluster>> =
        BTreeMap::new();
    for observation in eligible {
        let vector = &embeddings[&observation.id];
        let key = (
            observation
                .node_kind
                .clone()
                .expect("eligibility checked node_kind"),
            observation.kind,
        );
        let clusters = partitions.entry(key).or_default();
        let similarities = clusters
            .iter_mut()
            .map(|cluster| {
                cosine(&cluster.centroid, vector)
                    .map(|similarity| (similarity, cluster))
                    .map_err(|e| Error::Embedding(e.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;
        let target = similarities
            .into_iter()
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .filter(|target| target.0 + 1e-6 >= SIMILARITY_THRESHOLD);
        match target {
            Some((_, cluster)) => {
                let count = cluster.members.len() as f32;
                for (centroid, value) in cluster.centroid.iter_mut().zip(vector) {
                    *centroid = (*centroid * count + value) / (count + 1.0);
                }
                cluster.members.push(observation.clone());
            }
            None => clusters.push(EmbeddingCluster {
                members: vec![observation.clone()],
                centroid: vector.clone(),
            }),
        }
    }

    let mut out = Vec::new();
    for ((node_kind, kind), clusters) in partitions {
        for cluster in clusters {
            if cluster.members.len() < MIN_CLUSTER {
                continue;
            }
            let signature = representative_signature(&cluster.members, embeddings)?;
            out.push(Candidate {
                kind,
                node_kind: node_kind.clone(),
                signature,
                observations: cluster.members,
            });
        }
    }
    out.sort_by(|a, b| {
        a.node_kind
            .cmp(&b.node_kind)
            .then(a.kind.cmp(&b.kind))
            .then(a.signature.cmp(&b.signature))
    });
    Ok(out)
}

#[derive(Debug)]
struct EmbeddingCluster {
    members: Vec<Observation>,
    centroid: Vec<f32>,
}

/// Choose the observation closest to the cluster's semantic center,
/// then derive a human-readable signature from it. This gives vector
/// clusters stable, reviewable names without depending on processing
/// order.
fn representative_signature(
    members: &[Observation],
    embeddings: &HashMap<String, Vec<f32>>,
) -> Result<String> {
    let vectors = members
        .iter()
        .map(|member| {
            embeddings
                .get(&member.id)
                .map(|vector| vector.as_slice())
                .expect("cluster members passed the vector filter")
        })
        .collect::<Vec<_>>();
    let mean = mean_vector(&vectors).map_err(|e| Error::Embedding(e.to_string()))?;
    let similarities = vectors
        .iter()
        .enumerate()
        .map(|(index, vector)| {
            cosine(&mean, vector)
                .map(|similarity| (similarity, index))
                .map_err(|e| Error::Embedding(e.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    let representative = similarities
        .into_iter()
        .max_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, index)| &members[index])
        .expect("representative called with a cluster");

    if representative.kind == ObservationKind::Failure
        && let Some(error) = representative.error.as_deref()
    {
        let normalized = signature(error);
        if !normalized.is_empty() {
            return Ok(normalized);
        }
    }
    Ok(keyword_signature(&representative.summary))
}

/// A short, readable signature for semantic clusters. Unlike error
/// signatures, it does not preserve the `N` placeholder: names such as
/// "generic recipe 3" should cluster to "generic recipe".
fn keyword_signature(text: &str) -> String {
    const STOP_WORDS: [&str; 14] = [
        "a", "an", "and", "are", "at", "for", "in", "is", "of", "on", "or", "the", "this", "to",
    ];
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| word.len() > 1 && !word.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_ascii_lowercase)
        .filter(|word| !STOP_WORDS.contains(&word.as_str()))
        .take(6)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normalize an error text into a stable signature: first line,
/// whitespace-collapsed, numbers generalized so "column 3" and
/// "column 7" cluster together.
fn signature(error: &str) -> String {
    let first_line = error.lines().next().unwrap_or("");
    let collapsed: String = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(collapsed.len());
    let mut in_number = false;
    for ch in collapsed.chars() {
        if ch.is_ascii_digit() {
            if !in_number {
                out.push('N');
                in_number = true;
            }
        } else {
            in_number = false;
            out.push(ch);
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    //! Every test records through a real hub so observations carry
    //! the vectors the clustering reads.
    use super::*;
    use crate::ObservationHub;
    use crate::observation::{ObservationInput, ObservationKind, ObservationSource};
    use crate::storage::ObservationStorage as _;

    async fn clustered(hub: &ObservationHub) -> Vec<Candidate> {
        let observations = hub.storage().list().await;
        let ids = observations
            .iter()
            .map(|o| o.id.clone())
            .collect::<Vec<_>>();
        let embeddings = hub.load_embeddings(&ids).await.unwrap();
        candidates(&observations, &embeddings).unwrap()
    }

    async fn observe(hub: &ObservationHub, error: &str, body: &str) {
        hub.record(ObservationInput {
            kind: ObservationKind::Failure,
            source: ObservationSource::Agent,
            summary: format!("fix for {error}"),
            body: body.into(),
            node_kind: Some("file_to_dataframe".into()),
            error: Some(error.into()),
        })
        .await
        .unwrap();
    }

    async fn observe_kind(hub: &ObservationHub, kind: ObservationKind, summary: &str, body: &str) {
        hub.record(ObservationInput {
            kind,
            source: ObservationSource::Agent,
            summary: summary.into(),
            body: body.into(),
            node_kind: Some("file_to_dataframe".into()),
            error: None,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn clusters_by_anchor_with_numeric_generalization() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        observe(&hub, "column 3 has bad type", "cast to f64").await;
        observe(&hub, "column 7 has bad type", "cast to f64").await;
        observe(&hub, "column 9 has bad type", "cast to f64 or i64").await;
        observe(&hub, "unrelated error text", "other fix").await;

        let out = clustered(&hub).await;
        assert_eq!(out.len(), 1);
        let candidate = &out[0];
        assert_eq!(candidate.node_kind, "file_to_dataframe");
        assert_eq!(candidate.observations.len(), 3);
        assert_eq!(candidate.signature, "column N has bad type");
    }

    #[tokio::test]
    async fn below_threshold_never_surfaces() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        observe(&hub, "e1", "f1").await;
        observe(&hub, "e1", "f2").await;
        assert!(clustered(&hub).await.is_empty());
    }

    #[tokio::test]
    async fn unanchored_observations_do_not_participate() {
        // No node kind, no cluster — regardless of kind.
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        for i in 0..5 {
            hub.record(ObservationInput {
                kind: ObservationKind::Recipe,
                source: ObservationSource::Agent,
                summary: format!("note {i}"),
                body: "no anchor".into(),
                node_kind: None,
                error: None,
            })
            .await
            .unwrap();
        }
        assert!(clustered(&hub).await.is_empty());
    }

    /// Paraphrases cover one lesson without relying on exact text
    /// equality — vector similarity does the grouping.
    #[tokio::test]
    async fn recipe_clusters_per_node_kind_with_distinct_wordings() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        observe_kind(
            &hub,
            ObservationKind::Recipe,
            "quote mixed-case column names in queries",
            "recipe body a",
        )
        .await;
        observe_kind(
            &hub,
            ObservationKind::Recipe,
            "identifiers are case-sensitive; wrap them in double quotes",
            "recipe body b",
        )
        .await;
        observe_kind(
            &hub,
            ObservationKind::Recipe,
            "use quoted identifiers when column names are case-sensitive",
            "recipe body c",
        )
        .await;

        let out = clustered(&hub).await;
        assert_eq!(out.len(), 1);
        let candidate = &out[0];
        assert_eq!(candidate.kind, ObservationKind::Recipe);
        assert_eq!(candidate.observations.len(), 3);
        assert_eq!(
            candidate.signature,
            "use quoted identifiers when column names"
        );
    }

    #[tokio::test]
    async fn caveat_clusters_per_node_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        observe_kind(
            &hub,
            ObservationKind::Caveat,
            "locale changes text column sort order",
            "caveat body a",
        )
        .await;
        observe_kind(
            &hub,
            ObservationKind::Caveat,
            "text column sorting changes with locale order",
            "caveat body b",
        )
        .await;
        observe_kind(
            &hub,
            ObservationKind::Caveat,
            "sort order for text columns depends on locale",
            "caveat body c",
        )
        .await;

        let out = clustered(&hub).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, ObservationKind::Caveat);
    }

    #[tokio::test]
    async fn kinds_never_coalesce_into_one_cluster() {
        // Same node kind and the same normalized signature text, but
        // the kind is part of the key — the per-node recipe and
        // caveat buckets stay separate from each other and from the
        // failure bucket, and none reaches three.
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        observe(&hub, "column 3 has bad type", "failure fix").await;
        observe(&hub, "column 7 has bad type", "another failure fix").await;
        observe_kind(
            &hub,
            ObservationKind::Recipe,
            "column 3 has bad type",
            "recipe a",
        )
        .await;
        observe_kind(
            &hub,
            ObservationKind::Recipe,
            "column 7 has bad type",
            "recipe b",
        )
        .await;
        observe_kind(
            &hub,
            ObservationKind::Caveat,
            "column 3 has bad type",
            "caveat a",
        )
        .await;
        observe_kind(
            &hub,
            ObservationKind::Caveat,
            "column 7 has bad type",
            "caveat b",
        )
        .await;
        assert!(clustered(&hub).await.is_empty());
    }

    /// The idempotency key is order-insensitive over members.
    #[test]
    fn cluster_hash_is_order_insensitive() {
        let ids: BTreeSet<String> = ["b", "a", "c"].iter().map(|s| s.to_string()).collect();
        let first = cluster_hash(&ids);
        let reversed: BTreeSet<String> = ["c", "b", "a"].iter().map(|s| s.to_string()).collect();
        assert_eq!(first, cluster_hash(&reversed));
    }
}
