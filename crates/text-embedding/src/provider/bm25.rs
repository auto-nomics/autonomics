//! The offline deterministic default: BM25 weighted by frozen
//! corpus statistics and projected by signed feature hashing.
//!
//! BM25 turns the bag of word and character n-gram features into a
//! sparse weighted vector: idf downweights terms that appear in
//! every document, saturated term frequency keeps repetition from
//! dominating, and length normalization keeps bloated texts honest.
//! The sparse vector is then folded into a fixed-width dense one by
//! signed feature hashing — each feature's digest picks both its
//! slot and its sign, so unrelated features colliding in a slot
//! tend to cancel instead of fabricating similarity.
//!
//! Two shapes serve two roles:
//!
//! - [`Bm25TextEmbedder::unfitted`]: empty statistics, zero I/O,
//!   deterministic on every machine — the hub's boot default and
//!   the embedder hermetic tests run on. With no corpus every idf
//!   degenerates to the same constant, so the vector is a plain
//!   length-normalized term histogram.
//! - [`Bm25TextEmbedder::fit`]: statistics snapshotted from a text
//!   corpus and frozen for the embedder's lifetime. The snapshot
//!   fingerprint is part of the model signature: refitting after a
//!   corpus change yields a new signature, and the existing
//!   swap-and-backfill machinery treats that exactly like a model
//!   change — vectors from different snapshots never mix.

use std::collections::{BTreeMap, HashMap};

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::input::EmbeddingInput;
use crate::output::EmbeddingOutput;
use crate::provider::TextEmbedder;
use crate::provider::error::Result;
use crate::vector::normalize;

/// Default width of [`Bm25TextEmbedder`] vectors.
pub const DEFAULT_EMBEDDING_DIM: usize = 2048;

/// Default term-frequency saturation; the classic Robertson value.
pub const DEFAULT_K1: f32 = 1.2;

/// Default length-normalization strength.
pub const DEFAULT_B: f32 = 0.75;

/// Width of a character n-gram feature.
const NGRAM_LEN: usize = 3;

/// Corpus statistics one embedder instance weighs with. Frozen at
/// fit time; `df` is ordered so the snapshot fingerprint below is
/// stable across runs and machines.
#[derive(Debug, Clone, Default)]
struct Stats {
    n_docs: usize,
    /// Average feature count per document; 0 until the first fit.
    avgdl: f32,
    df: BTreeMap<String, u32>,
}

/// A BM25 embedder over frozen statistics.
#[derive(Debug, Clone)]
pub struct Bm25TextEmbedder {
    dim: usize,
    k1: f32,
    b: f32,
    stats: Stats,
    signature: String,
}

impl Bm25TextEmbedder {
    /// The deterministic boot default: no corpus, no I/O, the same
    /// vector for the same text on every machine and every run.
    #[must_use]
    pub fn unfitted() -> Self {
        Self::build(
            DEFAULT_EMBEDDING_DIM,
            DEFAULT_K1,
            DEFAULT_B,
            Stats::default(),
        )
    }

    /// The boot default at an explicit width; different widths are
    /// different vector spaces, and the signature says so.
    #[must_use]
    pub fn unfitted_with_dim(dim: usize) -> Self {
        Self::build(dim, DEFAULT_K1, DEFAULT_B, Stats::default())
    }

    /// Snapshot statistics from `texts` with the default width and
    /// parameters, and freeze them.
    #[must_use]
    pub fn fit(texts: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        Self::fit_with(DEFAULT_EMBEDDING_DIM, DEFAULT_K1, DEFAULT_B, texts)
    }

    /// Snapshot statistics from `texts` at an explicit width and
    /// parameters, and freeze them. Fitting an empty corpus yields
    /// exactly the unfitted default.
    #[must_use]
    pub fn fit_with(
        dim: usize,
        k1: f32,
        b: f32,
        texts: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Self {
        let mut stats = Stats::default();
        // f64 accumulator: the sum must not depend on float noise for
        // the snapshot fingerprint to be stable.
        let mut total_dl = 0.0_f64;
        for text in texts {
            let features = feature_counts(text.as_ref());
            let dl: usize = features.values().map(|&tf| tf as usize).sum();
            total_dl += dl as f64;
            stats.n_docs += 1;
            for feature in features.keys() {
                *stats.df.entry(feature.clone()).or_default() += 1;
            }
        }
        if stats.n_docs > 0 {
            stats.avgdl = (total_dl / stats.n_docs as f64) as f32;
        }
        Self::build(dim, k1, b, stats)
    }

    fn build(dim: usize, k1: f32, b: f32, stats: Stats) -> Self {
        let dim = dim.max(1);
        let signature = format!("bm25-d{dim}-k{k1:.2}-b{b:.2}-{}", snapshot_id(&stats));
        Self {
            dim,
            k1,
            b,
            stats,
            signature,
        }
    }

    /// The model signature stamped on every vector this embedder
    /// produces. Width, parameters, and the statistics snapshot are
    /// all part of the identity: vectors from different snapshots
    /// are not comparable.
    #[must_use]
    pub fn signature_str(&self) -> &str {
        &self.signature
    }

    /// One feature's weight: idf times saturated, length-normalized
    /// term frequency. Unseen features have df 0 and weigh the most;
    /// with an empty corpus every idf collapses to `ln 2`, the
    /// unfitted histogram behavior.
    fn weight(&self, feature: &str, tf: f32, dl: f32) -> f32 {
        let df = self.stats.df.get(feature).copied().unwrap_or(0) as f32;
        let n = self.stats.n_docs as f32;
        let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
        let avgdl = if self.stats.avgdl > 0.0 {
            self.stats.avgdl
        } else {
            1.0
        };
        let denom = tf + self.k1 * (1.0 - self.b + self.b * dl / avgdl);
        idf * tf * (self.k1 + 1.0) / denom
    }
}

#[async_trait]
impl TextEmbedder for Bm25TextEmbedder {
    fn signature(&self) -> String {
        self.signature.clone()
    }

    async fn embed(&self, input: &EmbeddingInput) -> Result<EmbeddingOutput> {
        let features = feature_counts(&input.text);
        let dl: f32 = features.values().map(|&tf| tf as f32).sum();
        let mut vector = vec![0.0_f32; self.dim];
        for (feature, tf) in &features {
            add_weighted(&mut vector, feature, self.weight(feature, *tf as f32, dl));
        }
        normalize(&mut vector);
        Ok(EmbeddingOutput::new(self.signature.clone(), vector))
    }

    // batch_embed: the default loop is exact for a stateless local
    // function — there is no batch API whose cost to amortize.
}

/// Stable fingerprint of the frozen statistics. Sorted `df` entries
/// and an f64-length accumulator keep the digest — and therefore
/// the signature — identical for the same corpus everywhere.
fn snapshot_id(stats: &Stats) -> String {
    let mut hasher = Sha256::new();
    hasher.update((stats.n_docs as u64).to_be_bytes());
    hasher.update(stats.avgdl.to_be_bytes());
    for (feature, df) in &stats.df {
        hasher.update(feature.as_bytes());
        hasher.update(b"\0");
        hasher.update(df.to_be_bytes());
    }
    let digest = hasher.finalize();
    hex(&digest[..4])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Project one weighted feature into the dense vector: the digest
/// picks both the slot and the sign. Shared features contribute
/// with the same sign on both sides of a dot product; unrelated
/// features colliding in a slot contribute with opposite signs
/// about half the time and cancel.
fn add_weighted(vector: &mut [f32], feature: &str, weight: f32) {
    let digest = Sha256::digest(feature.as_bytes());
    let slot =
        u64::from_be_bytes(digest[..8].try_into().expect("sha256 prefix")) as usize % vector.len();
    let sign = if digest[8] & 1 == 0 { 1.0 } else { -1.0 };
    vector[slot] += sign * weight;
}

/// Term frequencies over the feature set: every word token plus its
/// character n-grams. This is the exact feature scheme `fit` counts
/// document frequency over, so embed and fit always agree.
fn feature_counts(text: &str) -> HashMap<String, u32> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for token in tokens(text) {
        let ngrams: Vec<String> = char_ngrams(&token, NGRAM_LEN).collect();
        *counts.entry(token).or_default() += 1;
        for ngram in ngrams {
            *counts.entry(ngram).or_default() += 1;
        }
    }
    counts
}

fn tokens(text: &str) -> impl Iterator<Item = String> {
    const STOP_WORDS: [&str; 18] = [
        "a", "an", "and", "are", "as", "at", "be", "for", "from", "in", "is", "of", "on", "or",
        "the", "this", "to", "when",
    ];
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| token.len() > 1)
        .map(str::to_ascii_lowercase)
        .filter(move |token| !STOP_WORDS.contains(&token.as_str()))
}

fn char_ngrams(token: &str, n: usize) -> impl Iterator<Item = String> {
    let chars: Vec<char> = token.chars().collect();
    let n = n.max(1);
    (0..chars.len().saturating_sub(n.saturating_sub(1)))
        .map(move |start| chars[start..start + n].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::TextEmbedder as _;

    #[tokio::test]
    async fn unfitted_embedder_is_deterministic_and_normalized() {
        let embedder = Bm25TextEmbedder::unfitted();
        let text = "restart the daemon after config change";
        let first = embedder
            .embed(&EmbeddingInput::new("a", text))
            .await
            .unwrap();
        // Same text under a different id, fresh embedder instance —
        // the vector must not move.
        let second = Bm25TextEmbedder::unfitted()
            .embed(&EmbeddingInput::new("b", text))
            .await
            .unwrap();
        assert_eq!(first, second);
        assert!(first.model_signature.starts_with("bm25-d2048"));
        let norm = first.vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);
    }

    #[tokio::test]
    async fn different_texts_embed_differently() {
        let embedder = Bm25TextEmbedder::unfitted();
        let out = embedder
            .embed(&EmbeddingInput::new("a", "restart the daemon"))
            .await
            .unwrap();
        let other = embedder
            .embed(&EmbeddingInput::new("b", "quote mixed-case column names"))
            .await
            .unwrap();
        assert_ne!(out.vector, other.vector);
    }

    /// The batch entry must agree with the single entry — clustering
    /// mixes both (bridge passes batches, remote providers may embed
    /// one at a time through the default loop).
    #[tokio::test]
    async fn batch_embed_matches_single_embeds() {
        let embedder = Bm25TextEmbedder::unfitted();
        let inputs = [
            EmbeddingInput::new("a", "restart the daemon"),
            EmbeddingInput::new("b", "bump the timeout"),
        ];
        let batch = embedder.batch_embed(&inputs).await.unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch["a"], embedder.embed(&inputs[0]).await.unwrap());
        assert_eq!(batch["b"], embedder.embed(&inputs[1]).await.unwrap());
    }

    #[tokio::test]
    async fn unfitted_with_dim_clamps_to_at_least_one_slot() {
        let embedder = Bm25TextEmbedder::unfitted_with_dim(0);
        let out = embedder
            .embed(&EmbeddingInput::new("a", "text"))
            .await
            .unwrap();
        assert_eq!(out.vector.len(), 1);
    }

    /// The width and parameters are part of the model identity —
    /// vectors from different spaces cannot be compared, so the
    /// signature must say which.
    #[tokio::test]
    async fn signature_names_the_width_and_parameters() {
        assert!(
            Bm25TextEmbedder::unfitted_with_dim(64)
                .signature_str()
                .starts_with("bm25-d64-")
        );
        let out = Bm25TextEmbedder::unfitted_with_dim(64)
            .embed(&EmbeddingInput::new("a", "text"))
            .await
            .unwrap();
        assert!(out.model_signature.starts_with("bm25-d64-"));
    }

    /// The snapshot is part of the identity: the same corpus yields
    /// the same signature everywhere, a different corpus does not —
    /// refit means swap, and swap means backfill.
    #[test]
    fn fit_is_deterministic_and_corpus_sensitive() {
        let corpus_a = ["error one", "error two", "error three"];
        let corpus_b = ["error one", "error two", "different corpus"];
        let a1 = Bm25TextEmbedder::fit(corpus_a);
        let a2 = Bm25TextEmbedder::fit(corpus_a);
        assert_eq!(a1.signature_str(), a2.signature_str());
        assert_ne!(
            a1.signature_str(),
            Bm25TextEmbedder::fit(corpus_b).signature_str()
        );
        // An empty corpus is exactly the unfitted default.
        assert_eq!(
            Bm25TextEmbedder::fit(Vec::<String>::new()).signature_str(),
            Bm25TextEmbedder::unfitted().signature_str()
        );
    }

    /// The point of fitting: a term in every document stops pulling
    /// its hosts together, a rare term still does.
    #[tokio::test]
    async fn fitted_idf_downweights_ubiquitous_terms() {
        let mut corpus: Vec<String> = (0..20)
            .map(|i| format!("error routine failure {i}"))
            .collect();
        corpus.push("zebra unusual failure".into());
        let embedder = Bm25TextEmbedder::fit(corpus);

        let ubiquitous = crate::cosine(
            &embedder
                .embed(&EmbeddingInput::new("a", "error failure one"))
                .await
                .unwrap()
                .vector,
            &embedder
                .embed(&EmbeddingInput::new("b", "error failure two"))
                .await
                .unwrap()
                .vector,
        )
        .unwrap();
        let rare = crate::cosine(
            &embedder
                .embed(&EmbeddingInput::new("a", "zebra failure one"))
                .await
                .unwrap()
                .vector,
            &embedder
                .embed(&EmbeddingInput::new("b", "zebra failure two"))
                .await
                .unwrap()
                .vector,
        )
        .unwrap();
        // Both pairs share one distinguishing word plus "failure";
        // the rare word must bind its pair tighter than the
        // ubiquitous one does.
        assert!(
            rare > ubiquitous,
            "rare-word similarity {rare} must beat ubiquitous-word {ubiquitous}"
        );
    }

    /// Signed projection: texts sharing nothing land near zero
    /// cosine — colliding features cancel rather than fabricate
    /// similarity.
    #[tokio::test]
    async fn unrelated_texts_stay_near_zero_cosine() {
        let embedder = Bm25TextEmbedder::unfitted();
        let a = embedder
            .embed(&EmbeddingInput::new("a", "reboot the database cluster"))
            .await
            .unwrap();
        let b = embedder
            .embed(&EmbeddingInput::new("b", "quote mixed-case column names"))
            .await
            .unwrap();
        let similarity = crate::cosine(&a.vector, &b.vector).unwrap();
        assert!(
            similarity.abs() < 0.5,
            "unrelated texts must not look similar, got {similarity}"
        );
    }
}
