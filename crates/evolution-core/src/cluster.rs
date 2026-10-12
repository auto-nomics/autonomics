//! Deterministic semantic clustering of observations.
//!
//! Clustering is the observation-domain half of every evolution
//! loop: it turns the stored ledger plus its record-time vectors
//! into stable, reviewable groups — one per recurring pattern.
//! The algorithm is pure code, no model calls: vectors drive
//! similarity, structural anchors drive partitioning.
//!
//! 1. **Partition** observations by `(node_kind, kind)` — a hard
//!    boundary: different anchors or different signal kinds never
//!    merge, so free-form notes stay searchable without leaking
//!    into anchored patterns.
//! 2. **Cluster** within each partition by online (MacQueen) k-means
//!    over cosine similarity: each new observation joins its nearest
//!    centroid and pulls it by a running-mean step, or founds a new
//!    centroid when nothing is similar enough. Assignment is
//!    INCREMENTAL: persisted membership is the starting point, only
//!    observations not yet in any cluster are considered each pass.
//!    K is bounded per partition — past the cap, a dissimilar
//!    observation is force-assigned to its nearest centroid instead
//!    of growing the set.
//! 3. **Consolidate**: after each pass, same-partition centroids that
//!    drifted within `MERGE_THRESHOLD` of each other merge back into
//!    one pattern (weighted mean, membership union) — drift must not
//!    split one lesson into two surfaced clusters.
//! 4. **Threshold**: a cluster needs `MIN_CLUSTER` anchored
//!    observations before it is worth surfacing.
//!
//! Everything downstream of a [`Cluster`] — what it becomes, how
//! it is reviewed, where it is written — belongs to the consuming
//! subsystem, not to this module. Consumers hydrate member
//! records through the observation storage and derive their own
//! naming material.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use text_embedding::cosine;

use crate::ObservationStorage;
use crate::dimension::{Dimension, NodeKindDimension, ObservationKindDimension};
use crate::error::{Error, Result};
use crate::observation::Observation;

/// Anchored observations required before a cluster is surfaced.
pub const MIN_CLUSTER: usize = 3;

/// Minimum cosine similarity for an observation to join a cluster.
pub const SIMILARITY_THRESHOLD: f32 = 0.72;

/// Upper bound on clusters per partition. Past it, a dissimilar
/// observation is force-assigned to its nearest centroid rather than
/// founding another one — bounded state at the price of distortion.
/// Merging (below) keeps live partitions well under the cap; hitting
/// it means the partition genuinely carries this many patterns.
pub const MAX_CLUSTERS_PER_PARTITION: usize = 32;

/// Same-partition centroids within this cosine merge back into one
/// cluster at the end of a pass — higher than
/// [`SIMILARITY_THRESHOLD`] so a merge means "nearly the same
/// pattern", not merely "similar".
pub const MERGE_THRESHOLD: f32 = 0.90;

/// One recurring pattern: the member observation ids, their mean
/// vector, and the partition the pattern recruits from. This is the
/// unit downstream consumers (distillation) receive — they hydrate
/// the member records from storage and derive names themselves.
#[derive(Debug, Clone)]
pub struct Cluster {
    /// Stable id, fixed at creation from the founding member.
    pub id: String,
    /// Identity of the embedding model the vectors came from.
    /// Clusters are only ever compared against vectors under the
    /// same signature — switching models discards them instead of
    /// mixing incomparable vector spaces.
    pub model_signature: String,
    /// `partition_key()` — the hard boundary this cluster recruits
    /// members from; clusters in different partitions never merge.
    pub partition_key: String,
    /// Mean of the member vectors; the matching handle.
    pub vector: Vec<f32>,
    pub members: BTreeSet<String>,
}

/// The partition an observation belongs to: `(kind, node_kind)`
/// encoded into one string. `None` for unanchored observations —
/// they never participate.
pub fn partition_key(observation: &Observation) -> Option<String> {
    let node_kind = NodeKindDimension.extract(observation)?;
    let kind = ObservationKindDimension.extract(observation)?;
    Some(format!("{kind}\u{1f}{node_kind}"))
}

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

/// Outcome of one clustering pass.
pub struct ClusterReport {
    /// Clusters whose membership met [`MIN_CLUSTER`].
    pub surfaced: Vec<Cluster>,
    /// Observations assigned during this pass.
    pub assigned: usize,
    /// Clusters absorbed into a same-partition sibling by the
    /// post-pass consolidation.
    pub merged: usize,
}

/// Incremental clustering executor: loads persisted clusters,
/// assigns only the not-yet-clustered observations, persists the
/// deltas.
pub struct ClusterProcessor {
    storage: Arc<dyn ObservationStorage>,
}

impl ClusterProcessor {
    pub fn new(storage: Arc<dyn ObservationStorage>) -> Self {
        Self { storage }
    }

    /// One incremental pass under `signature` (the current
    /// embedder's identity, from the hub). Clusters persisted under
    /// a different signature are discarded and rebuilt from the
    /// full observation set.
    pub async fn execute_cluster(&self, signature: &str) -> Result<ClusterReport> {
        let mut clusters = self.storage.load_clusters().await?;
        if clusters.iter().any(|c| c.model_signature != signature) {
            self.storage.clear_clusters().await?;
            clusters.clear();
        }

        let assigned_ids: HashSet<String> = clusters
            .iter()
            .flat_map(|c| c.members.iter().cloned())
            .collect();

        // Candidates: anchored observations with a vector under the
        // current signature that no persisted cluster holds yet.
        // `list()` is ordered by id, so assignment order — and
        // therefore cluster ids — stay stable across replays.
        let mut pending = Vec::new();
        for observation in self.storage.list().await {
            if partition_key(&observation).is_none() || assigned_ids.contains(&observation.id) {
                continue;
            }
            let Some(vector) = self
                .storage
                .load_embedding(&observation.id, signature)
                .await?
            else {
                continue;
            };
            pending.push((observation, vector));
        }

        let mut dirty: HashSet<String> = HashSet::new();
        let mut assigned = 0;
        for (observation, vector) in pending {
            assigned += 1;
            // Anchored by the eligibility filter above; if that
            // invariant ever breaks, fail the pass instead of
            // panicking — the caller treats it like any storage
            // error and retries on the next cycle.
            let Some(key) = partition_key(&observation) else {
                return Err(Error::UnanchoredCandidate(observation.id));
            };
            let similarities = clusters
                .iter()
                .filter(|c| c.partition_key == key)
                .map(|cluster| {
                    cosine(&cluster.vector, &vector)
                        .map(|similarity| (similarity, cluster.id.clone()))
                        .map_err(|e| Error::Embedding(e.to_string()))
                })
                .collect::<Result<Vec<_>>>()?;
            let nearest = similarities.into_iter().max_by(|a, b| a.0.total_cmp(&b.0));
            let live_in_partition = clusters.iter().filter(|c| c.partition_key == key).count();
            // Online k-means assignment: join the nearest centroid
            // when similar enough; otherwise found a new one — unless
            // the partition is at its cap, in which case the nearest
            // centroid takes it anyway (bounded K).
            let target = match nearest {
                Some((similarity, id)) if similarity + 1e-6 >= SIMILARITY_THRESHOLD => Some(id),
                Some((_, id)) if live_in_partition >= MAX_CLUSTERS_PER_PARTITION => Some(id),
                _ => None,
            };
            match target {
                Some(id) => {
                    let cluster = clusters
                        .iter_mut()
                        .find(|c| c.id == id)
                        .expect("similarity scanned live clusters");
                    join_cluster(cluster, &vector, observation.id.clone());
                    dirty.insert(id);
                }
                None => {
                    let mut members = BTreeSet::new();
                    members.insert(observation.id.clone());
                    let id = cluster_hash(&members);
                    clusters.push(Cluster {
                        id: id.clone(),
                        model_signature: signature.to_string(),
                        partition_key: key,
                        vector,
                        members,
                    });
                    dirty.insert(id);
                }
            }
        }

        // Post-pass consolidation: drift can leave two centroids of
        // one partition nearly parallel — one pattern split in two.
        let merge = merge_close_clusters(&mut clusters)?;
        for id in &merge.removed {
            self.storage.delete_cluster(id).await?;
        }
        dirty.extend(merge.changed.iter().cloned());
        dirty.retain(|id| clusters.iter().any(|c| &c.id == id));

        for cluster in &clusters {
            if dirty.contains(&cluster.id) {
                self.storage.update_cluster(cluster.clone()).await?;
            }
        }

        Ok(ClusterReport {
            surfaced: clusters
                .into_iter()
                .filter(|c| c.members.len() >= MIN_CLUSTER)
                .collect(),
            assigned,
            merged: merge.removed.len(),
        })
    }
}

/// The MacQueen step: pull the centroid by a running mean over its
/// members and absorb the observation.
fn join_cluster(cluster: &mut Cluster, vector: &[f32], observation_id: String) {
    let count = cluster.members.len() as f32;
    for (mean, value) in cluster.vector.iter_mut().zip(vector) {
        *mean = (*mean * count + value) / (count + 1.0);
    }
    cluster.members.insert(observation_id);
}

/// Merge same-partition cluster pairs whose centroids are within
/// [`MERGE_THRESHOLD`], until no such pair remains. The larger-
/// membership cluster survives with a membership-weighted centroid
/// and the union of both member sets; the other id is dropped
/// (downstream idempotency keys are membership hashes, so the merged
/// cluster is simply a new key).
fn merge_close_clusters(clusters: &mut Vec<Cluster>) -> Result<MergeOutcome> {
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    loop {
        let mut pair = None;
        'scan: for i in 0..clusters.len() {
            for j in (i + 1)..clusters.len() {
                if clusters[i].partition_key != clusters[j].partition_key {
                    continue;
                }
                let similarity = cosine(&clusters[i].vector, &clusters[j].vector)
                    .map_err(|e| Error::Embedding(e.to_string()))?;
                if similarity + 1e-6 >= MERGE_THRESHOLD {
                    pair = Some((i, j));
                    break 'scan;
                }
            }
        }
        let Some((i, j)) = pair else { break };
        // The larger cluster keeps its id — likelier to be the one
        // downstream already knows.
        let (survivor, dropped) = if clusters[i].members.len() >= clusters[j].members.len() {
            (i, j)
        } else {
            (j, i)
        };
        let (lo, hi) = (survivor.min(dropped), survivor.max(dropped));
        let (first, second) = clusters.split_at_mut(hi);
        let (big, small) = if survivor == lo {
            (&mut first[lo], &second[0])
        } else {
            (&mut second[0], &first[lo])
        };
        let (big_n, small_n) = (big.members.len() as f32, small.members.len() as f32);
        for (mean, value) in big.vector.iter_mut().zip(&small.vector) {
            *mean = (*mean * big_n + value * small_n) / (big_n + small_n);
        }
        big.members.extend(small.members.iter().cloned());
        // Read the survivor's id BEFORE the removal: dropping the
        // lower index shifts the survivor down when it sits above it,
        // so a post-remove `clusters[survivor]` would read the wrong
        // cluster — or past the end when the survivor was last.
        let survivor_id = clusters[survivor].id.clone();
        let outcome = clusters.remove(dropped);
        removed.push(outcome.id);
        changed.push(survivor_id);
    }
    Ok(MergeOutcome { removed, changed })
}

/// Which clusters a consolidation pass dropped and rewrote.
struct MergeOutcome {
    removed: Vec<String>,
    changed: Vec<String>,
}

#[cfg(test)]
mod tests {
    //! Every test records through a real hub so observations carry
    //! the vectors the clustering reads.
    use super::*;
    use crate::ObservationHub;
    use crate::observation::{ObservationInput, ObservationKind, ObservationSource};

    async fn clustered(hub: &ObservationHub) -> Vec<Cluster> {
        let processor = ClusterProcessor::new(Arc::clone(hub.storage()));
        processor
            .execute_cluster(&hub.embedding_signature())
            .await
            .unwrap()
            .surfaced
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
        assert_eq!(out[0].members.len(), 3);
        assert_eq!(out[0].partition_key, "failure\u{1f}file_to_dataframe");
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
        assert_eq!(out[0].members.len(), 3);
        assert_eq!(out[0].partition_key, "recipe\u{1f}file_to_dataframe");
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
        assert_eq!(out[0].partition_key, "caveat\u{1f}file_to_dataframe");
    }

    #[tokio::test]
    async fn kinds_never_coalesce_into_one_cluster() {
        // Same node kind and similar wording, but the partition key
        // differs — none of the three buckets reaches the threshold.
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

    /// The incremental pass is exactly-once: a second run assigns
    /// nothing, and cluster ids/membership are unchanged.
    #[tokio::test]
    async fn second_pass_assigns_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        for i in 0..3 {
            observe(&hub, "boom at step N", &format!("fix {i}")).await;
        }
        let first = clustered(&hub).await;
        assert_eq!(first.len(), 1);
        let processor = ClusterProcessor::new(Arc::clone(hub.storage()));
        let report = processor
            .execute_cluster(&hub.embedding_signature())
            .await
            .unwrap();
        assert_eq!(report.assigned, 0);
        assert_eq!(report.surfaced.len(), 1);
        assert_eq!(report.surfaced[0].id, first[0].id);
        assert_eq!(report.surfaced[0].members, first[0].members);
    }

    /// A model swap discards stale clusters and replays from the
    /// full observation set under the new signature.
    #[tokio::test]
    async fn signature_change_replays_from_scratch() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        for i in 0..3 {
            observe(&hub, "boom at step N", &format!("fix {i}")).await;
        }
        assert_eq!(clustered(&hub).await.len(), 1);
        // Swap to a distinct embedder (the hub backfills its
        // vectors) — the persisted hashing clusters must be
        // discarded, not mixed with the new space.
        let vectors = hub
            .storage()
            .list()
            .await
            .iter()
            .map(|o| (o.id.clone(), vec![1.0, 0.0, 0.0]))
            .collect::<std::collections::BTreeMap<_, _>>();
        hub.set_embedder(Arc::new(text_embedding::VectorMapTextEmbedder::new(
            "other-model",
            vectors,
        )))
        .await
        .unwrap();
        let out = clustered(&hub).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].model_signature, "other-model");
    }

    /// Near-parallel centroids of one partition merge into the larger
    /// cluster (weighted mean, member union); other partitions are
    /// never touched, however close their centroids are.
    #[test]
    fn merge_unifies_drifted_duplicates_within_a_partition() {
        let cluster = |id: &str, key: &str, vector: Vec<f32>, members: &[&str]| Cluster {
            id: id.into(),
            model_signature: "s".into(),
            partition_key: key.into(),
            vector,
            members: members.iter().map(|m| (*m).to_string()).collect(),
        };
        let mut clusters = vec![
            cluster("big", "p", vec![1.0, 0.0], &["a1", "a2"]),
            // cos([1,0], [0.99, 0.141]) ≈ 0.99 — one pattern drifted.
            cluster("small", "p", vec![0.99, 0.141], &["b1"]),
            // Same partition, clearly distinct: survives.
            cluster("far", "p", vec![0.0, 1.0], &["c1"]),
            // Identical to "big" but another partition: untouched.
            cluster("other", "q", vec![1.0, 0.0], &["d1"]),
        ];
        let outcome = merge_close_clusters(&mut clusters).unwrap();
        assert_eq!(outcome.removed, vec!["small".to_string()]);
        let big = clusters.iter().find(|c| c.id == "big").unwrap();
        assert_eq!(big.members.len(), 3);
        // Weighted mean of (2×[1,0] + 1×[0.99,0.141]) / 3.
        let expected = [(2.0 + 0.99) / 3.0, 0.141 / 3.0];
        for (mean, want) in big.vector.iter().zip(expected) {
            assert!((mean - want).abs() < 1e-6, "{} vs {want}", mean);
        }
        assert!(clusters.iter().any(|c| c.id == "far"));
        assert!(
            clusters
                .iter()
                .any(|c| c.id == "other" && c.members.len() == 1)
        );
    }

    /// The survivor may sit at the LATER index (the pair's second
    /// cluster has strictly more members). The removal shifts it down,
    /// so the changed id must be captured before the remove — this
    /// regression pins that ordering (it previously indexed past the
    /// end and panicked).
    #[test]
    fn merge_survivor_at_the_later_index_records_the_survivor() {
        let cluster = |id: &str, key: &str, vector: Vec<f32>, members: &[&str]| Cluster {
            id: id.into(),
            model_signature: "s".into(),
            partition_key: key.into(),
            vector,
            members: members.iter().map(|m| (*m).to_string()).collect(),
        };
        let mut clusters = vec![
            cluster("small", "p", vec![0.99, 0.141], &["b1"]),
            cluster("big", "p", vec![1.0, 0.0], &["a1", "a2", "a3"]),
            // Same partition, clearly distinct: must survive untouched.
            cluster("far", "p", vec![0.0, 1.0], &["c1"]),
        ];
        let outcome = merge_close_clusters(&mut clusters).unwrap();
        assert_eq!(outcome.removed, vec!["small".to_string()]);
        assert_eq!(outcome.changed, vec!["big".to_string()]);
        let big = clusters.iter().find(|c| c.id == "big").unwrap();
        assert_eq!(big.members.len(), 4);
        assert!(
            clusters
                .iter()
                .any(|c| c.id == "far" && c.members.len() == 1)
        );
    }

    /// Past the per-partition cap, a dissimilar observation is
    /// force-assigned to its nearest centroid instead of founding
    /// yet another cluster — state stays bounded.
    #[tokio::test]
    async fn bounded_k_forces_assignment_instead_of_growth() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        // 40 pairwise-orthogonal directions: every observation is
        // maximally dissimilar to every other one.
        let count = MAX_CLUSTERS_PER_PARTITION + 8;
        for i in 0..count {
            hub.record(ObservationInput {
                kind: ObservationKind::Failure,
                source: ObservationSource::Agent,
                summary: format!("orthogonal failure {i}"),
                body: "body".into(),
                node_kind: Some("sql".into()),
                error: Some(format!("error {i}")),
            })
            .await
            .unwrap();
        }
        let observations = hub.storage().list().await;
        let vectors = observations
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let mut basis = vec![0.0; count];
                basis[i] = 1.0;
                (o.id.clone(), basis)
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        hub.set_embedder(Arc::new(text_embedding::VectorMapTextEmbedder::new(
            "orthogonal",
            vectors,
        )))
        .await
        .unwrap();

        let processor = ClusterProcessor::new(Arc::clone(hub.storage()));
        let report = processor
            .execute_cluster(&hub.embedding_signature())
            .await
            .unwrap();
        assert_eq!(report.assigned, count);
        // Orthogonal centroids never merge; the forced assignments
        // (all similarities tie at 0) deterministically pile onto one
        // cluster, which may surface — the invariant under test is
        // the bound, not the surfacing.
        assert_eq!(report.merged, 0);
        let stored = hub.storage().load_clusters().await.unwrap();
        assert_eq!(stored.len(), MAX_CLUSTERS_PER_PARTITION);
        assert_eq!(stored.iter().map(|c| c.members.len()).sum::<usize>(), count);
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
