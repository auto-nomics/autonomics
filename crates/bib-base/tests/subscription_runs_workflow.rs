//! Persistent execution budgets, history and source-scan recovery; no network.
use bib_base::{subscription_store::SubscriptionStore, subscriptions::*};
use bib_types::{
    Article, Identifier,
    discovery::{DiscoveryRequest, SourceFailure, SourceFailureKind, SourcePage},
};
const DAY: i64 = 86_400_000;
const NOW: i64 = 20_000 * DAY + DAY / 2;
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn spec(sources: &[&str]) -> SubscriptionSpec {
    SubscriptionSpec {
        name: "Tracked immune query".into(),
        request: DiscoveryRequest {
            query: "immune".into(),
            sources: sources.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        },
        ..Default::default()
    }
}
fn page(state: &SubscriptionSourceState, ids: &[&str], next: Option<&str>) -> SourcePage {
    SourcePage {
        source: state.source.clone(),
        articles: ids
            .iter()
            .map(|id| {
                let mut a = Article::new(*id, "Source metadata");
                a.identifiers.push(Identifier::doi(format!("10.1000/{id}")));
                a
            })
            .collect(),
        total: None,
        next_cursor: next.map(str::to_owned),
        executed_parameters: state.plan.as_ref().unwrap().parameters.clone(),
        query_translation: None,
        notices: Vec::new(),
    }
}
async fn begin(
    store: &SubscriptionStore,
    rule: &str,
    now: i64,
    manual: bool,
) -> (SubscriptionLease, SubscriptionRun) {
    let lease = store.claim(rule, &id(), now, 60000, manual).await.unwrap();
    store.begin_run(&lease, &id(), manual, now).await.unwrap()
}
async fn commit(
    store: &SubscriptionStore,
    lease: &SubscriptionLease,
    state: &SubscriptionSourceState,
    ids: &[&str],
    next: Option<&str>,
    now: i64,
) {
    store
        .reserve_run_page(lease, &state.source, state.sequence, now)
        .await
        .unwrap();
    store
        .commit_page(
            lease,
            &state.source,
            state.sequence,
            state.cursor.as_deref(),
            page(state, ids, next),
            now + 1,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn whole_page_budget_ends_a_run_and_the_next_run_resumes_its_cursor() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let mut s = spec(&["openalex"]);
    s.max_pages_per_run = 1;
    s.max_records_per_run = 25;
    let rule = store.create(s, NOW).await.unwrap();
    let (lease, first) = begin(&store, &rule.id, NOW, true).await;
    let state = store
        .prepare_run_source(&lease, "openalex", NOW + 1)
        .await
        .unwrap()
        .unwrap();
    let scan = state.scan.as_ref().unwrap().id.clone();
    commit(&store, &lease, &state, &["a"], Some("next"), NOW + 2).await;
    let pending = store.get(&rule.id).await.unwrap().sources.remove(0);
    assert!(matches!(
        store
            .reserve_run_page(&lease, "openalex", pending.sequence, NOW + 4)
            .await,
        Err(SubscriptionError::RunBudget(_))
    ));
    let ended = store
        .finish_run(
            &lease,
            SubscriptionRunState::Partial,
            Some("page budget".into()),
            NOW + 5,
        )
        .await
        .unwrap();
    assert_eq!(ended.pages_started, 1);
    assert_eq!(ended.pages_committed, 1);
    assert_eq!(ended.records, 1);
    assert!(store.get(&rule.id).await.unwrap().last_success_ms.is_none());
    let (next, second) = begin(&store, &rule.id, NOW + 6, true).await;
    let state = store
        .prepare_run_source(&next, "openalex", NOW + 7)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.cursor.as_deref(), Some("next"));
    assert_eq!(state.scan.as_ref().unwrap().id, scan);
    commit(&store, &next, &state, &["b"], None, NOW + 8).await;
    let ended = store
        .finish_run(&next, SubscriptionRunState::Complete, None, NOW + 10)
        .await
        .unwrap();
    assert!(ended.sources[0].done);
    assert_eq!(ended.new_notifications, 1);
    assert_eq!(
        store.get(&rule.id).await.unwrap().last_success_ms,
        Some(NOW + 10)
    );
    let (newer, cursor) = store.runs(&rule.id, None, 1).await.unwrap();
    assert_eq!(newer[0].id, second.id);
    let (older, end) = store.runs(&rule.id, cursor, 1).await.unwrap();
    assert_eq!(older[0].id, first.id);
    assert!(end.is_none());
    assert_eq!(
        store.run(&first.id).await.unwrap().state,
        SubscriptionRunState::Partial
    );
}

#[tokio::test]
async fn a_page_must_be_reserved_and_failed_requests_consume_page_budget() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let mut s = spec(&["openalex"]);
    s.max_pages_per_run = 1;
    let rule = store.create(s, NOW).await.unwrap();
    let (lease, run) = begin(&store, &rule.id, NOW, true).await;
    let state = store
        .prepare_run_source(&lease, "openalex", NOW + 1)
        .await
        .unwrap()
        .unwrap();
    assert!(
        store
            .commit_page(
                &lease,
                "openalex",
                state.sequence,
                None,
                page(&state, &["a"], None),
                NOW + 2
            )
            .await
            .is_err()
    );
    assert!(store.inbox(None, 10).await.unwrap().0.is_empty());
    store
        .reserve_run_page(&lease, "openalex", state.sequence, NOW + 3)
        .await
        .unwrap();
    assert!(
        store
            .reserve_run_page(&lease, "openalex", state.sequence, NOW + 4)
            .await
            .is_err()
    );
    store
        .record_failure(
            &lease,
            "openalex",
            state.sequence,
            SourceFailure::new(SourceFailureKind::Transport, "request failed", true),
            None,
            false,
            NOW + 5,
        )
        .await
        .unwrap();
    let state = store.get(&rule.id).await.unwrap().sources.remove(0);
    assert!(matches!(
        store
            .reserve_run_page(&lease, "openalex", state.sequence, NOW + 6)
            .await,
        Err(SubscriptionError::RunBudget(_))
    ));
    let saved = store.run(&run.id).await.unwrap();
    assert_eq!(saved.pages_started, 1);
    assert_eq!(saved.pages_committed, 0);
    assert_eq!(saved.reserved_records, 0);
    assert!(saved.sources[0].error.is_some());
    assert!(
        store
            .finish_run(&lease, SubscriptionRunState::Complete, None, NOW + 7)
            .await
            .is_err()
    );
    store
        .finish_run(&lease, SubscriptionRunState::Partial, None, NOW + 7)
        .await
        .unwrap();
}

#[tokio::test]
async fn pause_closes_the_attempt_and_a_resumed_owner_keeps_the_source_scan() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let rule = store.create(spec(&["openalex"]), NOW).await.unwrap();
    let (old, run) = begin(&store, &rule.id, NOW, true).await;
    let state = store
        .prepare_run_source(&old, "openalex", NOW + 1)
        .await
        .unwrap()
        .unwrap();
    store
        .reserve_run_page(&old, "openalex", state.sequence, NOW + 2)
        .await
        .unwrap();
    let paused = store
        .set_enabled(&rule.id, 1, false, NOW + 3)
        .await
        .unwrap();
    assert_eq!(
        store.run(&run.id).await.unwrap().state,
        SubscriptionRunState::Cancelled
    );
    assert!(
        store
            .commit_page(
                &old,
                "openalex",
                state.sequence,
                None,
                page(&state, &["late"], None),
                NOW + 4
            )
            .await
            .is_err()
    );
    store
        .set_enabled(&rule.id, paused.version, true, NOW + 5)
        .await
        .unwrap();
    let (next, _) = begin(&store, &rule.id, NOW + 6, false).await;
    let resumed = store
        .prepare_run_source(&next, "openalex", NOW + 7)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.scan.as_ref().unwrap().id, state.scan.unwrap().id);
    commit(&store, &next, &resumed, &["a"], None, NOW + 8).await;
    store
        .finish_run(&next, SubscriptionRunState::Complete, None, NOW + 10)
        .await
        .unwrap();
    assert_eq!(store.inbox(None, 10).await.unwrap().0.len(), 1);
}

#[tokio::test]
async fn overlap_longer_than_a_subwindow_advances_across_budgeted_attempts() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let mut s = spec(&["pubmed"]);
    s.initial_days = 1;
    s.overlap_days = 3;
    s.window_days = 1;
    s.max_pages_per_run = 1;
    let rule = store.create(s, NOW).await.unwrap();
    let (lease, _) = begin(&store, &rule.id, NOW, true).await;
    let state = store
        .prepare_run_source(&lease, "pubmed", NOW + 1)
        .await
        .unwrap()
        .unwrap();
    let target = state.window.as_ref().unwrap().through_ms;
    commit(&store, &lease, &state, &[], None, NOW + 2).await;
    store
        .finish_run(&lease, SubscriptionRunState::Complete, None, NOW + 4)
        .await
        .unwrap();
    let mut scan_id = None;
    for n in 0..3 {
        let now = NOW + 100 + i64::from(n) * 100;
        let (lease, _) = begin(&store, &rule.id, now, true).await;
        let state = store
            .prepare_run_source(&lease, "pubmed", now + 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            state.window.as_ref().unwrap().from_ms,
            target + 1 - i64::from(3 - n) * DAY
        );
        let scan = state.scan.as_ref().unwrap().id.clone();
        if let Some(old) = &scan_id {
            assert_eq!(&scan, old);
        } else {
            scan_id = Some(scan);
        }
        commit(&store, &lease, &state, &[], None, now + 2).await;
        let phase = if n == 2 {
            SubscriptionRunState::Complete
        } else {
            SubscriptionRunState::Partial
        };
        store
            .finish_run(&lease, phase, None, now + 4)
            .await
            .unwrap();
        assert_eq!(
            store.get(&rule.id).await.unwrap().sources[0].watermark_ms,
            Some(target)
        );
    }
}

#[tokio::test]
async fn a_blocked_source_does_not_prevent_other_sources_from_starting_fresh_scans() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let mut s = spec(&["pubmed", "openalex"]);
    s.enabled = true;
    s.initial_days = 1;
    let rule = store.create(s, NOW).await.unwrap();
    let (lease, _) = begin(&store, &rule.id, NOW, false).await;
    let a = store
        .prepare_run_source(&lease, "pubmed", NOW + 1)
        .await
        .unwrap()
        .unwrap();
    store
        .reserve_run_page(&lease, "pubmed", a.sequence, NOW + 2)
        .await
        .unwrap();
    store
        .record_failure(
            &lease,
            "pubmed",
            a.sequence,
            SourceFailure::new(
                SourceFailureKind::AuthRequired,
                "source returned HTTP 403",
                false,
            ),
            None,
            false,
            NOW + 3,
        )
        .await
        .unwrap();
    let b = store
        .prepare_run_source(&lease, "openalex", NOW + 4)
        .await
        .unwrap()
        .unwrap();
    commit(&store, &lease, &b, &[], None, NOW + 5).await;
    store
        .finish_run(&lease, SubscriptionRunState::Partial, None, NOW + 7)
        .await
        .unwrap();
    let (lease, _) = begin(&store, &rule.id, NOW + DAY, false).await;
    let blocked = store
        .prepare_run_source(&lease, "pubmed", NOW + DAY + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(blocked.window, a.window);
    assert!(
        store
            .reserve_run_page(&lease, "pubmed", blocked.sequence, NOW + DAY + 2)
            .await
            .is_err()
    );
    let fresh = store
        .prepare_run_source(&lease, "openalex", NOW + DAY + 3)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(fresh.scan.as_ref().unwrap().id, b.scan.unwrap().id);
    commit(&store, &lease, &fresh, &[], None, NOW + DAY + 4).await;
    store
        .finish_run(&lease, SubscriptionRunState::Partial, None, NOW + DAY + 6)
        .await
        .unwrap();
    assert!(store.get(&rule.id).await.unwrap().last_success_ms.is_none());
}

#[tokio::test]
async fn history_quota_rolls_back_creation_and_reserved_space_allows_finalization_after_lowering() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let a = store.create(spec(&["openalex"]), NOW).await.unwrap();
    let b = store.create(spec(&["openalex"]), NOW).await.unwrap();
    let mut policy = store.limits().await.unwrap();
    policy.max_runs = 1;
    store.set_limits(policy.clone()).await.unwrap();
    let (lease, run) = begin(&store, &a.id, NOW, true).await;
    let other = store.claim(&b.id, &id(), NOW, 60000, true).await.unwrap();
    let other_id = id();
    assert!(matches!(
        store.begin_run(&other, &other_id, true, NOW + 1).await,
        Err(SubscriptionError::Budget(_))
    ));
    assert!(matches!(
        store.run(&other_id).await,
        Err(SubscriptionError::NotFound)
    ));
    policy.max_history_bytes = 65536;
    store.set_limits(policy).await.unwrap();
    store
        .finish_run(
            &lease,
            SubscriptionRunState::Partial,
            Some("budget stop".into()),
            NOW + 2,
        )
        .await
        .unwrap();
    assert_eq!(store.runs(&a.id, None, 10).await.unwrap().0.len(), 1);
    store.delete_run(&run.id).await.unwrap();
    let mut policy = store.limits().await.unwrap();
    policy.max_history_bytes = 256 * 1024 * 1024;
    store.set_limits(policy).await.unwrap();
    store
        .begin_run(&other, &other_id, true, NOW + 3)
        .await
        .unwrap();
}

#[tokio::test]
async fn duplicate_run_creation_returns_one_record_and_an_interrupted_owner_is_fenced() {
    let dir = std::env::temp_dir().join(format!("bib-runs-{}", id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("library.db").to_string_lossy().into_owned();
    let store = SubscriptionStore::open(&path).await.unwrap();
    let rule = store.create(spec(&["openalex"]), NOW).await.unwrap();
    let lease = store.claim(&rule.id, &id(), NOW, 1000, true).await.unwrap();
    let run_id = id();
    let (old, first) = store
        .begin_run(&lease, &run_id, true, NOW + 1)
        .await
        .unwrap();
    let (_, same) = store
        .begin_run(&lease, &run_id, true, NOW + 2)
        .await
        .unwrap();
    assert_eq!(same.started_ms, first.started_ms);
    let state = store
        .prepare_run_source(&old, "openalex", NOW + 3)
        .await
        .unwrap()
        .unwrap();
    store
        .reserve_run_page(&old, "openalex", state.sequence, NOW + 4)
        .await
        .unwrap();
    drop(store);
    let reopened = SubscriptionStore::open(&path).await.unwrap();
    let now = old.expires_ms + 1;
    let (next, _) = begin(&reopened, &rule.id, now, true).await;
    assert_eq!(
        reopened.run(&run_id).await.unwrap().state,
        SubscriptionRunState::Interrupted
    );
    assert!(
        reopened
            .commit_page(
                &old,
                "openalex",
                state.sequence,
                None,
                page(&state, &["late"], None),
                now + 1
            )
            .await
            .is_err()
    );
    let state = reopened
        .prepare_run_source(&next, "openalex", now + 2)
        .await
        .unwrap()
        .unwrap();
    commit(&reopened, &next, &state, &["a"], None, now + 3).await;
    reopened
        .finish_run(&next, SubscriptionRunState::Complete, None, now + 5)
        .await
        .unwrap();
    drop(reopened);
    let _ = std::fs::remove_dir_all(dir);
}
