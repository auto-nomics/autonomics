use super::*;
use crate::{SearchEngineMode, bib_base::BibCore};
use async_trait::async_trait;
use bib_types::{AnnotationKind, Article};
use std::sync::atomic::{AtomicUsize, Ordering};
use text_embedding::{EmbeddingOutput, provider::error::Result as EmbedResult};

struct Directory(std::path::PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("bib-semantic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn sidecar(&self) -> String {
        self.0.join("vectors.db").to_string_lossy().into_owned()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Probe {
    calls: AtomicUsize,
    fail_at: AtomicUsize,
    pause_at: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    signature: &'static str,
}
impl Probe {
    fn new(signature: &'static str) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            fail_at: AtomicUsize::new(0),
            pause_at: AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            signature,
        })
    }
}
#[async_trait]
impl TextEmbedder for Probe {
    fn signature(&self) -> String {
        self.signature.into()
    }
    async fn embed(&self, input: &EmbeddingInput) -> EmbedResult<EmbeddingOutput> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_at.load(Ordering::SeqCst) == call {
            return Err("injected embedding outage".into());
        }
        if self.pause_at.load(Ordering::SeqCst) == call {
            self.entered.notify_one();
            self.release.notified().await;
        }
        // Structural tests only. The actual installed-model qualification is
        // separate; this intentionally deterministic probe makes no IR claim.
        let vector = if input.text.contains("immune") {
            vec![2., 0.]
        } else {
            vec![0., 3.]
        };
        Ok(EmbeddingOutput::new(self.signature(), vector))
    }
}
fn profile() -> SemanticProfile {
    SemanticProfile {
        dimensions: 2,
        ..Default::default()
    }
}
async fn core() -> Arc<BibCore> {
    Arc::new(
        BibCore::open_with_search_engine(":memory:", SearchEngineMode::Tantivy)
            .await
            .unwrap(),
    )
}
async fn index(core: &BibCore, directory: &Directory, probe: Arc<Probe>) -> SemanticIndex {
    SemanticIndex::open(core.conn(), &directory.sidecar(), profile(), probe)
        .await
        .unwrap()
}
fn paper(id: &str, text: &str) -> Article {
    let mut article = Article::new(id, format!("Paper {id}"));
    article.abstract_text = Some(text.into());
    article
}

#[tokio::test]
async fn resource_caps_reject_before_embedding_and_recover_without_new_signature() {
    let directory = Directory::new();
    let core = core().await;
    core.upsert_articles(&[paper("a", "immune first"), paper("b", "immune second")])
        .await
        .unwrap();
    let probe = Probe::new("resource-caps");
    let mut store = index(&core, &directory, probe.clone())
        .await
        .with_limits(SemanticLimits {
            max_chunks: 1,
            ..Default::default()
        })
        .unwrap();
    let signature = store.signature.clone();
    assert!(
        store
            .sync()
            .await
            .unwrap_err()
            .to_string()
            .contains("index chunks")
    );
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
    assert!(store.head().await.unwrap().is_none());
    store = store
        .with_limits(SemanticLimits {
            max_vector_bytes: 8,
            ..Default::default()
        })
        .unwrap();
    assert!(
        store
            .sync()
            .await
            .unwrap_err()
            .to_string()
            .contains("vector payload bytes")
    );
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
    store = store
        .with_limits(SemanticLimits {
            max_input_bytes: 5,
            ..Default::default()
        })
        .unwrap();
    assert!(
        store
            .sync()
            .await
            .unwrap_err()
            .to_string()
            .contains("model input bytes")
    );
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
    store = store
        .with_limits(SemanticLimits {
            max_vector_bytes: 16,
            ..Default::default()
        })
        .unwrap();
    let (snapshot, _) = store.sync().await.unwrap();
    assert_eq!(snapshot.chunks.len(), 2);
    assert_eq!(store.signature, signature);
    assert_eq!(store.resource_usage().await.unwrap().vector_bytes, 16);
    core.upsert_article(&paper("c", "immune third"))
        .await
        .unwrap();
    assert!(
        store
            .sync()
            .await
            .unwrap_err()
            .to_string()
            .contains("vector payload bytes")
    );
    assert_eq!(
        store.head().await.unwrap().unwrap().generation,
        snapshot.generation
    );
    store = store
        .with_limits(SemanticLimits {
            max_vector_bytes: 24,
            ..Default::default()
        })
        .unwrap();
    let (finished, report) = store.sync().await.unwrap();
    assert_eq!(finished.chunks.len(), 3);
    assert_eq!(report.embedded_chunks, 1);
    assert_eq!(store.resource_usage().await.unwrap().vector_bytes, 24);
}

#[tokio::test]
async fn model_input_budget_counts_utf8_title_newline_and_body_bytes() {
    let directory = Directory::new();
    let core = core().await;
    let mut article = Article::new("unicode", "研究甲");
    article.abstract_text = Some("肝细胞".into());
    core.upsert_article(&article).await.unwrap();
    let probe = Probe::new("input-bytes");
    let mut store = index(&core, &directory, probe.clone())
        .await
        .with_limits(SemanticLimits {
            max_input_bytes: 18,
            ..Default::default()
        })
        .unwrap();
    assert!(
        store
            .sync()
            .await
            .unwrap_err()
            .to_string()
            .contains("model input bytes requires 19")
    );
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
    store = store
        .with_limits(SemanticLimits {
            max_input_bytes: 19,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(store.sync().await.unwrap().0.chunks.len(), 1);
}

#[tokio::test]
async fn writer_transaction_prevents_concurrent_profiles_from_overrunning_payload_budget() {
    let directory = Directory::new();
    let core = core().await;
    core.upsert_article(&paper("a", "immune shared canonical source"))
        .await
        .unwrap();
    let a = Probe::new("quota-a");
    let b = Probe::new("quota-b");
    a.pause_at.store(1, Ordering::SeqCst);
    b.pause_at.store(1, Ordering::SeqCst);
    let limits = SemanticLimits {
        max_vector_bytes: 8,
        ..Default::default()
    };
    let mut one = index(&core, &directory, a.clone())
        .await
        .with_limits(limits.clone())
        .unwrap();
    let mut two = index(&core, &directory, b.clone())
        .await
        .with_limits(limits)
        .unwrap();
    let first = tokio::spawn(async move {
        let result = one.sync().await;
        (one, result)
    });
    a.entered.notified().await;
    let second = tokio::spawn(async move {
        let result = two.sync().await;
        (two, result)
    });
    b.entered.notified().await;
    a.release.notify_one();
    let (one, result) = first.await.unwrap();
    result.unwrap();
    b.release.notify_one();
    let (two, result) = second.await.unwrap();
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("vector payload bytes")
    );
    assert!(two.head().await.unwrap().is_none());
    assert_eq!(one.resource_usage().await.unwrap().vector_bytes, 8);
    assert_eq!(two.resource_usage().await.unwrap().vector_bytes, 8);
}

#[tokio::test]
async fn disk_budget_stops_model_work_and_lower_chunk_limit_can_rebuild_a_reduced_source() {
    let directory = Directory::new();
    let core = core().await;
    core.upsert_articles(&[paper("a", "immune first"), paper("b", "immune second")])
        .await
        .unwrap();
    let probe = Probe::new("disk-limit");
    let mut store = index(&core, &directory, probe.clone())
        .await
        .with_limits(SemanticLimits {
            max_disk_bytes: 1,
            ..Default::default()
        })
        .unwrap();
    assert!(
        store
            .sync()
            .await
            .unwrap_err()
            .to_string()
            .contains("cache disk bytes")
    );
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
    store = store.with_limits(SemanticLimits::default()).unwrap();
    store.sync().await.unwrap();
    store = store
        .with_limits(SemanticLimits {
            max_chunks: 1,
            ..Default::default()
        })
        .unwrap();
    assert!(store.sync().await.is_err());
    core.delete_article("b").await.unwrap();
    let (snapshot, report) = store.sync().await.unwrap();
    assert_eq!(snapshot.chunks.len(), 1);
    assert_eq!(report.embedded_chunks, 0);
    assert_eq!(store.resource_usage().await.unwrap().vector_bytes, 8);
}

#[tokio::test]
async fn snapshot_load_spans_batches_shares_duplicates_and_ignores_other_cache_rows() {
    let directory = Directory::new();
    let core = core().await;
    let mut articles = (0..300)
        .map(|i| paper(&format!("load-{i:04}"), &format!("immune source {i}")))
        .collect::<Vec<_>>();
    for id in ["duplicate-a", "duplicate-b"] {
        let mut article = paper(id, "shared immune source");
        article.title = "Shared title".into();
        articles.push(article);
    }
    core.upsert_articles(&articles).await.unwrap();
    let mut store = index(&core, &directory, Probe::new("snapshot-batches")).await;
    let (initial, _) = store.sync().await.unwrap();
    let mut foreign = index(&core, &directory, Probe::new("other-model")).await;
    foreign.sync().await.unwrap();
    foreign
        .store
        .execute(
            "UPDATE semanticVectors SET checksum='corrupt-unused-model' WHERE signature=?1",
            turso::params![foreign.signature.clone()],
        )
        .await
        .unwrap();
    store.store.execute("INSERT INTO semanticVectors(signature,inputHash,vector,checksum) VALUES(?1,'unreferenced',x'00','corrupt-unused-input')",turso::params![store.signature.clone()]).await.unwrap();
    let loaded = store.snapshot().await.unwrap().unwrap();
    assert_eq!(loaded.chunks.len(), 302);
    assert!(
        loaded
            .chunks
            .windows(2)
            .all(|c| c[0].chunk_id < c[1].chunk_id)
    );
    assert_eq!(
        initial
            .chunks
            .iter()
            .map(|c| (&c.chunk_id, &c.article_id, c.vector.as_slice()))
            .collect::<Vec<_>>(),
        loaded
            .chunks
            .iter()
            .map(|c| (&c.chunk_id, &c.article_id, c.vector.as_slice()))
            .collect::<Vec<_>>()
    );
    let duplicates = loaded
        .chunks
        .iter()
        .filter(|c| c.article_id.starts_with("duplicate-"))
        .collect::<Vec<_>>();
    assert_eq!(duplicates.len(), 2);
    assert!(Arc::ptr_eq(&duplicates[0].vector, &duplicates[1].vector));
    assert!(
        foreign.snapshot().await.is_err(),
        "a corrupt vector referenced by its own model must still fail"
    );
}

#[tokio::test]
async fn durable_restart_and_metadata_only_updates_reuse_model_outputs() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("revision-A");
    let mut article = paper("a", "immune regulation");
    core.upsert_article(&article).await.unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    let (first, report) = store.sync().await.unwrap();
    assert!(report.rebuilt);
    assert_eq!(report.embedded_chunks, 1);
    assert_eq!(first.chunks.len(), 1);
    let (idle, idle_report) = store.sync().await.unwrap();
    assert!(Arc::ptr_eq(&first, &idle));
    assert_eq!(idle_report.embedded_chunks, 0);
    let first_hash = first.chunks[0].content_hash.clone();
    article.year = Some(2026);
    core.upsert_article(&article).await.unwrap();
    let (second, report) = store.sync().await.unwrap();
    assert!(!report.rebuilt);
    assert_eq!(report.changed_articles, 1);
    assert_eq!(report.embedded_chunks, 0);
    assert_eq!(report.cached_chunks, 1);
    assert_ne!(first.generation, second.generation);
    assert_eq!(second.chunks[0].content_hash, first_hash);
    drop(store);
    let mut reopened = index(&core, &directory, probe.clone()).await;
    let (third, report) = reopened.sync().await.unwrap();
    assert_eq!(third.generation, second.generation);
    assert_eq!(report.embedded_chunks, 0);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn content_change_deletion_and_allowed_scope_precede_top_k() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("revision-A");
    core.upsert_article(&paper("a", "immune regulation"))
        .await
        .unwrap();
    core.upsert_article(&paper("b", "metabolic regulation"))
        .await
        .unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    let (old, _) = store.sync().await.unwrap();
    let restricted = old
        .rank(&[1., 0.], &HashSet::from(["b".into()]), 1)
        .unwrap();
    assert_eq!(restricted[0].0.article_id, "b");
    assert_eq!(restricted[0].1, 0.);
    core.upsert_article(&paper("a", "metabolic replacement"))
        .await
        .unwrap();
    let (changed, report) = store.sync().await.unwrap();
    assert_eq!(report.embedded_chunks, 1);
    let old_b = old.chunks.iter().find(|row| row.article_id == "b").unwrap();
    let next_b = changed
        .chunks
        .iter()
        .find(|row| row.article_id == "b")
        .unwrap();
    assert!(Arc::ptr_eq(&old_b.vector, &next_b.vector));
    assert_eq!(
        old.chunks
            .iter()
            .find(|row| row.article_id == "a")
            .unwrap()
            .vector
            .as_ref(),
        &vec![1., 0.]
    );
    assert_eq!(
        changed
            .chunks
            .iter()
            .find(|row| row.article_id == "a")
            .unwrap()
            .vector
            .as_ref(),
        &vec![0., 1.]
    );
    core.delete_article("a").await.unwrap();
    let (deleted, report) = store.sync().await.unwrap();
    assert_eq!(report.embedded_chunks, 0);
    assert_eq!(deleted.chunks.len(), 1);
    assert_eq!(deleted.chunks[0].article_id, "b");
    let mut rows = store
        .store
        .query("SELECT COUNT(*) FROM semanticVectors", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        1
    );
}

#[tokio::test]
async fn changed_model_and_preprocessing_never_reuse_other_geometry() {
    let directory = Directory::new();
    let core = core().await;
    core.upsert_article(&paper("a", "immune regulation"))
        .await
        .unwrap();
    let a = Probe::new("revision-A");
    let b = Probe::new("revision-B");
    let mut first = index(&core, &directory, a.clone()).await;
    let (old, _) = first.sync().await.unwrap();
    let mut second = index(&core, &directory, b.clone()).await;
    let (new, report) = second.sync().await.unwrap();
    assert_eq!(report.embedded_chunks, 1);
    assert_ne!(old.signature, new.signature);
    assert_eq!(
        first.snapshot().await.unwrap().unwrap().generation,
        old.generation
    );
    let revised = SemanticProfile {
        query_instruction: "Different query instruction".into(),
        ..profile()
    };
    let mut third = SemanticIndex::open(core.conn(), &directory.sidecar(), revised, a.clone())
        .await
        .unwrap();
    let (different, report) = third.sync().await.unwrap();
    assert_eq!(report.embedded_chunks, 1);
    assert_ne!(different.signature, old.signature);
    assert_eq!(a.calls.load(Ordering::SeqCst), 2);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn outage_keeps_previous_head_and_retry_reuses_completed_batches() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("revision-A");
    let mut store = index(&core, &directory, probe.clone()).await;
    let (old, _) = store.sync().await.unwrap();
    for i in 0..20 {
        core.upsert_article(&paper(&format!("a{i:02}"), "immune regulation"))
            .await
            .unwrap();
    }
    probe.fail_at.store(17, Ordering::SeqCst);
    assert!(store.sync().await.is_err());
    assert_eq!(
        store.snapshot().await.unwrap().unwrap().generation,
        old.generation
    );
    let (next, report) = store.sync().await.unwrap();
    assert_eq!(next.chunks.len(), 20);
    assert_eq!(report.cached_chunks, 16);
    assert_eq!(report.embedded_chunks, 4);
}

#[tokio::test]
async fn cancellation_is_not_a_partial_generation() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("revision-A");
    let mut store = index(&core, &directory, probe.clone()).await;
    let (old, _) = store.sync().await.unwrap();
    for i in 0..20 {
        core.upsert_article(&paper(&format!("a{i:02}"), "immune regulation"))
            .await
            .unwrap();
    }
    probe.pause_at.store(17, Ordering::SeqCst);
    {
        let pending = store.sync();
        tokio::pin!(pending);
        tokio::select! {result=&mut pending=>panic!("unexpected completion: {result:?}"),_=probe.entered.notified()=>{}}
    }
    assert_eq!(
        store.snapshot().await.unwrap().unwrap().generation,
        old.generation
    );
    probe.pause_at.store(18, Ordering::SeqCst);
    {
        let pending = store.sync();
        tokio::pin!(pending);
        tokio::select! {result=&mut pending=>panic!("unexpected completion: {result:?}"),_=probe.entered.notified()=>{}}
    }
    let mut rows = store
        .store
        .query("SELECT COUNT(*) FROM semanticGenerations", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        2
    );
    drop(rows);
    let (next, report) = store.sync().await.unwrap();
    assert_eq!(next.chunks.len(), 20);
    assert_eq!(report.embedded_chunks, 4);
    assert_eq!(report.cached_chunks, 16);
}

#[tokio::test]
async fn competing_builds_cannot_overwrite_a_newer_published_generation() {
    let directory = Directory::new();
    let path = directory
        .0
        .join("canonical.db")
        .to_string_lossy()
        .into_owned();
    let core = BibCore::open_with_search_engine(&path, SearchEngineMode::Tantivy)
        .await
        .unwrap();
    core.upsert_article(&paper("a", "immune regulation"))
        .await
        .unwrap();
    let db = turso::Builder::new_local(&path)
        .experimental_multiprocess_wal(true)
        .build()
        .await
        .unwrap();
    let slow = Probe::new("revision-A");
    let fast = Probe::new("revision-A");
    let mut a = SemanticIndex::open(
        db.connect().unwrap(),
        &directory.sidecar(),
        profile(),
        slow.clone(),
    )
    .await
    .unwrap();
    let mut b = SemanticIndex::open(db.connect().unwrap(), &directory.sidecar(), profile(), fast)
        .await
        .unwrap();
    slow.pause_at.store(1, Ordering::SeqCst);
    let winning_generation = {
        let pending = a.sync();
        tokio::pin!(pending);
        tokio::select! {result=&mut pending=>panic!("unexpected completion: {result:?}"),_=slow.entered.notified()=>{}}
        let (winner, _) = b.sync().await.unwrap();
        slow.release.notify_one();
        assert!(matches!(pending.await, Err(Error::SearchConflict(_))));
        winner.generation.clone()
    };
    let (latest, report) = a.sync().await.unwrap();
    assert_eq!(latest.generation, winning_generation);
    assert_eq!(report.embedded_chunks, 0);
    assert_eq!(latest.chunks.len(), 1);
}

#[tokio::test]
async fn restart_reaps_expired_staging_but_preserves_published_and_live_generations() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("revision-A");
    core.upsert_article(&paper("a", "immune regulation"))
        .await
        .unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    let (head, _) = store.sync().await.unwrap();
    store
        .store
        .execute("UPDATE semanticGenerations SET touchedAt=0", ())
        .await
        .unwrap();
    for (name, time) in [
        ("expired", 0),
        ("live", chrono::Utc::now().timestamp_millis()),
    ] {
        store
            .store
            .execute(
                "INSERT INTO semanticGenerations VALUES(?1,?2,?3,?4,2,?5)",
                turso::params![
                    name.to_owned(),
                    store.signature.clone(),
                    head.revision as i64,
                    head.token.clone(),
                    time
                ],
            )
            .await
            .unwrap();
    }
    drop(store);
    let reopened = index(&core, &directory, probe).await;
    assert_eq!(
        reopened.snapshot().await.unwrap().unwrap().generation,
        head.generation
    );
    let mut rows = reopened
        .store
        .query(
            "SELECT generation FROM semanticGenerations ORDER BY generation",
            (),
        )
        .await
        .unwrap();
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        ids.push(row.get::<String>(0).unwrap());
    }
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&head.generation));
    assert!(ids.contains(&"live".to_owned()));
}

struct InvalidProvider {
    output: EmbeddingOutput,
    changes_signature: bool,
    calls: AtomicUsize,
}
#[async_trait]
impl TextEmbedder for InvalidProvider {
    fn signature(&self) -> String {
        if self.changes_signature && self.calls.load(Ordering::SeqCst) > 0 {
            "revision-B".into()
        } else {
            "revision-A".into()
        }
    }
    async fn embed(&self, _: &EmbeddingInput) -> EmbedResult<EmbeddingOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.output.clone())
    }
}
#[tokio::test]
async fn bad_provider_geometry_and_mid_batch_signature_changes_do_not_publish() {
    for (signature, vector, changes) in [
        ("revision-A", vec![0., 0.], false),
        ("revision-A", vec![1., 0., 0.], false),
        ("revision-A", vec![f32::NAN, 0.], false),
        ("revision-B", vec![1., 0.], true),
    ] {
        let directory = Directory::new();
        let core = core().await;
        core.upsert_article(&paper("a", "immune regulation"))
            .await
            .unwrap();
        let provider = Arc::new(InvalidProvider {
            output: EmbeddingOutput::new(signature, vector),
            changes_signature: changes,
            calls: AtomicUsize::new(0),
        });
        let mut store = SemanticIndex::open(core.conn(), &directory.sidecar(), profile(), provider)
            .await
            .unwrap();
        assert!(store.sync().await.is_err());
        assert!(store.snapshot().await.unwrap().is_none());
    }
}

#[tokio::test]
async fn canonical_writes_during_embedding_are_not_locked_and_invalidate_publication() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("revision-A");
    let mut article = paper("a", "immune regulation");
    core.upsert_article(&article).await.unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    probe.pause_at.store(1, Ordering::SeqCst);
    let result = {
        let pending = store.sync();
        tokio::pin!(pending);
        tokio::select! {result=&mut pending=>panic!("unexpected completion: {result:?}"),_=probe.entered.notified()=>{}}
        article.abstract_text = Some("replacement metabolic text".into());
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            core.upsert_article(&article),
        )
        .await
        .unwrap()
        .unwrap();
        probe.release.notify_one();
        pending.await
    };
    assert!(matches!(result, Err(Error::SearchConflict(_))));
    assert!(store.snapshot().await.unwrap().is_none());
    let (next, _) = store.sync().await.unwrap();
    assert_eq!(next.chunks[0].vector.as_ref(), &vec![0., 1.]);
}

/// Long-running P4 qualification: lexical reads and canonical writes must stay
/// available while an embedding generation is blocked, and the blocked stale
/// generation must not publish. Run explicitly with `--ignored`.
#[tokio::test]
#[ignore = "long-running sustained-write qualification"]
async fn sustained_writes_keep_lexical_search_available_and_converge_after_pause() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("sustained-write-qualification");
    let mut original = paper("seed", "immune regulation");
    core.upsert_article(&original).await.unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    let (published, _) = store.sync().await.unwrap();

    original.abstract_text = Some("metabolic replacement after seed".into());
    core.upsert_article(&original).await.unwrap();
    probe
        .pause_at
        .store(probe.calls.load(Ordering::SeqCst) + 1, Ordering::SeqCst);

    let mut writes = 0usize;
    {
        let pending = store.sync();
        tokio::pin!(pending);
        tokio::select! {
            result = &mut pending => panic!("sync completed before the embedding gate: {result:?}"),
            _ = probe.entered.notified() => {}
        }

        let started = tokio::time::Instant::now();
        let duration = std::time::Duration::from_secs(60);
        while started.elapsed() < duration {
            let id = format!("sustained-{writes:04}");
            core.upsert_article(&paper(&id, &format!("lexicalprobe{writes:04}")))
                .await
                .unwrap();
            let page = core
                .search_v2(bib_types::SearchRequestV2 {
                    query: format!("lexicalprobe{writes:04}"),
                    ..Default::default()
                })
                .await
                .unwrap();
            assert!(page.hits.iter().any(|hit| hit.article_id == id));
            writes += 1;
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        probe.release.notify_one();
        assert!(matches!(pending.await, Err(Error::SearchConflict(_))));
    }
    assert!(
        writes >= 100,
        "only {writes} writes completed in 60 seconds"
    );
    assert_eq!(
        store.snapshot().await.unwrap().unwrap().generation,
        published.generation
    );
    let (latest, _) = store.sync().await.unwrap();
    assert!(latest.revision > published.revision);
    assert_eq!(latest.chunks.len(), writes + 1);
}

#[tokio::test]
async fn divergent_token_at_same_revision_forces_a_rebuild() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("revision-A");
    core.upsert_article(&paper("a", "immune regulation"))
        .await
        .unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    let (old, _) = store.sync().await.unwrap();
    core.upsert_article(&paper("a", "restored metabolic text"))
        .await
        .unwrap();
    let writer = core.write_conn();
    writer
        .execute(
            "UPDATE bib_meta SET value=?1 WHERE key='search_revision'",
            turso::params![old.revision.to_string()],
        )
        .await
        .unwrap();
    let (next, report) = store.sync().await.unwrap();
    assert!(report.rebuilt);
    assert_eq!(next.revision, old.revision);
    assert_ne!(next.token, old.token);
    assert_eq!(next.chunks[0].vector.as_ref(), &vec![0., 1.]);
}

#[tokio::test]
async fn checksum_corruption_and_missing_vector_never_look_like_empty_results() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("revision-A");
    core.upsert_article(&paper("a", "immune regulation"))
        .await
        .unwrap();
    let mut store = index(&core, &directory, probe).await;
    store.sync().await.unwrap();
    store
        .store
        .execute("UPDATE semanticVectors SET checksum='bad'", ())
        .await
        .unwrap();
    assert!(store.snapshot().await.is_err());
    store
        .store
        .execute("DELETE FROM semanticVectors", ())
        .await
        .unwrap();
    assert!(store.snapshot().await.is_err());
}

#[tokio::test]
async fn unicode_annotation_windows_keep_real_object_and_byte_offsets() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("revision-A");
    core.upsert_article(&paper("a", "immune regulation"))
        .await
        .unwrap();
    let text = "免疫细胞中的基因表达和调控机制需要结合具体实验分析。".repeat(5);
    let annotation = core
        .add_annotation("a", AnnotationKind::Note, &text, None, None, None, None)
        .await
        .unwrap();
    let mut rows = core
        .conn()
        .query(
            "SELECT content || ' ' || comment || ' ' || tags FROM annotations WHERE id=?1",
            turso::params![annotation.id.clone()],
        )
        .await
        .unwrap();
    let text = rows
        .next()
        .await
        .unwrap()
        .unwrap()
        .get::<String>(0)
        .unwrap();
    drop(rows);
    let policy = SemanticProfile {
        chunk_chars: Some(32),
        overlap_chars: 8,
        include_annotations: true,
        ..profile()
    };
    let mut store = SemanticIndex::open(core.conn(), &directory.sidecar(), policy, probe)
        .await
        .unwrap();
    let (snapshot, _) = store.sync().await.unwrap();
    let mut chunks = snapshot
        .chunks
        .iter()
        .filter(|row| row.kind == "annotation")
        .collect::<Vec<_>>();
    chunks.sort_by_key(|row| row.start_byte);
    assert!(chunks.len() > 1);
    assert_eq!(chunks[0].start_byte, 0);
    for chunk in chunks {
        assert!(text.is_char_boundary(chunk.start_byte));
        assert!(text.is_char_boundary(chunk.end_byte));
        assert_eq!(
            text[chunk.start_byte..chunk.end_byte].chars().count(),
            32.min(text[chunk.start_byte..].chars().count())
        );
        assert_eq!(
            chunk.anchor_key,
            annotation.item_key.as_deref().unwrap_or(&annotation.id)
        );
        assert_eq!(chunk.article_id, "a");
    }
}

#[test]
fn profiles_queries_and_vectors_reject_invalid_or_ambiguous_inputs() {
    for policy in [
        SemanticProfile {
            dimensions: 0,
            ..profile()
        },
        SemanticProfile {
            chunk_chars: Some(32),
            overlap_chars: 32,
            ..profile()
        },
        SemanticProfile {
            include_fulltext: true,
            ..profile()
        },
    ] {
        assert!(policy.validate().is_err());
    }
    assert!(profile().query_input(" ").is_err());
    assert_eq!(
        profile().query_input("immune").unwrap().text,
        "Instruct: Given a scientific claim, retrieve scientific papers that provide relevant evidence.\nQuery: immune"
    );
    assert!(normalized(&[0., 0.], 2).is_err());
    assert!(normalized(&[f32::NAN, 1.], 2).is_err());
    assert_eq!(normalized(&[f32::MAX, 0.], 2).unwrap(), vec![1., 0.]);
}

#[tokio::test]
async fn identical_inputs_share_vectors_across_embedding_batches_and_restart() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("shared-vectors");
    for i in 0..40 {
        let mut article = paper(&format!("same-{i:03}"), "immune regulation");
        article.title = "Same title".into();
        core.upsert_article(&article).await.unwrap();
    }
    let mut store = index(&core, &directory, probe.clone()).await;
    let (built, report) = store.sync().await.unwrap();
    assert_eq!(report.embedded_chunks, 1);
    assert_eq!(built.chunks.len(), 40);
    assert!(
        built
            .chunks
            .iter()
            .all(|chunk| Arc::ptr_eq(&chunk.vector, &built.chunks[0].vector))
    );
    drop(store);
    let mut reopened = index(&core, &directory, probe).await;
    let (restored, _) = reopened.sync().await.unwrap();
    assert!(
        restored
            .chunks
            .iter()
            .all(|chunk| Arc::ptr_eq(&chunk.vector, &restored.chunks[0].vector))
    );
    assert_eq!(restored.generation, built.generation);
}

#[tokio::test]
async fn blocked_model_batch_renews_staging_before_another_reader_reaps() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("long-batch");
    probe.pause_at.store(1, Ordering::SeqCst);
    core.upsert_article(&paper("a", "immune regulation"))
        .await
        .unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    let observer_db = turso::Builder::new_local(&directory.sidecar())
        .experimental_multiprocess_wal(true)
        .build()
        .await
        .unwrap();
    let observer = observer_db.connect().unwrap();
    let build = tokio::spawn(async move { store.sync().await });
    probe.entered.notified().await;
    observer
        .execute("UPDATE semanticGenerations SET touchedAt=0", ())
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let mut rows = observer
                .query("SELECT touchedAt FROM semanticGenerations", ())
                .await
                .unwrap();
            if rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap() > 0 {
                break;
            }
            drop(rows);
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let _other = index(&core, &directory, probe.clone()).await;
    probe.release.notify_one();
    assert_eq!(build.await.unwrap().unwrap().0.chunks.len(), 1);
}

#[tokio::test]
async fn idle_sync_reaps_expired_unpublished_generations_without_reopening() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("periodic-reaper");
    core.upsert_article(&paper("a", "immune regulation"))
        .await
        .unwrap();
    let mut store = index(&core, &directory, probe).await;
    let (head, _) = store.sync().await.unwrap();
    store
        .store
        .execute(
            "INSERT INTO semanticGenerations VALUES('abandoned',?1,0,'old-token',2,0)",
            turso::params![store.signature.clone()],
        )
        .await
        .unwrap();
    let (unchanged, _) = store.sync().await.unwrap();
    assert!(Arc::ptr_eq(&head, &unchanged));
    let mut rows = store
        .store
        .query("SELECT COUNT(*) FROM semanticGenerations", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        1
    );
}

#[tokio::test]
async fn reopened_higher_revision_backup_cannot_trust_the_previous_outbox_lineage() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("backup-lineage");
    core.upsert_article(&paper("a", "immune alpha"))
        .await
        .unwrap();
    core.upsert_article(&paper("b", "immune beta"))
        .await
        .unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    let (old, _) = store.sync().await.unwrap();
    drop(store);
    // A restored database may belong to a different branch with a larger
    // revision: b changed before the old head's revision, a changed after it.
    core.upsert_article(&paper("b", "metabolic restored beta"))
        .await
        .unwrap();
    core.upsert_article(&paper("a", "immune restored alpha"))
        .await
        .unwrap();
    let writer = core.write_conn();
    writer
        .execute("UPDATE searchOutbox SET revision=1 WHERE articleID='b'", ())
        .await
        .unwrap();
    let mut reopened = index(&core, &directory, probe.clone()).await;
    let (restored, report) = reopened.sync().await.unwrap();
    assert!(restored.revision > old.revision);
    assert!(report.rebuilt);
    assert_eq!(
        restored
            .chunks
            .iter()
            .find(|c| c.article_id == "b")
            .unwrap()
            .vector
            .as_ref(),
        &vec![0., 1.]
    );
    drop(reopened);
    let mut again = index(&core, &directory, probe).await;
    let (unchanged, report) = again.sync().await.unwrap();
    assert_eq!(unchanged.generation, restored.generation);
    assert_eq!(report.embedded_chunks, 0);
    assert!(!report.rebuilt);
}

async fn scalar(store: &SemanticIndex, sql: &str) -> i64 {
    store
        .store
        .query(sql, ())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap()
}

#[tokio::test]
async fn cleanup_failure_cannot_rollback_a_published_replacement() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("cleanup-failure");
    core.upsert_article(&paper("a", "immune original"))
        .await
        .unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    let (old, _) = store.sync().await.unwrap();
    store.store.execute_batch(&format!("CREATE TRIGGER refuseCleanup BEFORE DELETE ON semanticBindings WHEN OLD.generation='{}' BEGIN SELECT RAISE(ABORT,'injected cleanup failure'); END;",old.generation)).await.unwrap();
    core.upsert_article(&paper("a", "metabolic replacement"))
        .await
        .unwrap();
    let (new, _) = store.sync().await.unwrap();
    assert_ne!(new.generation, old.generation);
    assert_eq!(
        store.head().await.unwrap().unwrap().generation,
        new.generation
    );
    assert_eq!(
        store.snapshot().await.unwrap().unwrap().generation,
        new.generation
    );
    assert!(
        store
            .cleanup_warning()
            .unwrap()
            .contains("injected cleanup failure")
    );
    assert_eq!(new.chunks[0].vector.as_ref(), &vec![0., 1.]);
    let calls = probe.calls.load(Ordering::SeqCst);
    store
        .store
        .execute_batch("DROP TRIGGER refuseCleanup;")
        .await
        .unwrap();
    let (idle, _) = store.sync().await.unwrap();
    assert_eq!(idle.generation, new.generation);
    assert!(store.cleanup_warning().is_none());
    assert_eq!(probe.calls.load(Ordering::SeqCst), calls);
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticGenerations").await,
        1
    );
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticVectors").await,
        1
    );
}

#[tokio::test]
async fn vector_gc_failure_keeps_the_head_and_can_retry_without_embedding() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("vector-gc-failure");
    core.upsert_article(&paper("a", "immune original"))
        .await
        .unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    store.sync().await.unwrap();
    store.store.execute_batch("CREATE TRIGGER refuseVectorCleanup BEFORE DELETE ON semanticVectors BEGIN SELECT RAISE(ABORT,'injected vector cleanup failure'); END;").await.unwrap();
    core.upsert_article(&paper("a", "metabolic replacement"))
        .await
        .unwrap();
    let (new, _) = store.sync().await.unwrap();
    assert_eq!(
        store.head().await.unwrap().unwrap().generation,
        new.generation
    );
    assert!(
        store
            .cleanup_warning()
            .unwrap()
            .contains("injected vector cleanup failure")
    );
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticVectors").await,
        2
    );
    let calls = probe.calls.load(Ordering::SeqCst);
    store
        .store
        .execute_batch("DROP TRIGGER refuseVectorCleanup;")
        .await
        .unwrap();
    store.cleanup_bounded().await;
    assert!(store.cleanup_warning().is_none());
    assert_eq!(probe.calls.load(Ordering::SeqCst), calls);
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticVectors").await,
        1
    );
    assert_eq!(
        store.snapshot().await.unwrap().unwrap().generation,
        new.generation
    );
}

#[tokio::test]
async fn generation_gc_is_bounded_and_preserves_the_published_binding_set() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("bounded-gc");
    core.upsert_article(&paper("a", "immune original"))
        .await
        .unwrap();
    let mut store = index(&core, &directory, probe).await;
    let (published, _) = store.sync().await.unwrap();
    store
        .store
        .execute(
            "INSERT INTO semanticGenerations VALUES('retired',?1,0,'retired-token',2,0)",
            turso::params![store.signature.clone()],
        )
        .await
        .unwrap();
    let tx = Transaction::new_unchecked(&store.store, TransactionBehavior::Immediate)
        .await
        .unwrap();
    for i in 0..8193 {
        store.store.execute("INSERT INTO semanticBindings VALUES('retired',?1,'retired-doc','retired-article','retired-anchor','item','retired-content',0,1,'retired-input')",turso::params![format!("retired-{i:05}")]).await.unwrap();
    }
    tx.commit().await.unwrap();
    store.cleanup_bounded().await;
    assert!(store.cleanup_warning().is_none());
    assert_eq!(
        scalar(
            &store,
            "SELECT COUNT(*) FROM semanticBindings WHERE generation='retired'"
        )
        .await,
        1
    );
    assert_eq!(
        store.snapshot().await.unwrap().unwrap().generation,
        published.generation
    );
    store.cleanup_bounded().await;
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticBindings").await,
        1
    );
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticGenerations").await,
        1
    );
}

#[tokio::test]
async fn vector_gc_resumes_its_durable_scan_after_reopening() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("gc-checkpoint");
    core.upsert_article(&paper("a", "immune original"))
        .await
        .unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    let (published, _) = store.sync().await.unwrap();
    let tx = Transaction::new_unchecked(&store.store, TransactionBehavior::Immediate)
        .await
        .unwrap();
    for i in 0..4100 {
        store
            .store
            .execute(
                "INSERT INTO semanticVectors VALUES(?1,?2,x'00','unreferenced-invalid-geometry')",
                turso::params![store.signature.clone(), format!("zz-orphan-{i:05}")],
            )
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    store.cleanup_bounded().await;
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticVectors").await,
        6
    );
    let mut rows = store
        .store
        .query(
            "SELECT afterInputHash FROM semanticGarbageCollection WHERE signature=?1",
            turso::params![store.signature.clone()],
        )
        .await
        .unwrap();
    let checkpoint: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    drop(rows);
    assert!(checkpoint.starts_with("zz-orphan-"));
    drop(store);
    let mut reopened = index(&core, &directory, probe.clone()).await;
    let (same, _) = reopened.sync().await.unwrap();
    assert_eq!(same.generation, published.generation);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        scalar(&reopened, "SELECT COUNT(*) FROM semanticVectors").await,
        1
    );
    assert_eq!(reopened.resource_usage().await.unwrap().vector_bytes, 8);
}

#[tokio::test]
async fn interrupted_binding_copy_keeps_head_and_retries_across_page_boundary() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("binding-page-retry");
    let articles = (0..2051)
        .map(|i| paper(&format!("copy-{i:05}"), &format!("immune study {i}")))
        .collect::<Vec<_>>();
    core.upsert_articles(&articles).await.unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    let (original, _) = store.sync().await.unwrap();
    let mut changed = articles[0].clone();
    changed.title = "metabolic replacement".into();
    changed.abstract_text = Some("metabolic replacement evidence".into());
    core.upsert_article(&changed).await.unwrap();
    // Fail on the first row of the second copy page. Audit writes in the
    // completed first page survive; writes in a failed page are rolled back.
    store.store.execute_batch("CREATE TABLE copyAudit(chunkID TEXT); CREATE TRIGGER failCopy BEFORE INSERT ON semanticBindings WHEN (SELECT COUNT(*) FROM semanticBindings WHERE generation=NEW.generation)>=2048 BEGIN SELECT RAISE(ABORT,'injected copy page failure'); END; CREATE TRIGGER auditCopy AFTER INSERT ON semanticBindings BEGIN INSERT INTO copyAudit VALUES(NEW.chunkID); END;").await.unwrap();
    assert!(
        store
            .sync()
            .await
            .unwrap_err()
            .to_string()
            .contains("injected copy page failure")
    );
    assert_eq!(scalar(&store, "SELECT COUNT(*) FROM copyAudit").await, 2048);
    assert_eq!(
        store.head().await.unwrap().unwrap().generation,
        original.generation
    );
    assert_eq!(store.snapshot().await.unwrap().unwrap().chunks.len(), 2051);
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticBindings").await,
        2051
    );
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2051);
    store
        .store
        .execute_batch("DROP TRIGGER failCopy; DROP TRIGGER auditCopy;")
        .await
        .unwrap();
    let (next, report) = store.sync().await.unwrap();
    assert_ne!(next.generation, original.generation);
    assert_eq!(next.chunks.len(), 2051);
    assert_eq!(report.embedded_chunks, 1);
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticBindings").await,
        2051
    );
    assert_eq!(
        next.chunks
            .iter()
            .find(|c| c.article_id == changed.id)
            .unwrap()
            .vector
            .as_ref(),
        &vec![0., 1.]
    );
    drop(store);
    let mut reopened = index(&core, &directory, probe).await;
    assert_eq!(reopened.sync().await.unwrap().0.generation, next.generation);
}

#[tokio::test]
async fn metadata_aliases_keep_one_binding_set_across_edits_restart_and_reaping() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("metadata-alias");
    let mut a = paper("a", "immune alpha");
    core.upsert_article(&a).await.unwrap();
    core.upsert_article(&paper("b", "immune beta"))
        .await
        .unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    let (original, _) = store.sync().await.unwrap();
    let root = original.generation.clone();
    for year in 2000..2020 {
        a.year = Some(year);
        core.upsert_article(&a).await.unwrap();
        let (next, report) = store.sync().await.unwrap();
        assert_eq!(report.embedded_chunks, 0);
        assert_eq!(report.cached_chunks, 1);
        assert_ne!(next.generation, root);
        assert_eq!(
            store.binding_generation(&next.generation).await.unwrap(),
            root
        );
        for chunk in &next.chunks {
            let old = original
                .chunks
                .iter()
                .find(|old| old.chunk_id == chunk.chunk_id)
                .unwrap();
            assert!(Arc::ptr_eq(&chunk.vector, &old.vector));
        }
        assert_eq!(
            scalar(&store, "SELECT COUNT(*) FROM semanticBindings").await,
            2
        );
        assert_eq!(
            scalar(&store, "SELECT COUNT(*) FROM semanticGenerations").await,
            2
        );
        assert_eq!(
            scalar(&store, "SELECT COUNT(*) FROM semanticBindingAliases").await,
            1
        );
    }
    store
        .store
        .execute("UPDATE semanticGenerations SET touchedAt=0", ())
        .await
        .unwrap();
    let generation = store.current.as_ref().unwrap().generation.clone();
    drop(store);
    let mut store = index(&core, &directory, probe.clone()).await;
    let (restored, _) = store.sync().await.unwrap();
    assert_eq!(restored.generation, generation);
    assert_eq!(restored.chunks.len(), 2);
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticGenerations").await,
        2
    );
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    // A real content change materializes a new complete set and collects both
    // the old alias and its now-unreferenced backing set.
    core.upsert_article(&paper("b", "metabolic replacement"))
        .await
        .unwrap();
    let (changed, report) = store.sync().await.unwrap();
    assert_eq!(report.embedded_chunks, 1);
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticBindingAliases").await,
        0
    );
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticGenerations").await,
        1
    );
    assert_eq!(changed.chunks.len(), 2);
    assert_eq!(
        changed
            .chunks
            .iter()
            .find(|c| c.article_id == "b")
            .unwrap()
            .vector
            .as_ref(),
        &vec![0., 1.]
    );
    core.delete_article("a").await.unwrap();
    let (deleted, _) = store.sync().await.unwrap();
    assert_eq!(deleted.chunks.len(), 1);
    assert_eq!(deleted.chunks[0].article_id, "b");
    drop(store);
    let mut store = index(&core, &directory, probe).await;
    assert_eq!(store.sync().await.unwrap().0.generation, deleted.generation);
}

#[tokio::test]
async fn failed_metadata_alias_insert_rolls_back_generation_and_head() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("alias-rollback");
    let mut article = paper("a", "immune alpha");
    core.upsert_article(&article).await.unwrap();
    let mut store = index(&core, &directory, probe.clone()).await;
    let (old, _) = store.sync().await.unwrap();
    // Fault-injected constraint, while no aliases exist yet.
    store.store.execute_batch("DROP TABLE semanticBindingAliases; CREATE TABLE semanticBindingAliases(generation TEXT PRIMARY KEY CHECK(generation='injected-rejection'),bindingGeneration TEXT NOT NULL);").await.unwrap();
    article.year = Some(2026);
    core.upsert_article(&article).await.unwrap();
    assert!(store.sync().await.is_err());
    assert_eq!(
        store.head().await.unwrap().unwrap().generation,
        old.generation
    );
    assert_eq!(
        scalar(&store, "SELECT COUNT(*) FROM semanticGenerations").await,
        1
    );
    assert!(Arc::ptr_eq(store.current.as_ref().unwrap(), &old));
    store
        .store
        .execute("DROP TABLE semanticBindingAliases", ())
        .await
        .unwrap();
    store.store.execute_batch(SCHEMA).await.unwrap();
    let (recovered, report) = store.sync().await.unwrap();
    assert_ne!(recovered.generation, old.generation);
    assert_eq!(report.embedded_chunks, 0);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_metadata_alias_cannot_replace_a_competing_published_head() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("alias-cas");
    let mut article = paper("a", "immune alpha");
    core.upsert_article(&article).await.unwrap();
    let mut first = index(&core, &directory, probe.clone()).await;
    let (old, _) = first.sync().await.unwrap();
    let old_head = first.head().await.unwrap().unwrap();
    article.year = Some(2026);
    core.upsert_article(&article).await.unwrap();
    let mut other = index(&core, &directory, probe).await;
    let (winner, _) = other.sync().await.unwrap();
    let result = first
        .metadata_only(&old_head, winner.revision, &winner.token, &[article.id])
        .await;
    assert!(matches!(result, Err(Error::SearchConflict(_))));
    assert_eq!(
        first.head().await.unwrap().unwrap().generation,
        winner.generation
    );
    assert_eq!(
        scalar(&first, "SELECT COUNT(*) FROM semanticBindingAliases").await,
        0
    );
    assert_eq!(old.chunks.len(), 1);
}

#[tokio::test]
async fn persisted_aliases_reject_cross_profile_and_nested_backing_sets() {
    let directory = Directory::new();
    let core = core().await;
    let probe = Probe::new("alias-validation");
    let mut article = paper("a", "immune alpha");
    core.upsert_article(&article).await.unwrap();
    let mut store = index(&core, &directory, probe).await;
    let (root, _) = store.sync().await.unwrap();
    article.year = Some(2026);
    core.upsert_article(&article).await.unwrap();
    let (alias, _) = store.sync().await.unwrap();
    store.store.execute("INSERT INTO semanticGenerations VALUES('bad-profile','different-signature',0,'token',2,0)", ()).await.unwrap();
    store
        .store
        .execute(
            "UPDATE semanticBindingAliases SET bindingGeneration='bad-profile' WHERE generation=?1",
            turso::params![alias.generation.clone()],
        )
        .await
        .unwrap();
    assert!(store.snapshot().await.is_err());
    store
        .store
        .execute(
            "INSERT INTO semanticGenerations VALUES('nested',?1,0,'token',2,0)",
            turso::params![store.signature.clone()],
        )
        .await
        .unwrap();
    store
        .store
        .execute(
            "INSERT INTO semanticBindingAliases VALUES('nested',?1)",
            turso::params![root.generation.clone()],
        )
        .await
        .unwrap();
    store
        .store
        .execute(
            "UPDATE semanticBindingAliases SET bindingGeneration='nested' WHERE generation=?1",
            turso::params![alias.generation.clone()],
        )
        .await
        .unwrap();
    assert!(store.snapshot().await.is_err());
}

#[test]
fn exact_top_k_matches_exhaustive_chunk_order_for_scopes_and_ties() {
    // Independent oracle: order every eligible chunk, retain the first chunk
    // per article, then take K. Covers max-over-chunks, both levels of ties,
    // negative scores, excluded high scores and K at every boundary.
    let mut seed = 17_u64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        ((seed % 21) as f32 - 10.) / 10.
    };
    let mut chunks = Vec::new();
    for article in 0..127 {
        let vector = Arc::new((0..7).map(|_| random()).collect::<Vec<_>>());
        for part in (0..3).rev() {
            chunks.push(SemanticChunk {
                chunk_id: format!("chunk-{article:03}-{part}"),
                doc_id: format!("doc-{article:03}-{part}"),
                article_id: format!("article-{article:03}"),
                anchor_key: format!("attachment-{part}"),
                kind: "attachment".into(),
                content_hash: format!("content-{article}-{part}"),
                start_byte: part * 10,
                end_byte: part * 10 + 10,
                vector: if part < 2 {
                    vector.clone()
                } else {
                    Arc::new((0..7).map(|_| random()).collect())
                },
            });
        }
    }
    let snapshot = SemanticSnapshot {
        signature: "probe".into(),
        generation: "generation".into(),
        revision: 1,
        token: "token".into(),
        dimensions: 7,
        chunks,
    };
    for query in [
        vec![1.; 7],
        vec![-1.; 7],
        vec![1., 0., -1., 0., 1., 0., -1.],
    ] {
        let normalized = normalized(&query, 7).unwrap();
        for stride in [1, 3, 127, 128] {
            let allowed = (0..127)
                .filter(|i| i % stride == stride - 1)
                .map(|i| format!("article-{i:03}"))
                .collect::<HashSet<_>>();
            let mut oracle = snapshot
                .chunks
                .iter()
                .filter(|c| allowed.contains(&c.article_id))
                .map(|c| {
                    let score = c
                        .vector
                        .iter()
                        .zip(&normalized)
                        .map(|(a, b)| f64::from(*a) * f64::from(*b))
                        .sum::<f64>();
                    (c, score)
                })
                .collect::<Vec<_>>();
            oracle.sort_by(|a, b| {
                b.1.total_cmp(&a.1)
                    .then_with(|| a.0.article_id.cmp(&b.0.article_id))
                    .then_with(|| a.0.chunk_id.cmp(&b.0.chunk_id))
            });
            let mut seen = HashSet::new();
            oracle.retain(|(c, _)| seen.insert(&c.article_id));
            for limit in [0, 1, 7, 31, 127, 200] {
                let actual = snapshot.rank(&query, &allowed, limit).unwrap();
                assert_eq!(actual.len(), oracle.len().min(limit));
                for ((a, score), (expected, expected_score)) in actual.iter().zip(&oracle) {
                    assert_eq!(a.chunk_id, expected.chunk_id);
                    assert_eq!(a.doc_id, expected.doc_id);
                    assert_eq!(a.anchor_key, expected.anchor_key);
                    assert_eq!(a.content_hash, expected.content_hash);
                    assert_eq!(
                        (a.start_byte, a.end_byte),
                        (expected.start_byte, expected.end_byte)
                    );
                    assert!(Arc::ptr_eq(&a.vector, &expected.vector));
                    assert_eq!(score.to_bits(), expected_score.to_bits());
                }
            }
        }
    }
    assert!(snapshot.rank(&[0.; 7], &HashSet::new(), 0).is_err());
    assert!(snapshot.rank(&[f32::NAN; 7], &HashSet::new(), 0).is_err());
}

#[test]
fn bounded_parallel_rank_preserves_global_max_and_ties_across_partitions() {
    let mut chunks = Vec::new();
    let mut allowed = HashSet::new();
    for part in (0..4).rev() {
        for article in (0..20000).rev() {
            let id = format!("parallel-{article:05}");
            allowed.insert(id.clone());
            chunks.push(SemanticChunk {
                chunk_id: format!("{id}-{part}"),
                doc_id: id.clone(),
                article_id: id,
                anchor_key: format!("attachment-{part}"),
                kind: "attachment".into(),
                content_hash: "content".into(),
                start_byte: 0,
                end_byte: 1,
                vector: Arc::new(if part < 2 {
                    vec![1., 0.]
                } else {
                    vec![-1., 0.]
                }),
            });
        }
    }
    let snapshot = SemanticSnapshot {
        signature: "parallel".into(),
        generation: "generation".into(),
        revision: 1,
        token: "token".into(),
        dimensions: 2,
        chunks,
    };
    for limit in [0, 1, 100, 20000, 20001] {
        let actual = snapshot.rank(&[1., 0.], &allowed, limit).unwrap();
        assert_eq!(actual.len(), limit.min(20000));
        for (index, (chunk, score)) in actual.iter().enumerate() {
            assert_eq!(chunk.article_id, format!("parallel-{index:05}"));
            assert_eq!(chunk.chunk_id, format!("parallel-{index:05}-0"));
            assert_eq!(score.to_bits(), 1_f64.to_bits());
        }
    }
}
