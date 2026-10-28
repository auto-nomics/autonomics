//! P5 storage qualification. Format fixtures below are
//! not relevance judgments or evidence of upstream service behavior.
use bib_base::BibBase;
use bib_base::{search_snapshot_exports, search_snapshots};
use bib_types::{Article, SearchRequestV2, discovery::*};
use search_snapshot_exports::SnapshotExportFormat;
use search_snapshots::*;
use std::{collections::BTreeMap, path::PathBuf};

fn execution_command(key: &str, max_items: usize) -> LibraryCaptureCommand {
    LibraryCaptureCommand {
        key: key.into(),
        name: "Executed search".into(),
        request: SearchRequestV2 {
            query: "Study".into(),
            page_size: 2,
            ..Default::default()
        },
        max_items,
    }
}

#[tokio::test]
async fn server_capture_executes_real_pages_and_retry_skips_model_and_search() {
    let db = library().await;
    let store = SnapshotStore::open(":memory:").await.unwrap();
    let saved = capture_library(
        &store,
        execution_command("server.capture", 10),
        || async { Ok(SnapshotRuntime::current(None)) },
        |request| async {
            db.search_v2(request)
                .await
                .map_err(|e| SnapshotError::Conflict(e.to_string()))
        },
    )
    .await
    .unwrap();
    assert_eq!(saved.capture.articles().len(), 3);
    assert!(saved.summary.coverage.complete_obtained_set);
    db.delete_article("a").await.unwrap();
    let retried = capture_library(
        &store,
        execution_command("server.capture", 10),
        || async { panic!("a committed retry must not require a live model") },
        |_| async { panic!("a committed retry must not query current rows") },
    )
    .await
    .unwrap();
    assert_eq!(saved.summary.checksum, retried.summary.checksum);
    assert_eq!(saved.summary.id, retried.summary.id);
}

#[tokio::test]
async fn server_capture_item_budget_preserves_whole_actual_pages() {
    let db = library().await;
    db.upsert_article(&Article::new("d", "Study d"))
        .await
        .unwrap();
    let store = SnapshotStore::open(":memory:").await.unwrap();
    let saved = capture_library(
        &store,
        execution_command("bounded.capture", 3),
        || async { Ok(SnapshotRuntime::current(None)) },
        |request| async {
            db.search_v2(request)
                .await
                .map_err(|e| SnapshotError::Conflict(e.to_string()))
        },
    )
    .await
    .unwrap();
    assert_eq!(saved.summary.coverage.captured_items, 2);
    assert!(!saved.summary.coverage.complete_obtained_set);
    let SnapshotData::Library {
        pages, termination, ..
    } = saved.capture.data
    else {
        panic!()
    };
    assert_eq!(termination, CaptureTermination::Budget);
    assert_eq!(pages.len(), 1);
    assert!(pages[0].response.next_cursor.is_some());
    assert!(pages[0].request.include_articles);
}

#[tokio::test]
async fn server_capture_includes_a_short_final_page_when_the_whole_result_fits() {
    let db = library().await;
    let store = SnapshotStore::open(":memory:").await.unwrap();
    let saved = capture_library(
        &store,
        execution_command("complete.short-tail", 3),
        || async { Ok(SnapshotRuntime::current(None)) },
        |request| async {
            db.search_v2(request)
                .await
                .map_err(|error| SnapshotError::Conflict(error.to_string()))
        },
    )
    .await
    .unwrap();
    assert_eq!(saved.summary.coverage.captured_items, 3);
    assert!(saved.summary.coverage.complete_obtained_set);
    let SnapshotData::Library {
        pages, termination, ..
    } = saved.capture.data
    else {
        panic!()
    };
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[1].response.hits.len(), 1);
    assert_eq!(termination, CaptureTermination::End);
}

#[tokio::test]
async fn server_capture_generation_change_and_cancellation_publish_nothing() {
    let db = library().await;
    let store = SnapshotStore::open(":memory:").await.unwrap();
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let result = capture_library(
        &store,
        execution_command("changed.capture", 10),
        || async { Ok(SnapshotRuntime::current(None)) },
        |request| {
            let call = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let db = &db;
            async move {
                if call == 1 {
                    db.upsert_article(&Article::new("d", "Study d"))
                        .await
                        .unwrap();
                }
                db.search_v2(request)
                    .await
                    .map_err(|e| SnapshotError::Conflict(e.to_string()))
            }
        },
    )
    .await;
    assert!(matches!(result, Err(SnapshotError::Conflict(_))));
    assert!(store.list(100, None).await.unwrap().snapshots.is_empty());
    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(25),
        capture_library(
            &store,
            execution_command("cancelled.capture", 10),
            || async { Ok(SnapshotRuntime::current(None)) },
            |_| std::future::pending::<Result<bib_types::SearchPageV2, SnapshotError>>(),
        ),
    )
    .await;
    assert!(cancelled.is_err());
    assert!(store.list(100, None).await.unwrap().snapshots.is_empty());
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("bib-p5-snapshots-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn db(&self) -> String {
        self.0.join("history.db").to_string_lossy().into_owned()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn library() -> BibBase {
    let db = BibBase::open_in_memory().await.unwrap();
    for id in ["a", "b", "c"] {
        db.upsert_article(&Article::new(id, format!("Study {id}")))
            .await
            .unwrap();
    }
    db
}
async fn capture(db: &BibBase) -> SnapshotCapture {
    let started_at = chrono::Utc::now();
    let mut request = SearchRequestV2 {
        query: "Study".into(),
        include_articles: true,
        page_size: 2,
        ..Default::default()
    };
    let mut pages = Vec::new();
    loop {
        let response = db.search_v2(request.clone()).await.unwrap();
        let next = response.next_cursor.clone();
        pages.push(CapturedLibraryPage {
            request: request.clone(),
            response,
        });
        if next.is_none() {
            break;
        }
        request.cursor = next;
    }
    SnapshotCapture {
        started_at,
        finished_at: chrono::Utc::now(),
        runtime: SnapshotRuntime::current(None),
        data: SnapshotData::Library {
            pages,
            requested_max_items: 10,
            termination: CaptureTermination::End,
        },
    }
}
fn discovery() -> SnapshotCapture {
    let article = Article::new("source-a", "Source format fixture");
    let record = DiscoveryRecord {
        source: "pubmed".into(),
        article: article.clone(),
        source_rank: 1,
        query_fingerprint: "fixture-query".into(),
        retrieved_at: "2026-10-28T00:00:00Z".into(),
        executed_parameters: BTreeMap::from([("term".into(), "fixture".into())]),
        query_translation: Some("fixture translation".into()),
    };
    let candidate = DiscoveryCandidate {
        id: "candidate-a".into(),
        fingerprint: "fixture-identity".into(),
        article,
        records: vec![record],
        conflicts: Vec::new(),
        related_versions: Vec::new(),
        possible_duplicates: Vec::new(),
        rrf_score: 1. / 61.,
        first_seen: 0,
    };
    let source = DiscoverySourceStatus {
        source: "pubmed".into(),
        phase: DiscoverySourcePhase::Failed,
        plan: None,
        failure: Some(SourceFailure::new(
            SourceFailureKind::Transport,
            "fixture source failure",
            true,
        )),
        total: None,
        delivered_records: 1,
        pages: 1,
        has_more: false,
        attempt: 1,
        retry_at: None,
        executed_parameters: BTreeMap::new(),
        query_translation: None,
        notices: Vec::new(),
    };
    let session = DiscoverySessionSnapshot {
        id: "fixture-session".into(),
        request: DiscoveryRequest {
            query: "fixture".into(),
            sources: vec!["pubmed".into()],
            ..Default::default()
        },
        created_at: "2026-10-28T00:00:00Z".into(),
        expires_at: "2026-10-28T00:30:00Z".into(),
        phase: DiscoverySessionPhase::Idle,
        sequence: 3,
        merged_candidates: 1,
        sources: vec![source],
    };
    SnapshotCapture {
        started_at: chrono::Utc::now(),
        finished_at: chrono::Utc::now(),
        runtime: SnapshotRuntime::current(None),
        data: SnapshotData::Discovery {
            session,
            results: DiscoveryResultPage {
                candidates: vec![candidate],
                obtained_candidates: 1,
                offset: 0,
                next_offset: None,
                sequence: 3,
            },
            order: DiscoveryResultOrder::FirstSeen,
            requested_max_items: 10,
            termination: CaptureTermination::Failure,
        },
    }
}

#[tokio::test]
async fn frozen_metadata_survives_live_edits_deletion_and_store_reopen() {
    let dir = Directory::new();
    let db = library().await;
    let data = capture(&db).await;
    let store = SnapshotStore::open(&dir.db()).await.unwrap();
    let saved = store
        .save("intent-1", " My fixed search ", data)
        .await
        .unwrap();
    assert_eq!(saved.summary.name, "My fixed search");
    assert!(saved.summary.coverage.complete_obtained_set);
    assert_eq!(saved.summary.coverage.captured_items, 3);
    db.upsert_article(&Article::new("a", "Changed live title"))
        .await
        .unwrap();
    db.delete_article("b").await.unwrap();
    drop(store);
    let reopened = SnapshotStore::open(&dir.db()).await.unwrap();
    let frozen = reopened.get(&saved.summary.id).await.unwrap().unwrap();
    let articles = frozen.capture.articles();
    assert_eq!(articles.len(), 3);
    assert!(articles.iter().any(|a| a.id == "a" && a.title == "Study a"));
    assert!(articles.iter().any(|a| a.id == "b"));
    assert_eq!(frozen.summary.checksum, saved.summary.checksum);
    let json = serde_json::to_vec(&frozen).unwrap();
    let roundtrip: StoredSnapshot = serde_json::from_slice(&json).unwrap();
    assert_eq!(roundtrip.capture.articles().len(), 3);
}

#[tokio::test]
async fn idempotent_retry_returns_original_capture_after_source_changes_and_budget_shrink() {
    let dir = Directory::new();
    let db = library().await;
    let store = SnapshotStore::open(&dir.db()).await.unwrap();
    let saved = store
        .save("retry", "Original", capture(&db).await)
        .await
        .unwrap();
    db.upsert_article(&Article::new("d", "Study d"))
        .await
        .unwrap();
    let newer = capture(&db).await;
    store
        .configure_limits(SnapshotLimits {
            max_snapshots: 1,
            max_snapshot_bytes: 1,
            max_total_bytes: 1,
        })
        .await
        .unwrap();
    let same = store
        .save("retry", "Original", newer.clone())
        .await
        .unwrap();
    assert_eq!(same.summary.id, saved.summary.id);
    assert_eq!(same.capture.articles().len(), 3);
    assert!(matches!(
        store.save("retry", "Different intent", newer.clone()).await,
        Err(SnapshotError::Conflict(_))
    ));
    assert!(matches!(
        store.save("new", "Original", newer).await,
        Err(SnapshotError::Budget(_))
    ));
    assert_eq!(store.list(100, None).await.unwrap().snapshots.len(), 1);
}

#[tokio::test]
async fn capture_rejects_mixed_generations_bad_cursors_duplicates_and_missing_metadata() {
    let db = library().await;
    let valid = capture(&db).await;
    for mutation in 0..4 {
        let mut bad = valid.clone();
        let SnapshotData::Library { pages, .. } = &mut bad.data else {
            unreachable!()
        };
        match mutation {
            0 => pages[1].response.index_generation.push_str("changed"),
            1 => pages[1].request.cursor = Some("unrelated".into()),
            2 => {
                pages[1].response.hits[0] = pages[0].response.hits[0].clone();
                pages[1].response.articles[0] = pages[0].response.articles[0].clone();
            }
            _ => pages[0].response.articles.clear(),
        };
        assert!(bad.coverage().is_err());
    }
    let mut partial = valid;
    let SnapshotData::Library {
        pages, termination, ..
    } = &mut partial.data
    else {
        unreachable!()
    };
    pages.truncate(1);
    *termination = CaptureTermination::Budget;
    assert!(!partial.coverage().unwrap().complete_obtained_set);
}

#[tokio::test]
async fn discovery_preserves_partial_source_failure_and_rejects_sequence_races() {
    let dir = Directory::new();
    let store = SnapshotStore::open(&dir.db()).await.unwrap();
    let data = discovery();
    let saved = store
        .save("external", "External obtained results", data.clone())
        .await
        .unwrap();
    assert_eq!(saved.summary.kind, SnapshotKind::Discovery);
    assert!(saved.summary.coverage.complete_obtained_set);
    assert_eq!(saved.summary.coverage.sources_exhausted, Some(false));
    let loaded = store.get(&saved.summary.id).await.unwrap().unwrap();
    let SnapshotData::Discovery {
        session, results, ..
    } = loaded.capture.data
    else {
        unreachable!()
    };
    assert_eq!(
        session.sources[0].failure.as_ref().unwrap().message,
        "fixture source failure"
    );
    assert_eq!(
        results.candidates[0].records[0]
            .query_translation
            .as_deref(),
        Some("fixture translation")
    );
    let mut bad = data;
    let SnapshotData::Discovery { results, .. } = &mut bad.data else {
        unreachable!()
    };
    results.sequence += 1;
    assert!(matches!(bad.coverage(), Err(SnapshotError::Conflict(_))));
}

#[tokio::test]
async fn shared_persisted_budget_serializes_competing_writers_and_recovers_after_delete() {
    let dir = Directory::new();
    let a = SnapshotStore::open(&dir.db()).await.unwrap();
    let b = SnapshotStore::open(&dir.db()).await.unwrap();
    let data = discovery();
    a.configure_limits(SnapshotLimits {
        max_snapshots: 1,
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(b.limits().await.unwrap().max_snapshots, 1);
    let (left, right) = tokio::join!(
        a.save("left", "One", data.clone()),
        b.save("right", "Two", data.clone())
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert!(matches!(
        left.as_ref().err().or(right.as_ref().err()),
        Some(SnapshotError::Budget(_))
    ));
    let saved = left.or(right).unwrap();
    assert!(matches!(
        b.delete(&saved.summary.id, &"0".repeat(64)).await,
        Err(SnapshotError::Conflict(_))
    ));
    assert!(
        a.delete(&saved.summary.id, &saved.summary.checksum)
            .await
            .unwrap()
    );
    assert!(
        !a.delete(&saved.summary.id, &saved.summary.checksum)
            .await
            .unwrap()
    );
    b.save("after-delete", "Three", data).await.unwrap();
}

#[tokio::test]
async fn payload_budget_failure_does_not_publish_and_limits_retain_existing_data() {
    let dir = Directory::new();
    let store = SnapshotStore::open(&dir.db()).await.unwrap();
    let saved = store.save("first", "First", discovery()).await.unwrap();
    store
        .configure_limits(SnapshotLimits {
            max_snapshot_bytes: saved.summary.payload_bytes - 1,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(matches!(
        store.save("second", "Second", discovery()).await,
        Err(SnapshotError::Budget(_))
    ));
    assert_eq!(store.list(100, None).await.unwrap().snapshots.len(), 1);
    store
        .configure_limits(SnapshotLimits {
            max_total_bytes: saved.summary.payload_bytes as u64,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(matches!(
        store.save("second", "Second", discovery()).await,
        Err(SnapshotError::Budget(_))
    ));
    assert!(store.get(&saved.summary.id).await.unwrap().is_some());
}

#[tokio::test]
async fn listing_anchors_to_sequence_high_water_and_rejects_other_store_cursor() {
    let dir = Directory::new();
    let store = SnapshotStore::open(&dir.db()).await.unwrap();
    for n in 0..4 {
        store
            .save(&format!("key{n}"), &format!("Snapshot {n}"), discovery())
            .await
            .unwrap();
    }
    let first = store.list(2, None).await.unwrap();
    let cursor = first.next_cursor.clone().unwrap();
    let newest = store.save("later", "Later", discovery()).await.unwrap();
    let second = store.list(2, Some(&cursor)).await.unwrap();
    assert_eq!(second.snapshots.len(), 2);
    assert!(second.next_cursor.is_none());
    assert!(!second.snapshots.iter().any(|s| s.id == newest.summary.id));
    let ids = first
        .snapshots
        .iter()
        .chain(&second.snapshots)
        .map(|s| &s.id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 4);
    let other = SnapshotStore::open(":memory:").await.unwrap();
    assert!(matches!(
        other.list(2, Some(&cursor)).await,
        Err(SnapshotError::Invalid(_))
    ));
    assert!(store.list(0, None).await.is_err());
}

#[tokio::test]
async fn corrupted_payload_is_detected_but_explicit_checked_delete_still_recovers() {
    let dir = Directory::new();
    let store = SnapshotStore::open(&dir.db()).await.unwrap();
    let saved = store.save("first", "First", discovery()).await.unwrap();
    let db = turso::Builder::new_local(&dir.db())
        .experimental_multiprocess_wal(true)
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    conn.execute(
        "UPDATE bibliographySearchSnapshots SET payload=?1 WHERE id=?2",
        turso::params![b"damaged".to_vec(), saved.summary.id.clone()],
    )
    .await
    .unwrap();
    assert!(matches!(
        store.get(&saved.summary.id).await,
        Err(SnapshotError::Corrupt(_))
    ));
    assert_eq!(store.list(10, None).await.unwrap().snapshots.len(), 1);
    assert!(
        store
            .delete(&saved.summary.id, &saved.summary.checksum)
            .await
            .unwrap()
    );
    store
        .save("recovered", "Recovered", discovery())
        .await
        .unwrap();
}

#[tokio::test]
async fn hybrid_capture_requires_the_actual_model_and_matching_page_generations() {
    let db = library().await;
    let mut data = capture(&db).await;
    let SnapshotData::Library { pages, .. } = &mut data.data else {
        unreachable!()
    };
    for page in pages {
        page.request.mode = bib_types::SearchMode::Hybrid;
        page.request.semantic_query = Some("format fixture topic".into());
        page.response.retrieval = Some(bib_types::SearchRetrieval {
            algorithm: "weighted_rrf_v1".into(),
            total_kind: "candidate_pool".into(),
            facets_basis: "hard_query_disjunctive_filters".into(),
            eligible_articles: 3,
            candidate_depth: 100,
            lexical_weight: 1.,
            semantic_weight: 4.,
            rrf_constant: 10,
            semantic_generation: "fixture-generation".into(),
            configuration_generation: "fixture-configuration".into(),
            indexed_chunks: 3,
            hits: BTreeMap::new(),
        });
    }
    assert!(data.coverage().is_err());
    data.runtime.model = Some(SnapshotModel {
        provider: "ollama".into(),
        origin: "http://127.0.0.1:11434".into(),
        name: "format fixture; no inference performed".into(),
        digest: "a".repeat(64),
        signature: "b".repeat(64),
        profile: serde_json::json!({"dimensions":1024}),
        inference: serde_json::json!({}),
        semantic_generation: "fixture-generation".into(),
        configuration_generation: "fixture-configuration".into(),
    });
    assert_eq!(data.coverage().unwrap().result_basis, "candidate_pool");
    data.runtime
        .model
        .as_mut()
        .unwrap()
        .configuration_generation = "different".into();
    assert!(matches!(data.coverage(), Err(SnapshotError::Conflict(_))));
}

#[tokio::test]
async fn recorded_window_metadata_order_and_source_coverage_are_validated() {
    let db = library().await;
    let mut data = capture(&db).await;
    let expected = if let SnapshotData::Library { pages, .. } = &mut data.data {
        let ids = pages
            .iter()
            .flat_map(|page| page.response.hits.iter().map(|hit| hit.article_id.clone()))
            .collect::<Vec<_>>();
        for page in pages {
            page.response.articles.reverse();
        }
        ids
    } else {
        unreachable!()
    };
    assert!(data.coverage().is_ok());
    assert_eq!(
        data.articles()
            .into_iter()
            .map(|article| article.id)
            .collect::<Vec<_>>(),
        expected
    );
    data.started_at = data.finished_at + chrono::Duration::seconds(1);
    assert!(data.coverage().is_err());
    let mut external = discovery();
    let SnapshotData::Discovery { session, .. } = &mut external.data else {
        unreachable!()
    };
    session.request.sources.push("openalex".into());
    assert!(external.coverage().is_err());
}

#[tokio::test]
async fn concurrent_same_intent_is_single_capture_and_metadata_corruption_is_detected() {
    let dir = Directory::new();
    let a = SnapshotStore::open(&dir.db()).await.unwrap();
    let b = SnapshotStore::open(&dir.db()).await.unwrap();
    let first = discovery();
    let mut second = first.clone();
    let SnapshotData::Discovery { results, .. } = &mut second.data else {
        unreachable!()
    };
    results.candidates[0].article.title = "Updated obtained title".into();
    let (left, right) = tokio::join!(
        a.save("same-intent", "Same", first),
        b.save("same-intent", "Same", second)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.summary.id, right.summary.id);
    assert_eq!(left.summary.checksum, right.summary.checksum);
    assert_eq!(a.list(100, None).await.unwrap().snapshots.len(), 1);
    let db = turso::Builder::new_local(&dir.db())
        .experimental_multiprocess_wal(true)
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    conn.execute(
        "UPDATE bibliographySearchSnapshots SET name='changed outside store' WHERE id=?1",
        turso::params![left.summary.id.clone()],
    )
    .await
    .unwrap();
    assert!(matches!(
        a.get(&left.summary.id).await,
        Err(SnapshotError::Corrupt(_))
    ));
}

#[tokio::test]
async fn completed_action_is_recoverable_without_querying_an_expired_source_session() {
    let dir = Directory::new();
    let store = SnapshotStore::open(&dir.db()).await.unwrap();
    let captured = discovery();
    let intent = captured.intent_spec().unwrap();
    assert!(
        store
            .find_for_intent("lost-response", "Saved", &intent)
            .await
            .unwrap()
            .is_none()
    );
    let saved = store
        .save("lost-response", "Saved", captured)
        .await
        .unwrap();
    drop(store);
    let reopened = SnapshotStore::open(&dir.db()).await.unwrap();
    let recovered = reopened
        .find_for_intent("lost-response", "Saved", &intent)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.summary.id, saved.summary.id);
    let mut changed = intent;
    let SnapshotIntent::Discovery { sequence, .. } = &mut changed else {
        unreachable!()
    };
    *sequence += 1;
    assert!(matches!(
        reopened
            .find_for_intent("lost-response", "Saved", &changed)
            .await,
        Err(SnapshotError::Conflict(_))
    ));
}

#[tokio::test]
async fn all_exports_use_frozen_ranked_metadata_and_bibtex_keys_are_unique() {
    let dir = Directory::new();
    let db = library().await;
    let mut article = Article::new("a", "Study original\nER  -\nTY  - JOUR");
    article.abstract_text = Some("Original paragraph.\nER  -\nAnother paragraph.".into());
    db.upsert_article(&article).await.unwrap();
    let store = SnapshotStore::open(&dir.db()).await.unwrap();
    let saved = store
        .save("export", "Frozen exports", capture(&db).await)
        .await
        .unwrap();
    db.upsert_article(&Article::new(
        "a",
        "Current live metadata must not replace export",
    ))
    .await
    .unwrap();
    let saved = store.get(&saved.summary.id).await.unwrap().unwrap();
    let ris = saved
        .export(SnapshotExportFormat::Ris, 1024 * 1024)
        .unwrap();
    assert_eq!(ris.articles, 3);
    assert_eq!(
        ris.content
            .lines()
            .filter(|line| *line == "TY  - JOUR")
            .count(),
        3
    );
    assert_eq!(
        ris.content.lines().filter(|line| *line == "ER  -").count(),
        3
    );
    assert!(ris.content.contains("Study original ER  - TY  - JOUR"));
    assert!(!ris.content.contains("Current live metadata"));
    let bib = saved
        .export(SnapshotExportFormat::Bibtex, 1024 * 1024)
        .unwrap();
    let keys = bib
        .content
        .lines()
        .filter_map(|line| {
            line.strip_prefix("@article{")
                .and_then(|line| line.split(',').next())
        })
        .collect::<Vec<_>>();
    assert_eq!(keys.len(), 3);
    assert_eq!(
        keys.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
    assert!(keys.contains(&"anonndstudy"));
    assert!(keys.contains(&"anonndstudy_2"));
    assert!(keys.contains(&"anonndstudy_3"));
    let json = saved
        .export(SnapshotExportFormat::Json, 1024 * 1024)
        .unwrap();
    let decoded: StoredSnapshot = serde_json::from_str(&json.content).unwrap();
    assert_eq!(decoded.summary.checksum, saved.summary.checksum);
    assert!(
        decoded
            .capture
            .articles()
            .iter()
            .any(|article| article.id == "a" && article.title.contains('\n'))
    );
    assert_eq!(decoded.summary.coverage.result_basis, "complete_matches");
    assert!(json.file_name.ends_with(".json"));
}

#[tokio::test]
async fn export_never_truncates_or_silently_accepts_mutated_captures() {
    let dir = Directory::new();
    let store = SnapshotStore::open(&dir.db()).await.unwrap();
    let saved = store.save("export", "Export", discovery()).await.unwrap();
    for format in [
        SnapshotExportFormat::Json,
        SnapshotExportFormat::Ris,
        SnapshotExportFormat::Bibtex,
    ] {
        assert!(matches!(
            saved.export(format, 1),
            Err(SnapshotError::Budget(_))
        ));
    }
    let mut bad = saved.clone();
    let SnapshotData::Discovery { results, .. } = &mut bad.capture.data else {
        unreachable!()
    };
    results.candidates[0].article.title = "Changed after reading snapshot".into();
    assert!(matches!(
        bad.export(SnapshotExportFormat::Json, 1024 * 1024),
        Err(SnapshotError::Corrupt(_))
    ));
    let mut bad = saved;
    bad.summary.id = "../../wrong-file".into();
    assert!(matches!(
        bad.export(SnapshotExportFormat::Ris, 1024 * 1024),
        Err(SnapshotError::Corrupt(_))
    ));
}

#[tokio::test]
async fn list_anchor_rejects_restored_or_reused_sequence_from_another_history_branch() {
    let dir = Directory::new();
    let path = dir.db();
    let store = SnapshotStore::open(&path).await.unwrap();
    store.save("first", "First", discovery()).await.unwrap();
    let anchor = store.save("second", "Second", discovery()).await.unwrap();
    let cursor = store.list(1, None).await.unwrap().next_cursor.unwrap();
    drop(store);
    // Closed fixture: retain a valid SQL row/payload but replace the top UUID,
    // representing a restored branch that reused the numeric high-water seq.
    let db = turso::Builder::new_local(&path)
        .experimental_multiprocess_wal(true)
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    conn.execute(
        "UPDATE bibliographySearchSnapshots SET id=?1 WHERE id=?2",
        turso::params![uuid::Uuid::new_v4().to_string(), anchor.summary.id],
    )
    .await
    .unwrap();
    drop(conn);
    drop(db);
    let reopened = SnapshotStore::open(&path).await.unwrap();
    assert!(matches!(
        reopened.list(1, Some(&cursor)).await,
        Err(SnapshotError::Conflict(_))
    ));
    assert_eq!(reopened.list(10, None).await.unwrap().snapshots.len(), 2);
}

#[tokio::test]
async fn stored_fractional_scores_survive_read_and_export_without_false_corruption() {
    let dir = Directory::new();
    let store = SnapshotStore::open(&dir.db()).await.unwrap();
    let mut state = 0x9e3779b97f4a7c15u64;
    for index in 0..128 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let score = (state >> 11) as f64 / (1u64 << 53) as f64;
        let mut data = discovery();
        let SnapshotData::Discovery { results, .. } = &mut data.data else {
            unreachable!()
        };
        results.candidates[0].rrf_score = score;
        let saved = store
            .save(&format!("score-{index}"), "Score fidelity", data)
            .await
            .unwrap();
        let reloaded = store.get(&saved.summary.id).await.unwrap().unwrap();
        let exported = reloaded
            .export(SnapshotExportFormat::Json, 1024 * 1024)
            .unwrap_or_else(|error| panic!("valid score {score:?} failed read/export: {error}"));
        let decoded: StoredSnapshot = serde_json::from_str(&exported.content).unwrap();
        let SnapshotData::Discovery { results, .. } = decoded.capture.data else {
            unreachable!()
        };
        assert_eq!(results.candidates[0].rrf_score.to_bits(), score.to_bits());
    }
}
