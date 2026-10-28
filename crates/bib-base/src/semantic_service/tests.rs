use super::*;
use crate::{SearchEngineMode, bib_base::BibCore};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use text_embedding::{EmbeddingInput, EmbeddingOutput, provider::error::Result as EmbedResult};
use tokio::sync::Notify;
mod maintenance_tests;

#[derive(Default)]
struct Probe {
    connections: AtomicUsize,
    calls: AtomicUsize,
    pause_at: AtomicUsize,
    block: AtomicBool,
    outage: AtomicBool,
    entered: Notify,
    release: Notify,
}
struct TestConnector(Arc<Probe>);
struct TestEmbedder {
    probe: Arc<Probe>,
    signature: String,
}
#[async_trait]
impl Connector for TestConnector {
    async fn connect(&self, config: &SemanticConfig) -> Result<Arc<dyn TextEmbedder>> {
        self.0.connections.fetch_add(1, Ordering::SeqCst);
        if self.0.outage.load(Ordering::SeqCst) {
            return Err(invalid("injected model outage"));
        }
        Ok(Arc::new(TestEmbedder {
            probe: self.0.clone(),
            signature: config.model_digest.clone(),
        }))
    }
}
#[async_trait]
impl TextEmbedder for TestEmbedder {
    fn signature(&self) -> String {
        self.signature.clone()
    }
    async fn embed(&self, _input: &EmbeddingInput) -> EmbedResult<EmbeddingOutput> {
        let call = self.probe.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.probe.block.load(Ordering::SeqCst)
            || self.probe.pause_at.load(Ordering::SeqCst) == call
        {
            self.probe.entered.notify_one();
            self.probe.release.notified().await;
        }
        Ok(EmbeddingOutput::new(self.signature(), vec![1., 2.]))
    }
}
struct Fixture {
    core: BibCore,
    db: turso::Database,
    path: std::path::PathBuf,
    probe: Arc<Probe>,
}
impl Fixture {
    async fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("bib-semantic-worker-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let source = path.join("source.db").to_string_lossy().into_owned();
        let mut core = BibCore::open_with_search_engine(&source, SearchEngineMode::Tantivy)
            .await
            .unwrap();
        // Structural tests supply a deterministic connector; do not leave the
        // default Ollama observer running against the same test database.
        core.semantic = None;
        core.search_context.as_mut().unwrap().semantic = None;
        let db = turso::Builder::new_local(&source)
            .experimental_multiprocess_wal(true)
            .build()
            .await
            .unwrap();
        Self {
            core,
            db,
            path,
            probe: Arc::new(Probe::default()),
        }
    }
    async fn service(&self) -> SemanticService {
        SemanticService::open_with_connector(
            self.db.connect().unwrap(),
            self.db.connect().unwrap(),
            self.db.connect().unwrap(),
            self.path.join("vectors.db").to_string_lossy().into_owned(),
            Arc::new(TestConnector(self.probe.clone())),
        )
        .await
        .unwrap()
    }
    async fn paper(&self, id: &str) {
        let mut article = bib_types::Article::new(id, "Actual canonical title");
        article.abstract_text = Some(format!("Canonical abstract for {id}"));
        self.core.upsert_article(&article).await.unwrap();
    }
    async fn library(&self, service: &SemanticService) -> crate::BibBase {
        let path = self.path.join("source.db").to_string_lossy().into_owned();
        let mut core = BibCore::open_with_search_engine(&path, SearchEngineMode::Tantivy)
            .await
            .unwrap();
        core.semantic = Some(service.clone());
        core.search_context.as_mut().unwrap().semantic = Some(service.clone());
        crate::actor::spawn(core)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
fn enabled() -> SemanticConfig {
    SemanticConfig {
        enabled: true,
        model_digest: "a".repeat(64),
        profile: SemanticProfile {
            dimensions: 2,
            ..Default::default()
        },
        ..Default::default()
    }
}
async fn wait(service: &SemanticService, phase: SemanticPhase) -> SemanticStatus {
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut state = service.state.clone();
        loop {
            let status = state.borrow_and_update().status.clone();
            if status.phase == phase {
                return status;
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}

#[test]
fn explicit_model_and_origin_policy() {
    assert!(SemanticConfig::default().validate().is_ok());
    let mut config = enabled();
    for origin in [
        "http://127.0.0.2:11434",
        "http://[::1]:11434",
        "http://localhost:11434",
    ] {
        config.origin = origin.into();
        assert!(config.validate().is_ok());
    }
    config.origin = "https://embedding.example".into();
    assert!(config.validate().is_err());
    config.allow_remote = true;
    assert!(config.validate().is_ok());
    for origin in [
        "https://user:password@embedding.example",
        "https://embedding.example/path",
        "https://embedding.example?secret=hidden",
        "file:///tmp/model",
    ] {
        config.origin = origin.into();
        assert!(config.validate().is_err());
    }
    config = enabled();
    config.model_digest = "A".repeat(64);
    assert!(config.validate().is_err());
    config = enabled();
    config.batch_size = 65;
    assert!(config.validate().is_err());
}

#[test]
fn a_retry_copy_phase_is_indexing_while_idle_planning_preserves_ready_state() {
    let config = VersionedConfig {
        config: enabled(),
        generation: "control-generation".into(),
        maintenance: None,
    };
    let mut state = initial(&config);
    state.ready = Some(Ready {
        provider: Arc::new(TestEmbedder {
            probe: Arc::new(Probe::default()),
            signature: "fixture".into(),
        }),
        profile: config.config.profile.clone(),
        snapshot: Arc::new(SemanticSnapshot {
            signature: "fixture".into(),
            generation: "published".into(),
            revision: 1,
            token: "old-token".into(),
            dimensions: 2,
            chunks: Vec::new(),
        }),
        store_generation: String::new(),
    });
    state.status.phase = SemanticPhase::Failed;
    state.status.last_error = Some("search conflict: source changed".into());
    let (sender, _receiver) = watch::channel(state);
    progress_callback(
        &sender,
        &config.generation,
        SemanticProgress {
            stage: "copying_bindings".into(),
            ..Default::default()
        },
    );
    assert_eq!(sender.borrow().status.phase, SemanticPhase::Indexing);
    sender.send_modify(|s| s.status.phase = SemanticPhase::Ready);
    progress_callback(
        &sender,
        &config.generation,
        SemanticProgress {
            stage: "planning".into(),
            ..Default::default()
        },
    );
    assert_eq!(sender.borrow().status.phase, SemanticPhase::Ready);
}

#[tokio::test]
async fn resource_policy_failure_preserves_writes_and_lexical_search_then_reuses_vectors() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    fixture.paper("b").await;
    let service = fixture.service().await;
    let mut config = enabled();
    service.configure(config.clone()).await.unwrap();
    let initial = wait(&service, SemanticPhase::Ready).await;
    let calls = fixture.probe.calls.load(Ordering::SeqCst);
    assert_eq!(initial.resources.as_ref().unwrap().vector_bytes, 16);
    config.limits.max_chunks = 1;
    service.configure(config.clone()).await.unwrap();
    let failed = wait(&service, SemanticPhase::Failed).await;
    assert!(failed.last_error.unwrap().contains("index chunks"));
    let library = fixture.library(&service).await;
    assert_eq!(
        library
            .search_v2(bib_types::SearchRequestV2::default())
            .await
            .unwrap()
            .total,
        2
    );
    fixture.paper("c").await;
    assert_eq!(
        library
            .search_v2(bib_types::SearchRequestV2::default())
            .await
            .unwrap()
            .total,
        3
    );
    config.limits.max_chunks = 3;
    service.configure(config.clone()).await.unwrap();
    let restored = wait(&service, SemanticPhase::Ready).await;
    assert_eq!(restored.signature, initial.signature);
    assert_eq!(restored.indexed_chunks, 3);
    assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), calls + 1);
    assert_eq!(restored.resources.as_ref().unwrap().vector_bytes, 24);
    let before = fixture.probe.calls.load(Ordering::SeqCst);
    config.limits.max_input_bytes = 64;
    service.configure(config).await.unwrap();
    wait(&service, SemanticPhase::Ready).await;
    assert!(
        service
            .prepare("immune gene expression")
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("query input bytes")
    );
    assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), before);
}

#[test]
fn inference_overrides_preserve_old_config_and_reject_invalid_limits() {
    let old: SemanticConfig = serde_json::from_str("{}").unwrap();
    assert!(old.inference.is_empty());
    let mut config = enabled();
    config.inference = OllamaInferenceOptions {
        num_ctx: Some(8192),
        num_batch: Some(8192),
    };
    assert!(config.validate().is_ok());
    assert_eq!(
        serde_json::from_str::<SemanticConfig>(&serde_json::to_string(&config).unwrap()).unwrap(),
        config
    );
    for value in [0, 131073] {
        config.inference.num_ctx = Some(value);
        assert!(config.validate().is_err());
    }
    config.inference.num_ctx = Some(8192);
    for value in [0, 8193] {
        config.inference.num_batch = Some(value);
        assert!(config.validate().is_err());
    }
    assert!(serde_json::from_str::<SemanticConfig>(r#"{"inference":{"unknown":1}}"#).is_err());
}

#[tokio::test]
async fn resource_failure_waits_without_progress_flapping_and_source_reduction_resumes_it() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    fixture.paper("b").await;
    let service = fixture.service().await;
    let mut config = enabled();
    config.limits.max_chunks = 1;
    service.configure(config).await.unwrap();
    wait(&service, SemanticPhase::Failed).await;
    let second = fixture.service().await;
    wait(&second, SemanticPhase::Failed).await;
    let mut observer = service.state.clone();
    observer.borrow_and_update();
    let mut second_observer = second.state.clone();
    second_observer.borrow_and_update();
    tokio::time::sleep(Duration::from_millis(3200)).await;
    assert!(
        !observer.has_changed().unwrap(),
        "unchanged resource failure should not restart planning or flicker progress"
    );
    assert!(
        !second_observer.has_changed().unwrap(),
        "other workers' staging/WAL writes must not restart a chunk-limited build"
    );
    fixture.core.delete_article("b").await.unwrap();
    let ready = wait(&service, SemanticPhase::Ready).await;
    assert_eq!(ready.indexed_chunks, 1);
    wait(&second, SemanticPhase::Ready).await;
    assert!((1..=2).contains(&fixture.probe.calls.load(Ordering::SeqCst)));
}

#[tokio::test]
async fn over_disk_budget_repair_does_not_create_another_private_store() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    let service = fixture.service().await;
    let mut config = enabled();
    service.configure(config.clone()).await.unwrap();
    wait(&service, SemanticPhase::Ready).await;
    config.limits.max_disk_bytes = 1;
    let epoch = service.configure(config).await.unwrap();
    wait(&service, SemanticPhase::Failed).await;
    let id = uuid::Uuid::new_v4().to_string();
    service
        .maintain(SemanticMaintenanceRequest {
            action: SemanticMaintenanceAction::Repair,
            expected_configuration_generation: epoch,
            request_id: id.clone(),
        })
        .await
        .unwrap();
    let failed = wait(&service, SemanticPhase::Failed).await;
    assert_eq!(failed.maintenance.as_ref().unwrap().request_id, id);
    assert!(
        !fixture
            .path
            .join(format!("vectors.db.repair-{id}"))
            .exists()
    );
    assert!(fixture.path.join("vectors.db").exists());
}

#[tokio::test]
async fn default_is_disabled_and_persisted_restart_reuses_vectors() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    let service = fixture.service().await;
    assert_eq!(service.status().phase, SemanticPhase::Disabled);
    assert_eq!(fixture.probe.connections.load(Ordering::SeqCst), 0);
    assert!(service.prepare("topic").await.is_err());
    let configuration = service.configure(enabled()).await.unwrap();
    let ready = wait(&service, SemanticPhase::Ready).await;
    assert_eq!(ready.configuration_generation, configuration);
    assert_eq!(ready.indexed_chunks, 1);
    let query = service.prepare("immune regulation").await.unwrap();
    assert_eq!(query.snapshot.generation, ready.generation.unwrap());
    assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), 2);
    drop(service);
    let reopened = fixture.service().await;
    let restored = wait(&reopened, SemanticPhase::Ready).await;
    assert_eq!(restored.configuration_generation, configuration);
    assert_eq!(restored.generation, Some(query.snapshot.generation.clone()));
    assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), 2);
    let config = SemanticConfig {
        enabled: false,
        ..enabled()
    };
    reopened.configure(config).await.unwrap();
    assert_eq!(reopened.status().phase, SemanticPhase::Disabled);
    assert!(reopened.prepare("topic").await.is_err());
}

#[tokio::test]
async fn disabling_cancels_indexing_without_blocking_canonical_writes() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    fixture.probe.block.store(true, Ordering::SeqCst);
    let service = fixture.service().await;
    service.configure(enabled()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), fixture.probe.entered.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), fixture.paper("b"))
        .await
        .unwrap();
    assert_eq!(
        fixture
            .core
            .search_v2(bib_types::SearchRequestV2 {
                query: "Canonical".into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .total,
        2
    );
    service.configure(SemanticConfig::default()).await.unwrap();
    assert_eq!(service.status().phase, SemanticPhase::Disabled);
    assert!(service.state.borrow().ready.is_none());
    fixture.probe.block.store(false, Ordering::SeqCst);
    service.configure(enabled()).await.unwrap();
    assert_eq!(wait(&service, SemanticPhase::Ready).await.indexed_chunks, 2);
}

#[tokio::test]
async fn reconfiguration_cancels_in_flight_query_and_isolates_model_generation() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    let service = fixture.service().await;
    service.configure(enabled()).await.unwrap();
    let old = wait(&service, SemanticPhase::Ready).await;
    fixture.probe.block.store(true, Ordering::SeqCst);
    let task = {
        let service = service.clone();
        tokio::spawn(async move { service.prepare("topic").await })
    };
    tokio::time::timeout(Duration::from_secs(5), fixture.probe.entered.notified())
        .await
        .unwrap();
    service.configure(SemanticConfig::default()).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(Error::SearchConflict(_))));
    fixture.probe.block.store(false, Ordering::SeqCst);
    service
        .configure(SemanticConfig {
            model_digest: "b".repeat(64),
            ..enabled()
        })
        .await
        .unwrap();
    let new = wait(&service, SemanticPhase::Ready).await;
    assert_ne!(old.signature, new.signature);
    assert_ne!(old.generation, new.generation);
    assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn model_outage_and_invalid_configuration_preserve_lexical_search() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    let service = fixture.service().await;
    let generation = service.status().configuration_generation;
    assert!(
        service
            .configure(SemanticConfig {
                model_digest: String::new(),
                ..enabled()
            })
            .await
            .is_err()
    );
    assert_eq!(service.status().configuration_generation, generation);
    fixture.probe.outage.store(true, Ordering::SeqCst);
    service.configure(enabled()).await.unwrap();
    assert!(
        wait(&service, SemanticPhase::Failed)
            .await
            .last_error
            .is_some()
    );
    assert_eq!(
        fixture
            .core
            .search_v2(bib_types::SearchRequestV2 {
                query: "Canonical".into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .total,
        1
    );
    fixture.probe.outage.store(false, Ordering::SeqCst);
    service.configure(enabled()).await.unwrap();
    assert_eq!(wait(&service, SemanticPhase::Ready).await.indexed_chunks, 1);
}

#[tokio::test]
async fn configuration_changes_from_another_handle_are_observed() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    let writer = fixture.service().await;
    let observer = fixture.service().await;
    let generation = writer.configure(enabled()).await.unwrap();
    assert_eq!(
        wait(&observer, SemanticPhase::Ready)
            .await
            .configuration_generation,
        generation
    );
    let disabled = writer.configure(SemanticConfig::default()).await.unwrap();
    assert_eq!(
        wait(&observer, SemanticPhase::Disabled)
            .await
            .configuration_generation,
        disabled
    );
    assert!(observer.prepare("topic").await.is_err());
}

#[tokio::test]
async fn corrupt_persisted_configuration_can_be_repaired_without_breaking_library() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    fixture
        .db
        .connect()
        .unwrap()
        .execute(
            "INSERT INTO bibliographySemanticConfig VALUES(1,'{broken','bad')",
            (),
        )
        .await
        .unwrap();
    let service = fixture.service().await;
    assert_eq!(service.status().phase, SemanticPhase::Failed);
    assert_eq!(fixture.probe.connections.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture
            .core
            .search_v2(bib_types::SearchRequestV2 {
                query: "Canonical".into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .total,
        1
    );
    service.configure(enabled()).await.unwrap();
    assert_eq!(wait(&service, SemanticPhase::Ready).await.indexed_chunks, 1);
}

fn hybrid(topic: &str) -> bib_types::SearchRequestV2 {
    bib_types::SearchRequestV2 {
        mode: bib_types::SearchMode::Hybrid,
        semantic_query: Some(topic.into()),
        include_articles: true,
        ..Default::default()
    }
}
async fn synchronized(service: &SemanticService, library: &crate::BibBase) {
    let target = library.search_index_status().await.unwrap().source_revision;
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut state = service.state.clone();
        loop {
            let status = state.borrow_and_update().status.clone();
            if status.phase == SemanticPhase::Ready && status.indexed_revision == Some(target) {
                break;
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cleanup_warning_keeps_published_hybrid_search_ready_and_recovers_without_reembedding() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    let service = fixture.service().await;
    service.configure(enabled()).await.unwrap();
    let initial = wait(&service, SemanticPhase::Ready).await;
    let path = fixture
        .path
        .join("vectors.db")
        .to_string_lossy()
        .into_owned();
    let db = turso::Builder::new_local(&path)
        .experimental_multiprocess_wal(true)
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    conn.execute_batch(&format!("CREATE TRIGGER refuseOldCleanup BEFORE DELETE ON semanticBindings WHEN OLD.generation='{}' BEGIN SELECT RAISE(ABORT,'old generation cleanup refused'); END;",initial.generation.unwrap())).await.unwrap();
    fixture.paper("b").await;
    let library = fixture.library(&service).await;
    synchronized(&service, &library).await;
    let status = service.status();
    assert_eq!(status.phase, SemanticPhase::Ready);
    assert!(status.last_error.unwrap().contains("cleanup pending"));
    assert_eq!(status.indexed_chunks, 2);
    let page = library
        .search_v2(hybrid("immune regulation"))
        .await
        .unwrap();
    assert_eq!(page.total, 2);
    let calls = fixture.probe.calls.load(Ordering::SeqCst);
    conn.execute_batch("DROP TRIGGER refuseOldCleanup;")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut state = service.state.clone();
        loop {
            let status = state.borrow_and_update().status.clone();
            if status.phase == SemanticPhase::Ready && status.last_error.is_none() {
                break;
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn hybrid_actor_merges_recall_with_real_evidence_and_candidate_pagination() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    fixture.paper("b").await;
    let service = fixture.service().await;
    let library = fixture.library(&service).await;
    service.configure(enabled()).await.unwrap();
    synchronized(&service, &library).await;
    let mut request = hybrid("xylophone");
    request.page_size = 1;
    let first = library.search_v2(request.clone()).await.unwrap();
    assert_eq!(first.total, 2);
    assert_eq!(first.articles.len(), 1);
    assert_eq!(first.hits[0].article_id, "a");
    assert_eq!(first.hits[0].snippet, "Canonical abstract for a");
    assert!(first.hits[0].highlight_ranges.is_empty());
    let retrieval = first.retrieval.as_ref().unwrap();
    assert_eq!(retrieval.total_kind, "candidate_pool");
    assert_eq!(retrieval.eligible_articles, 2);
    assert_eq!(retrieval.hits.len(), 1);
    let detail = &retrieval.hits["a"];
    assert_eq!(detail.lexical_rank, None);
    assert_eq!(detail.semantic_rank, Some(1));
    let evidence = detail.semantic_evidence.as_ref().unwrap();
    assert_eq!(evidence.snippet, first.hits[0].snippet);
    assert_eq!(Some(evidence.anchor_key.clone()), first.hits[0].anchor_key);
    request.cursor = first.next_cursor.clone();
    let second = library.search_v2(request.clone()).await.unwrap();
    assert_eq!(second.hits[0].article_id, "b");
    assert!(second.next_cursor.is_none());
    assert_eq!(second.index_generation, first.index_generation);
    let both = library.search_v2(hybrid("Canonical")).await.unwrap();
    assert!(
        both.retrieval
            .unwrap()
            .hits
            .values()
            .all(|hit| hit.lexical_rank.is_some() && hit.semantic_rank.is_some())
    );
    let literal = library
        .search_v2(hybrid("NOT unsupportedfield:alpha AND \"beta\""))
        .await
        .unwrap();
    assert_eq!(
        literal.total, 2,
        "topic is not interpreted as query language"
    );
    let page = library
        .search_evidence(
            "a",
            bib_types::SearchEvidenceRequest {
                search: hybrid("xylophone"),
                index_generation: Some(first.index_generation.clone()),
            },
        )
        .await
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.evidence[0].snippet, "Canonical abstract for a");
    assert!(page.evidence[0].highlight_ranges.is_empty());
    assert!(page.evidence[0].semantic_evidence.is_some());
    assert_eq!(page.index_generation, first.index_generation);
    service.configure(enabled()).await.unwrap();
    synchronized(&service, &library).await;
    assert!(
        library.search_v2(request).await.is_err(),
        "configuration changes invalidate cursors even if vectors are reused"
    );
}

#[tokio::test]
async fn hard_boolean_filters_and_collection_scope_apply_before_dense_top_k() {
    use bib_types::{AddedBy, Article, ArticleRole, Collection};
    let fixture = Fixture::new().await;
    let service = fixture.service().await;
    let library = fixture.library(&service).await;
    let mut papers = Vec::new();
    for n in 0..110 {
        let mut paper = Article::new(format!("a{n:03}"), "Excluded document");
        paper.year = Some(2010);
        paper.abstract_text = Some("Known scientific abstract".into());
        papers.push(paper);
    }
    let mut allowed = Article::new("z_allowed", "Allowed document");
    allowed.year = Some(2026);
    allowed.abstract_text = Some("An actual allowed abstract".into());
    papers.push(allowed);
    library.upsert_articles(&papers).await.unwrap();
    library
        .upsert_collection(&Collection::new("only", "Only"))
        .await
        .unwrap();
    library
        .add_to_collection(
            "only",
            "z_allowed",
            ArticleRole::Referenced,
            AddedBy::User,
            None,
        )
        .await
        .unwrap();
    service.configure(enabled()).await.unwrap();
    synchronized(&service, &library).await;
    let unrestricted = library.search_v2(hybrid("xylophone")).await.unwrap();
    assert_eq!(unrestricted.total, 100);
    assert_eq!(unrestricted.retrieval.unwrap().eligible_articles, 111);
    for choice in 0..3 {
        let mut request = hybrid("xylophone");
        request.include_facets = true;
        match choice {
            0 => request.query = "NOT title:Excluded".into(),
            1 => request.filters.year_from = Some(2026),
            _ => request.scope.collection_id = Some("only".into()),
        }
        let page = library.search_v2(request).await.unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.hits[0].article_id, "z_allowed");
        let retrieval = page.retrieval.unwrap();
        assert_eq!(retrieval.eligible_articles, 1);
        assert_eq!(retrieval.hits["z_allowed"].semantic_rank, Some(1));
        assert!(page.facets.is_some());
    }
    let mut impossible = hybrid("xylophone");
    impossible.query = "title:Allowed AND title:Excluded".into();
    assert_eq!(library.search_v2(impossible).await.unwrap().total, 0);
}

#[tokio::test]
async fn stale_source_or_configuration_cannot_publish_a_prepared_query() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    let service = fixture.service().await;
    let library = fixture.library(&service).await;
    service.configure(enabled()).await.unwrap();
    synchronized(&service, &library).await;
    let prepared = service.prepare("xylophone").await.unwrap();
    fixture.paper("b").await;
    let result = library
        .call(|reply| crate::actor::BibCommand::SearchV2 {
            request: hybrid("xylophone"),
            prepared: Some(prepared),
            reply,
        })
        .await;
    assert!(matches!(result, Err(Error::SearchConflict(_))));
    synchronized(&service, &library).await;
    let prepared = service.prepare("xylophone").await.unwrap();
    service.configure(SemanticConfig::default()).await.unwrap();
    let result = library
        .call(|reply| crate::actor::BibCommand::SearchV2 {
            request: hybrid("xylophone"),
            prepared: Some(prepared),
            reply,
        })
        .await;
    assert!(matches!(result, Err(Error::SearchConflict(_))));
    assert_eq!(
        library
            .search_v2(bib_types::SearchRequestV2 {
                query: "Canonical".into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .total,
        2
    );
}

#[tokio::test]
async fn hybrid_saved_rule_export_and_exact_identifier_keep_the_same_hard_constraints() {
    let fixture = Fixture::new().await;
    let service = fixture.service().await;
    let library = fixture.library(&service).await;
    let mut a = bib_types::Article::new("a", "Scientific title");
    a.abstract_text = Some("真实的摘要，包含基因表达证据。".into());
    a.identifiers.push(bib_types::Identifier {
        kind: bib_types::IdKind::Doi,
        value: "10.1000/Exact.ID".into(),
    });
    library.upsert_article(&a).await.unwrap();
    fixture.paper("b").await;
    service.configure(enabled()).await.unwrap();
    synchronized(&service, &library).await;
    let mut request = hybrid("xylophone");
    request.query = "https://doi.org/10.1000/exact.id".into();
    let page = library.search_v2(request.clone()).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.hits[0].article_id, "a");
    assert_eq!(page.hits[0].snippet, "真实的摘要，包含基因表达证据。");
    let rule = library
        .save_search(
            None,
            bib_types::SaveSearchInput {
                name: "A constrained topic".into(),
                request: request.clone(),
                expected_version: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(rule.request.semantic_query, request.semantic_query);
    assert_eq!(
        library
            .export_search(rule.request)
            .await
            .unwrap()
            .iter()
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a"]
    );
    let mut excluded = request;
    excluded.filters.year_from = Some(2020);
    assert!(
        library.export_search(excluded).await.unwrap().is_empty(),
        "unknown year must not bypass a hard filter"
    );
}

#[tokio::test]
async fn a_blocked_topic_embedding_does_not_block_the_lexical_queue() {
    let fixture = Fixture::new().await;
    fixture.paper("a").await;
    let service = fixture.service().await;
    let library = Arc::new(fixture.library(&service).await);
    service.configure(enabled()).await.unwrap();
    synchronized(&service, &library).await;
    fixture.probe.block.store(true, Ordering::SeqCst);
    let task = {
        let library = library.clone();
        tokio::spawn(async move { library.search_v2(hybrid("xylophone")).await })
    };
    tokio::time::timeout(Duration::from_secs(3), fixture.probe.entered.notified())
        .await
        .unwrap();
    let lexical = tokio::time::timeout(
        Duration::from_secs(2),
        library.search_v2(bib_types::SearchRequestV2 {
            query: "Canonical".into(),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(lexical.total, 1);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    fixture.probe.block.store(false, Ordering::SeqCst);
    assert_eq!(
        library.search_v2(hybrid("xylophone")).await.unwrap().total,
        1
    );
}
