//! Durable execution ownership tests. No source network calls are made.
use bib_base::{subscription_store::SubscriptionStore, subscriptions::*};
use bib_types::discovery::DiscoveryRequest;
use bib_types::{
    Article, Identifier,
    discovery::{SourcePage, SourcePlan},
};
use std::path::PathBuf;
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("bib-subscription-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn db(&self) -> String {
        self.0.join("library.db").to_string_lossy().into()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn spec() -> SubscriptionSpec {
    SubscriptionSpec {
        name: "Immune surveillance".into(),
        request: DiscoveryRequest {
            query: "immune".into(),
            sources: vec!["pubmed".into()],
            ..Default::default()
        },
        ..Default::default()
    }
}
fn owner() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn plan(s: &SubscriptionSpec) -> bib_types::discovery::SourcePlan {
    bib_base::discovery_plan::plan("pubmed", &s.request).unwrap()
}
fn article(id: &str, doi: &str) -> Article {
    let mut a = Article::new(id, "Actual source title");
    if !doi.is_empty() {
        a.identifiers.push(Identifier::doi(doi));
    }
    a
}
fn page(plan: &SourcePlan, articles: Vec<Article>, next: Option<&str>) -> SourcePage {
    SourcePage {
        source: plan.source.clone(),
        articles,
        total: None,
        next_cursor: next.map(str::to_owned),
        executed_parameters: plan.parameters.clone(),
        query_translation: None,
        notices: plan.notices.clone(),
    }
}

#[tokio::test]
async fn cursor_cycles_and_missing_execution_constraints_do_not_advance_checkpoint() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let s = spec();
    let p = plan(&s);
    let r = store.create(s, 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 60000, true)
        .await
        .unwrap();
    store
        .prepare_source(&lease, "pubmed", 0, p.clone(), None, 1001)
        .await
        .unwrap();
    let a = article("a", "10.1000/a");
    let mut invalid = page(&p, vec![a.clone()], Some("a"));
    invalid.executed_parameters.clear();
    assert!(
        store
            .commit_page(&lease, "pubmed", 1, None, invalid, 1002)
            .await
            .is_err()
    );
    store
        .commit_page(
            &lease,
            "pubmed",
            1,
            None,
            page(&p, vec![a.clone()], Some("a")),
            1002,
        )
        .await
        .unwrap();
    store
        .commit_page(
            &lease,
            "pubmed",
            2,
            Some("a"),
            page(&p, vec![a.clone()], Some("b")),
            1003,
        )
        .await
        .unwrap();
    assert!(
        store
            .commit_page(
                &lease,
                "pubmed",
                3,
                Some("b"),
                page(&p, vec![a], Some("a")),
                1004
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("cursor loop")
    );
    let state = store.get(&r.id).await.unwrap().sources.remove(0);
    assert_eq!(state.sequence, 3);
    assert_eq!(state.cursor.as_deref(), Some("b"));
    assert_eq!(store.inbox(None, 10).await.unwrap().0.len(), 1);
}

#[tokio::test]
async fn safe_identifier_bridge_merges_groups_without_notifying_again() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let mut s = spec();
    s.request.sources = vec!["pubmed".into(), "crossref".into(), "openalex".into()];
    s.request.allow_approximate = true;
    let r = store.create(s.clone(), 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 60000, true)
        .await
        .unwrap();
    let mut b = article("b", "");
    b.identifiers.push(Identifier::pmid("123"));
    let mut bridge = article("bridge", "10.1000/a");
    bridge.identifiers.push(Identifier::pmid("123"));
    for (name, a) in [
        ("pubmed", article("a", "10.1000/a")),
        ("crossref", b),
        ("openalex", bridge),
    ] {
        let p = bib_base::discovery_plan::plan(name, &s.request).unwrap();
        store
            .prepare_source(&lease, name, 0, p.clone(), None, 1001)
            .await
            .unwrap();
        let result = store
            .commit_page(&lease, name, 1, None, page(&p, vec![a], None), 1002)
            .await
            .unwrap();
        assert_eq!(result.new_notifications, usize::from(name != "openalex"));
    }
    let entries = store.inbox(None, 10).await.unwrap().0;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].records.len(), 3);
}

#[tokio::test]
async fn conflicting_identifier_bridge_does_not_choose_a_publication_by_arrival_order() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let mut s = spec();
    s.request.sources = vec!["pubmed".into(), "crossref".into(), "openalex".into()];
    s.request.allow_approximate = true;
    let r = store.create(s.clone(), 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 60000, true)
        .await
        .unwrap();
    let mut a = article("a", "10.1000/a");
    a.identifiers.push(Identifier::pmid("123"));
    let mut b = article("b", "10.1000/b");
    b.identifiers.push(Identifier::pmid("456"));
    let mut bridge = article("bridge", "");
    bridge.identifiers = vec![Identifier::pmid("123"), Identifier::pmid("456")];
    for (name, a) in [("pubmed", a), ("crossref", b), ("openalex", bridge)] {
        let p = bib_base::discovery_plan::plan(name, &s.request).unwrap();
        store
            .prepare_source(&lease, name, 0, p.clone(), None, 1001)
            .await
            .unwrap();
        let result = store
            .commit_page(&lease, name, 1, None, page(&p, vec![a], None), 1002)
            .await
            .unwrap();
        assert_eq!(result.new_notifications, 1);
    }
    let entries = store.inbox(None, 10).await.unwrap().0;
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|i| i.records.len() == 1));
    assert_eq!(
        entries.iter().map(|i| i.related_groups.len()).max(),
        Some(2)
    );
}

#[tokio::test]
async fn independent_rule_commits_cannot_overrun_the_global_seen_quota() {
    let dir = Directory::new();
    let a = SubscriptionStore::open(&dir.db()).await.unwrap();
    let b = SubscriptionStore::open(&dir.db()).await.unwrap();
    let s = spec();
    let p = plan(&s);
    let ra = a.create(s.clone(), 1000).await.unwrap();
    let rb = b.create(s, 1000).await.unwrap();
    let la = a.claim(&ra.id, &owner(), 1000, 60000, true).await.unwrap();
    let lb = b.claim(&rb.id, &owner(), 1000, 60000, true).await.unwrap();
    a.prepare_source(&la, "pubmed", 0, p.clone(), None, 1001)
        .await
        .unwrap();
    b.prepare_source(&lb, "pubmed", 0, p.clone(), None, 1001)
        .await
        .unwrap();
    let mut limits = a.limits().await.unwrap();
    limits.max_seen = 1;
    a.set_limits(limits).await.unwrap();
    let (x, y) = tokio::join!(
        a.commit_page(
            &la,
            "pubmed",
            1,
            None,
            page(&p, vec![article("a", "10.1000/a")], None),
            1002
        ),
        b.commit_page(
            &lb,
            "pubmed",
            1,
            None,
            page(&p, vec![article("b", "10.1000/b")], None),
            1002
        )
    );
    assert!(matches!(
        (x, y),
        (Ok(_), Err(SubscriptionError::Budget(_))) | (Err(SubscriptionError::Budget(_)), Ok(_))
    ));
    assert_eq!(a.inbox(None, 10).await.unwrap().0.len(), 1);
    let x = a.get(&ra.id).await.unwrap().sources.remove(0);
    let y = b.get(&rb.id).await.unwrap().sources.remove(0);
    assert_eq!(usize::from(x.complete) + usize::from(y.complete), 1);
}

#[test]
fn frozen_source_windows_preserve_queries_and_publication_filters() {
    let mut s = spec();
    s.request.query = "(immune OR metabolic) AND year:2020..2024".into();
    let base = plan(&s);
    let window = SubscriptionWindow {
        kind: SubscriptionWindowKind::PubmedCreatedOrRevised,
        from_ms: 86_400_000,
        through_ms: 172_799_999,
    };
    let actual = bib_base::subscription_plan::plan(&s.request, "pubmed", Some(&window)).unwrap();
    assert!(actual.parameters["term"].starts_with(&format!("({}) AND", base.parameters["term"])));
    assert!(actual.parameters["term"].contains("1970/01/02:1970/01/02[crdt]"));
    assert!(actual.parameters["term"].contains("[lr]"));
    assert_ne!(actual.fingerprint, base.fingerprint);
    let invalid = SubscriptionWindow {
        from_ms: 86_400_001,
        ..window
    };
    assert!(bib_base::subscription_plan::plan(&s.request, "pubmed", Some(&invalid)).is_err());
    s.request.sources = vec!["crossref".into()];
    s.request.query = "year:2020..2024".into();
    let base = bib_base::discovery_plan::plan("crossref", &s.request).unwrap();
    let window = SubscriptionWindow {
        kind: SubscriptionWindowKind::CrossrefIndexed,
        from_ms: 86_400_000,
        through_ms: 172_799_999,
    };
    let actual = bib_base::subscription_plan::plan(&s.request, "crossref", Some(&window)).unwrap();
    assert!(actual.parameters["filter"].starts_with(&base.parameters["filter"]));
    assert!(actual.parameters["filter"].contains("from-index-date:1970-01-02T00:00:00"));
    assert!(actual.parameters["filter"].contains("until-index-date:1970-01-02T23:59:59"));
    assert_eq!(actual.parameters["sort"], "indexed");
    assert_eq!(actual.parameters["order"], "asc");
    assert!(
        actual
            .notices
            .iter()
            .any(|n| n.code == "subscription_scan_order")
    );
}

#[tokio::test]
async fn late_strong_identifiers_are_remembered_without_resurrecting_deleted_notifications() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let mut s = spec();
    s.request.sources = vec!["pubmed".into(), "crossref".into()];
    s.request.allow_approximate = true;
    let r = store.create(s.clone(), 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 60000, true)
        .await
        .unwrap();
    let pp = plan(&s);
    let cp = bib_base::discovery_plan::plan("crossref", &s.request).unwrap();
    let mut a = article("a", "");
    a.identifiers.push(Identifier::pmid("123"));
    store
        .prepare_source(&lease, "pubmed", 0, pp.clone(), None, 1001)
        .await
        .unwrap();
    store
        .commit_page(
            &lease,
            "pubmed",
            1,
            None,
            page(&pp, vec![a.clone()], None),
            1002,
        )
        .await
        .unwrap();
    let item = store.inbox(None, 10).await.unwrap().0.pop().unwrap();
    store.delete_inbox(&item.id).await.unwrap();
    a.identifiers.push(Identifier::doi("10.1000/late"));
    store
        .prepare_source(&lease, "pubmed", 2, pp.clone(), None, 1003)
        .await
        .unwrap();
    let enriched = store
        .commit_page(&lease, "pubmed", 3, None, page(&pp, vec![a], None), 1004)
        .await
        .unwrap();
    assert_eq!(enriched.duplicate_records, 1);
    assert_eq!(enriched.new_notifications, 0);
    store
        .prepare_source(&lease, "crossref", 0, cp.clone(), None, 1005)
        .await
        .unwrap();
    let result = store
        .commit_page(
            &lease,
            "crossref",
            1,
            None,
            page(&cp, vec![article("b", "10.1000/late")], None),
            1006,
        )
        .await
        .unwrap();
    assert_eq!(result.new_records, 1);
    assert_eq!(result.new_notifications, 0);
    assert!(store.inbox(None, 10).await.unwrap().0.is_empty());
}

#[tokio::test]
async fn preprint_and_publication_without_shared_doi_remain_related_versions() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let mut s = spec();
    s.request.sources = vec!["arxiv".into(), "pubmed".into()];
    s.request.allow_approximate = true;
    let r = store.create(s.clone(), 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 60000, true)
        .await
        .unwrap();
    let mut preprint = article("preprint", "");
    preprint.source = bib_types::ArticleSource::Arxiv;
    preprint.identifiers.push(Identifier {
        kind: bib_types::IdKind::Arxiv,
        value: "2401.00001v1".into(),
    });
    let mut published = article("publication", "");
    published.source = bib_types::ArticleSource::Pubmed;
    published.identifiers.push(Identifier {
        kind: bib_types::IdKind::Arxiv,
        value: "2401.00001".into(),
    });
    for (name, a) in [("arxiv", preprint), ("pubmed", published)] {
        let p = bib_base::discovery_plan::plan(name, &s.request).unwrap();
        store
            .prepare_source(&lease, name, 0, p.clone(), None, 1001)
            .await
            .unwrap();
        let result = store
            .commit_page(&lease, name, 1, None, page(&p, vec![a], None), 1002)
            .await
            .unwrap();
        assert_eq!(result.new_notifications, 1);
    }
    let entries = store.inbox(None, 10).await.unwrap().0;
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|item| item.related_groups.len() == 1));
}

#[tokio::test]
async fn identity_enrichment_preserves_new_preprint_evidence_for_future_matches() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let mut s = spec();
    s.request.sources = vec!["crossref".into(), "pubmed".into()];
    s.request.allow_approximate = true;
    let r = store.create(s.clone(), 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 60000, true)
        .await
        .unwrap();
    let cp = bib_base::discovery_plan::plan("crossref", &s.request).unwrap();
    let pp = plan(&s);
    let mut a = article("preprint-source", "");
    a.source = bib_types::ArticleSource::CrossRef;
    a.identifiers.push(Identifier::pmid("123"));
    store
        .prepare_source(&lease, "crossref", 0, cp.clone(), None, 1001)
        .await
        .unwrap();
    store
        .commit_page(
            &lease,
            "crossref",
            1,
            None,
            page(&cp, vec![a.clone()], None),
            1002,
        )
        .await
        .unwrap();
    a.pub_types.push("preprint".into());
    store
        .prepare_source(&lease, "crossref", 2, cp.clone(), None, 1003)
        .await
        .unwrap();
    store
        .commit_page(&lease, "crossref", 3, None, page(&cp, vec![a], None), 1004)
        .await
        .unwrap();
    let mut b = article("publication", "");
    b.source = bib_types::ArticleSource::Pubmed;
    b.identifiers.push(Identifier::pmid("123"));
    store
        .prepare_source(&lease, "pubmed", 0, pp.clone(), None, 1005)
        .await
        .unwrap();
    let result = store
        .commit_page(&lease, "pubmed", 1, None, page(&pp, vec![b], None), 1006)
        .await
        .unwrap();
    assert_eq!(result.new_notifications, 1);
    let items = store.inbox(None, 10).await.unwrap().0;
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|item| item.related_groups.len() == 1));
}

#[tokio::test]
async fn atomic_page_retry_and_deleted_notifications_do_not_forget_seen_identity() {
    let dir = Directory::new();
    let store = SubscriptionStore::open(&dir.db()).await.unwrap();
    let s = spec();
    let p = plan(&s);
    let r = store.create(s, 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 60000, true)
        .await
        .unwrap();
    store
        .prepare_source(&lease, "pubmed", 0, p.clone(), None, 1001)
        .await
        .unwrap();
    let a = article("a", "10.1000/a");
    let first = page(&p, vec![a.clone()], Some("provider-cursor-1"));
    let receipt = store
        .commit_page(&lease, "pubmed", 1, None, first.clone(), 1002)
        .await
        .unwrap();
    assert_eq!(receipt.new_records, 1);
    assert_eq!(receipt.new_notifications, 1);
    let retry = store
        .commit_page(&lease, "pubmed", 1, None, first, 1003)
        .await
        .unwrap();
    assert_eq!(retry.sequence, receipt.sequence);
    let entries = store.inbox(None, 10).await.unwrap().0;
    assert_eq!(entries.len(), 1);
    store.delete_inbox(&entries[0].id).await.unwrap();
    let second = page(&p, vec![a.clone(), article("b", "10.1000/b")], None);
    assert!(
        store
            .commit_page(&lease, "pubmed", 2, Some("wrong"), second.clone(), 1004)
            .await
            .is_err()
    );
    let result = store
        .commit_page(&lease, "pubmed", 2, Some("provider-cursor-1"), second, 1004)
        .await
        .unwrap();
    assert_eq!(
        (
            result.new_records,
            result.duplicate_records,
            result.new_notifications
        ),
        (1, 1, 1)
    );
    let b = store.inbox(None, 10).await.unwrap().0.pop().unwrap();
    store.set_inbox_read(&b.id, true).await.unwrap();
    drop(store);
    let store = SubscriptionStore::open(&dir.db()).await.unwrap();
    store
        .prepare_source(&lease, "pubmed", 3, p.clone(), None, 1005)
        .await
        .unwrap();
    let replay = store
        .commit_page(
            &lease,
            "pubmed",
            4,
            None,
            page(&p, vec![a, article("b", "10.1000/b")], None),
            1006,
        )
        .await
        .unwrap();
    assert_eq!(replay.new_notifications, 0);
    assert_eq!(replay.duplicate_records, 2);
    let retained = store.inbox(None, 10).await.unwrap().0;
    assert_eq!(retained.len(), 1);
    assert!(retained[0].read);
}

#[tokio::test]
async fn page_quota_failure_rolls_back_every_identity_notification_and_checkpoint() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let s = spec();
    let p = plan(&s);
    let r = store.create(s, 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 60000, true)
        .await
        .unwrap();
    store
        .prepare_source(&lease, "pubmed", 0, p.clone(), None, 1001)
        .await
        .unwrap();
    let mut policy = store.limits().await.unwrap();
    policy.max_seen = 1;
    store.set_limits(policy.clone()).await.unwrap();
    let input = page(
        &p,
        vec![article("a", "10.1000/a"), article("b", "10.1000/b")],
        None,
    );
    assert!(matches!(
        store
            .commit_page(&lease, "pubmed", 1, None, input.clone(), 1002)
            .await,
        Err(SubscriptionError::Budget(_))
    ));
    assert!(store.inbox(None, 10).await.unwrap().0.is_empty());
    let state = store.get(&r.id).await.unwrap().sources.remove(0);
    assert_eq!(state.sequence, 1);
    assert!(!state.complete);
    assert!(state.last_page.is_none());
    policy.max_seen = 10;
    store.set_limits(policy).await.unwrap();
    let result = store
        .commit_page(&lease, "pubmed", 1, None, input, 1003)
        .await
        .unwrap();
    assert_eq!(result.new_records, 2);
    assert_eq!(result.new_notifications, 2);
    let mut policy = store.limits().await.unwrap();
    policy.max_page_bytes = 1;
    store.set_limits(policy).await.unwrap();
    let entries = store.inbox(None, 10).await.unwrap().0;
    store.set_inbox_read(&entries[0].id, true).await.unwrap();
    assert_eq!(store.inbox(None, 10).await.unwrap().0.len(), 2);
}

#[tokio::test]
async fn a_late_checkpoint_failure_rolls_back_already_written_inbox_and_seen_rows() {
    let dir = Directory::new();
    let store = SubscriptionStore::open(&dir.db()).await.unwrap();
    let s = spec();
    let p = plan(&s);
    let r = store.create(s, 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 60000, true)
        .await
        .unwrap();
    store
        .prepare_source(&lease, "pubmed", 0, p.clone(), None, 1001)
        .await
        .unwrap();
    let db = turso::Builder::new_local(&dir.db())
        .experimental_multiprocess_wal(true)
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    conn.execute_batch("CREATE TRIGGER failCheckpoint BEFORE UPDATE ON bibliographySubscriptionSources BEGIN SELECT RAISE(ABORT,'checkpoint fault'); END;").await.unwrap();
    let input = page(&p, vec![article("a", "10.1000/a")], None);
    assert!(
        store
            .commit_page(&lease, "pubmed", 1, None, input.clone(), 1002)
            .await
            .unwrap_err()
            .to_string()
            .contains("checkpoint fault")
    );
    assert!(store.inbox(None, 10).await.unwrap().0.is_empty());
    conn.execute_batch("DROP TRIGGER failCheckpoint;")
        .await
        .unwrap();
    let result = store
        .commit_page(&lease, "pubmed", 1, None, input, 1003)
        .await
        .unwrap();
    assert_eq!(result.new_records, 1);
    assert_eq!(result.new_notifications, 1);
}

#[tokio::test]
async fn invalidating_ownership_during_checkpoint_write_prevents_a_late_commit() {
    let dir = Directory::new();
    let store = SubscriptionStore::open(&dir.db()).await.unwrap();
    let s = spec();
    let p = plan(&s);
    let r = store.create(s, 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 60000, true)
        .await
        .unwrap();
    store
        .prepare_source(&lease, "pubmed", 0, p.clone(), None, 1001)
        .await
        .unwrap();
    let db = turso::Builder::new_local(&dir.db())
        .experimental_multiprocess_wal(true)
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    conn.execute_batch("CREATE TRIGGER invalidateLease AFTER UPDATE ON bibliographySubscriptionSources BEGIN UPDATE bibliographySubscriptions SET leaseUntil=0 WHERE id=NEW.ruleID; END;").await.unwrap();
    let input = page(&p, vec![article("a", "10.1000/a")], None);
    assert!(matches!(
        store
            .commit_page(&lease, "pubmed", 1, None, input.clone(), 1002)
            .await,
        Err(SubscriptionError::Conflict(_))
    ));
    assert!(store.inbox(None, 10).await.unwrap().0.is_empty());
    assert_eq!(store.get(&r.id).await.unwrap().sources[0].sequence, 1);
    conn.execute_batch("DROP TRIGGER invalidateLease;")
        .await
        .unwrap();
    let retried = store
        .commit_page(&lease, "pubmed", 1, None, input, 1003)
        .await
        .unwrap();
    assert_eq!(retried.new_records, 1);
    assert_eq!(retried.new_notifications, 1);
}

#[tokio::test]
async fn retry_after_and_cursor_replay_survive_reopen_without_losing_seen_state() {
    use bib_types::discovery::{SourceFailure, SourceFailureKind};
    let dir = Directory::new();
    let store = SubscriptionStore::open(&dir.db()).await.unwrap();
    let s = spec();
    let p = plan(&s);
    let r = store.create(s, 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 60000, true)
        .await
        .unwrap();
    store
        .prepare_source(&lease, "pubmed", 0, p.clone(), None, 1001)
        .await
        .unwrap();
    let a = article("a", "10.1000/a");
    store
        .commit_page(
            &lease,
            "pubmed",
            1,
            None,
            page(&p, vec![a.clone()], Some("cursor-a")),
            1002,
        )
        .await
        .unwrap();
    let mut error = SourceFailure::new(
        SourceFailureKind::RateLimited,
        "source returned HTTP 429",
        true,
    );
    error.retry_after_seconds = Some(60);
    let state = store
        .record_failure(&lease, "pubmed", 2, error, Some(61003), false, 1003)
        .await
        .unwrap();
    assert_eq!(state.cursor.as_deref(), Some("cursor-a"));
    assert_eq!(state.retry_at_ms, Some(61003));
    assert!(state.watermark_ms.is_none());
    drop(store);
    let store = SubscriptionStore::open(&dir.db()).await.unwrap();
    let state = store.get(&r.id).await.unwrap().sources.remove(0);
    assert_eq!(state.retry_at_ms, Some(61003));
    assert_eq!(
        state.last_error.as_ref().unwrap().retry_after_seconds,
        Some(60)
    );
    let replay = store
        .record_failure(
            &lease,
            "pubmed",
            3,
            SourceFailure::new(
                SourceFailureKind::CursorExpired,
                "source history expired",
                false,
            ),
            None,
            true,
            1004,
        )
        .await
        .unwrap();
    assert_eq!(replay.plan, Some(p.clone()));
    assert!(replay.cursor.is_none());
    assert_eq!(replay.scanned_records, 0);
    let result = store
        .commit_page(&lease, "pubmed", 4, None, page(&p, vec![a], None), 1005)
        .await
        .unwrap();
    assert_eq!(result.new_notifications, 0);
    assert_eq!(result.duplicate_records, 1);
    assert_eq!(store.inbox(None, 10).await.unwrap().0.len(), 1);
}

#[tokio::test]
async fn source_watermark_advances_only_after_the_frozen_window_is_exhausted() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let s = spec();
    let window = SubscriptionWindow {
        kind: SubscriptionWindowKind::PubmedCreatedOrRevised,
        from_ms: 0,
        through_ms: 86_399_999,
    };
    let p = bib_base::subscription_plan::plan(&s.request, "pubmed", Some(&window)).unwrap();
    let r = store.create(s, 86401000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 86401000, 60000, true)
        .await
        .unwrap();

    store
        .prepare_source(
            &lease,
            "pubmed",
            0,
            p.clone(),
            Some(window.clone()),
            86401001,
        )
        .await
        .unwrap();
    store
        .commit_page(
            &lease,
            "pubmed",
            1,
            None,
            page(&p, vec![article("a", "10.86401000/a")], Some("cursor")),
            86401002,
        )
        .await
        .unwrap();
    let partial = store.get(&r.id).await.unwrap();
    assert!(partial.sources[0].watermark_ms.is_none());
    assert!(partial.last_success_ms.is_none());
    store.release(&lease, 86401003, 0).await.unwrap();
    let next = store
        .claim(&r.id, &owner(), 86401004, 60000, true)
        .await
        .unwrap();
    let persisted = store.get(&r.id).await.unwrap().sources.remove(0);
    assert_eq!(persisted.window, Some(window));
    assert_eq!(persisted.cursor.as_deref(), Some("cursor"));
    assert!(
        store
            .commit_page(
                &lease,
                "pubmed",
                2,
                Some("cursor"),
                page(&p, vec![], None),
                86401005
            )
            .await
            .is_err()
    );
    store
        .commit_page(
            &next,
            "pubmed",
            2,
            Some("cursor"),
            page(&p, vec![], None),
            86401005,
        )
        .await
        .unwrap();
    let done = store.get(&r.id).await.unwrap();
    assert_eq!(done.sources[0].watermark_ms, Some(86_399_999));
    assert_eq!(done.sources[0].completed_epoch, next.epoch);
    assert!(
        done.last_success_ms.is_none(),
        "a source is not a complete multi-source round"
    );
}

#[tokio::test]
async fn cross_source_doi_matches_merge_but_conflicting_identities_create_review_links() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let mut s = spec();
    s.request.sources = vec!["pubmed".into(), "crossref".into()];
    s.request.allow_approximate = true;
    let r = store.create(s.clone(), 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 60000, true)
        .await
        .unwrap();
    let pp = plan(&s);
    let cp = bib_base::discovery_plan::plan("crossref", &s.request).unwrap();
    let mut a = article("pubmed-a", "10.1000/same");
    a.identifiers.push(Identifier::pmid("123"));
    store
        .prepare_source(&lease, "pubmed", 0, pp.clone(), None, 1001)
        .await
        .unwrap();
    store
        .commit_page(&lease, "pubmed", 1, None, page(&pp, vec![a], None), 1002)
        .await
        .unwrap();
    store
        .prepare_source(&lease, "crossref", 0, cp.clone(), None, 1003)
        .await
        .unwrap();
    let mut c = article("crossref-conflict", "10.1000/different");
    c.identifiers.push(Identifier::pmid("123"));
    let result = store
        .commit_page(
            &lease,
            "crossref",
            1,
            None,
            page(&cp, vec![article("crossref-same", "10.1000/same"), c], None),
            1004,
        )
        .await
        .unwrap();
    assert_eq!(result.new_records, 2);
    assert_eq!(result.new_notifications, 1);
    let entries = store.inbox(None, 10).await.unwrap().0;
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|i| i.related_groups.len() == 1));
    assert!(entries.iter().any(|i| i.records.len() == 2));
}

#[tokio::test]
async fn creation_requires_explicit_network_intent_and_survives_reopen() {
    let dir = Directory::new();
    let store = SubscriptionStore::open(&dir.db()).await.unwrap();
    let s = spec();
    let r = store.create(s.clone(), 1000).await.unwrap();
    assert!(!r.spec.enabled);
    assert!(r.last_success_ms.is_none());
    assert!(r.sources[0].watermark_ms.is_none());
    assert!(matches!(
        store.claim(&r.id, &owner(), 1000, 5000, false).await,
        Err(SubscriptionError::Conflict(_))
    ));
    let lease = store
        .claim(&r.id, &owner(), 1000, 5000, true)
        .await
        .unwrap();
    let state = store
        .prepare_source(&lease, "pubmed", 0, plan(&s), None, 1001)
        .await
        .unwrap();
    assert!(!state.complete);
    assert_eq!(state.sequence, 1);
    drop(store);
    let reopened = SubscriptionStore::open(&dir.db()).await.unwrap();
    let r = reopened.get(&r.id).await.unwrap();
    assert_eq!(r.sources[0].plan, Some(plan(&s)));
    assert_eq!(r.sources[0].sequence, 1);
    assert!(!r.sources[0].complete);
    assert!(r.last_success_ms.is_none());
    assert_eq!(reopened.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn independent_connections_compete_for_one_lease_and_expired_owner_is_fenced() {
    let dir = Directory::new();
    let a = SubscriptionStore::open(&dir.db()).await.unwrap();
    let b = SubscriptionStore::open(&dir.db()).await.unwrap();
    let mut s = spec();
    s.enabled = true;
    let r = a.create(s.clone(), 1000).await.unwrap();
    let oa = owner();
    let ob = owner();
    let (x, y) = tokio::join!(
        a.claim(&r.id, &oa, 1000, 1000, false),
        b.claim(&r.id, &ob, 1000, 1000, false)
    );
    let old = match (x, y) {
        (Ok(x), Err(SubscriptionError::Conflict(_))) => x,
        (Err(SubscriptionError::Conflict(_)), Ok(y)) => y,
        other => panic!("expected one owner: {other:?}"),
    };
    let expired = old.expires_ms;
    assert!(
        a.heartbeat(&old, expired, 1000).await.is_err(),
        "expiry cannot be revived"
    );
    let new = b
        .claim(&r.id, &owner(), expired, 5000, false)
        .await
        .unwrap();
    assert!(new.epoch > old.epoch);
    assert!(
        a.prepare_source(&old, "pubmed", 0, plan(&s), None, expired + 1)
            .await
            .is_err()
    );
    b.prepare_source(&new, "pubmed", 0, plan(&s), None, expired + 1)
        .await
        .unwrap();
    assert!(a.release(&old, expired + 1, 0).await.is_err());
    let renewed = b.heartbeat(&new, expired + 100, 6000).await.unwrap();
    assert!(renewed.expires_ms >= expired + 6100);
}

#[tokio::test]
async fn pause_and_resume_preserve_window_and_fence_old_executions() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let s = spec();
    let r = store.create(s.clone(), 1000).await.unwrap();
    let old = store
        .claim(&r.id, &owner(), 1000, 5000, true)
        .await
        .unwrap();
    let before = store
        .prepare_source(&old, "pubmed", 0, plan(&s), None, 1001)
        .await
        .unwrap();
    let paused = store.set_enabled(&r.id, 1, false, 1100).await.unwrap();
    assert_eq!(paused.version, 2);
    assert_eq!(paused.sources[0].plan, before.plan);
    assert_eq!(paused.sources[0].sequence, before.sequence);
    assert!(store.heartbeat(&old, 1101, 5000).await.is_err());
    assert!(
        store
            .claim(&r.id, &owner(), 1101, 5000, false)
            .await
            .is_err()
    );
    let resumed = store.set_enabled(&r.id, 2, true, 1200).await.unwrap();
    assert_eq!(resumed.version, 3);
    let next = store
        .claim(&r.id, &owner(), 1200, 5000, false)
        .await
        .unwrap();
    assert!(
        store
            .prepare_source(&next, "pubmed", before.sequence, plan(&s), None, 1201)
            .await
            .is_err(),
        "unfinished plan cannot be overwritten"
    );
    store.release(&next, 1201, 1000).await.unwrap();
    assert!(
        store
            .claim(&r.id, &owner(), 1202, 5000, false)
            .await
            .is_err(),
        "not yet due"
    );
    assert!(store.get(&r.id).await.unwrap().last_success_ms.is_none());
}

#[tokio::test]
async fn configuration_replacement_and_deletion_require_versions_and_cancel_leases() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let s = spec();
    let r = store.create(s.clone(), 1000).await.unwrap();
    let old = store
        .claim(&r.id, &owner(), 1000, 5000, true)
        .await
        .unwrap();
    store
        .prepare_source(&old, "pubmed", 0, plan(&s), None, 1001)
        .await
        .unwrap();
    let mut changed = s;
    changed.request.query = "metabolic".into();
    assert!(
        store
            .replace(&r.id, 2, changed.clone(), 1100)
            .await
            .is_err()
    );
    let new = store.replace(&r.id, 1, changed, 1100).await.unwrap();
    assert_eq!(new.version, 2);
    assert!(new.sources[0].plan.is_none());
    assert!(new.sources[0].watermark_ms.is_none());
    assert!(store.heartbeat(&old, 1101, 5000).await.is_err());
    assert!(store.delete(&r.id, 1).await.is_err());
    store.delete(&r.id, 2).await.unwrap();
    assert!(store.list().await.unwrap().is_empty());
    assert!(matches!(
        store.get(&r.id).await,
        Err(SubscriptionError::NotFound)
    ));
}

#[tokio::test]
async fn persisted_quota_is_shared_and_lowering_it_does_not_evict_rules() {
    let dir = Directory::new();
    let a = SubscriptionStore::open(&dir.db()).await.unwrap();
    let b = SubscriptionStore::open(&dir.db()).await.unwrap();
    let mut limits = a.limits().await.unwrap();
    limits.max_rules = 2;
    a.set_limits(limits.clone()).await.unwrap();
    a.create(spec(), 1000).await.unwrap();
    b.create(spec(), 1000).await.unwrap();
    limits.max_rules = 1;
    b.set_limits(limits).await.unwrap();
    assert_eq!(a.list().await.unwrap().len(), 2);
    assert!(matches!(
        a.create(spec(), 1000).await,
        Err(SubscriptionError::Budget(_))
    ));
    assert_eq!(b.limits().await.unwrap().max_rules, 1);
}

#[tokio::test]
async fn invalid_windows_and_checkpoint_sequences_do_not_modify_state() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let s = spec();
    let r = store.create(s.clone(), 1000).await.unwrap();
    let lease = store
        .claim(&r.id, &owner(), 1000, 5000, true)
        .await
        .unwrap();
    let window = SubscriptionWindow {
        kind: SubscriptionWindowKind::CrossrefIndexed,
        from_ms: 0,
        through_ms: 999,
    };
    assert!(
        store
            .prepare_source(&lease, "pubmed", 0, plan(&s), Some(window), 1001)
            .await
            .is_err()
    );
    assert!(
        store
            .prepare_source(&lease, "pubmed", 1, plan(&s), None, 1001)
            .await
            .is_err()
    );
    assert_eq!(store.get(&r.id).await.unwrap().sources[0].sequence, 0);
    store
        .prepare_source(&lease, "pubmed", 0, plan(&s), None, 1001)
        .await
        .unwrap();
    assert!(
        store
            .prepare_source(&lease, "pubmed", 1, plan(&s), None, 1001)
            .await
            .is_err()
    );
    assert_eq!(store.get(&r.id).await.unwrap().sources[0].sequence, 1);
}
