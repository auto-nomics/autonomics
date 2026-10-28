//! Real executor with injected source fixtures; no external network calls.
use bib_base::{
    discovery_sessions::DiscoverySessions,
    query::{LiteratureGateway, LiteratureSource, SourceBatch},
    subscription_execution::{SubscriptionExecutor, now_ms},
    subscription_store::SubscriptionStore,
    subscriptions::*,
};
use bib_types::{Article, Identifier, StructuredSearch, discovery::*};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, watch};
enum Step {
    Page(Option<&'static str>),
    Fail(SourceFailure),
    Hold,
    Wait(Arc<Notify>),
}
struct Source {
    name: &'static str,
    steps: Mutex<VecDeque<Step>>,
    calls: AtomicUsize,
    dropped: AtomicUsize,
    started: Notify,
}
struct Witness<'a>(&'a AtomicUsize);
impl Drop for Witness<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl LiteratureSource for Source {
    fn name(&self) -> &'static str {
        self.name
    }
    async fn search(&self, _: &StructuredSearch, _: usize) -> bib_base::Result<SourceBatch> {
        unreachable!()
    }
    async fn fetch(&self, _: &Identifier) -> bib_base::Result<Option<Article>> {
        unreachable!()
    }
    async fn discovery_page(
        &self,
        plan: &SourcePlan,
        _: Option<&str>,
        _: u16,
    ) -> std::result::Result<SourcePage, SourceFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let _witness = Witness(&self.dropped);
        self.started.notify_one();
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected provider request");
        let next = match step {
            Step::Fail(e) => return Err(e),
            Step::Hold => std::future::pending::<Option<&str>>().await,
            Step::Wait(signal) => {
                signal.notified().await;
                None
            }
            Step::Page(next) => next,
        };
        Ok(SourcePage {
            source: self.name.into(),
            articles: vec![Article::new(
                format!("{}-{}", self.name, self.calls.load(Ordering::SeqCst)),
                "Fixture result",
            )],
            total: None,
            next_cursor: next.map(str::to_owned),
            executed_parameters: plan.parameters.clone(),
            query_translation: None,
            notices: Vec::new(),
        })
    }
}
fn source(name: &'static str, steps: Vec<Step>) -> Arc<Source> {
    Arc::new(Source {
        name,
        steps: Mutex::new(steps.into()),
        calls: AtomicUsize::new(0),
        dropped: AtomicUsize::new(0),
        started: Notify::new(),
    })
}
async fn executor(
    sources: &[Arc<Source>],
    max_pages: u16,
) -> (SubscriptionExecutor, SubscriptionRule) {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let spec = SubscriptionSpec {
        name: "Executor fixture".into(),
        enabled: true,
        max_pages_per_run: max_pages,
        request: DiscoveryRequest {
            query: "immune".into(),
            sources: sources.iter().map(|s| s.name.into()).collect(),
            ..Default::default()
        },
        ..Default::default()
    };
    let rule = store.create(spec, now_ms()).await.unwrap();
    let mut gateway = LiteratureGateway::new();
    for s in sources {
        gateway.add_source(s.clone());
    }
    (
        SubscriptionExecutor {
            store,
            sessions: DiscoverySessions::default(),
            gateway: Arc::new(gateway),
        },
        rule,
    )
}
async fn execute(e: &SubscriptionExecutor, rule: &str) -> SubscriptionRun {
    let id = uuid::Uuid::new_v4().to_string();
    let (lease, _) = e.start(rule, &id, true).await.unwrap();
    let (_sender, receiver) = watch::channel(false);
    tokio::time::timeout(Duration::from_secs(10), e.execute(lease.unwrap(), receiver))
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn a_one_page_budget_rotates_sources_between_runs_without_starvation() {
    let a = source("openalex", vec![Step::Page(Some("second"))]);
    let b = source("semantic_scholar", vec![Step::Page(None)]);
    let (e, rule) = executor(&[a.clone(), b.clone()], 1).await;
    let first = execute(&e, &rule.id).await;
    assert_eq!(first.state, SubscriptionRunState::Partial);
    assert_eq!(first.pages_started, 1);
    assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    let second = execute(&e, &rule.id).await;
    assert_eq!(second.state, SubscriptionRunState::Partial);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    let astate = e
        .store
        .get(&rule.id)
        .await
        .unwrap()
        .sources
        .into_iter()
        .find(|s| s.source == "openalex")
        .unwrap();
    assert_eq!(astate.cursor.as_deref(), Some("second"));
    assert_eq!(e.store.inbox(None, 10).await.unwrap().0.len(), 2);
}
#[tokio::test]
async fn a_completed_run_retry_returns_original_without_new_lease_or_provider_calls() {
    let a = source("openalex", vec![Step::Page(None)]);
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    let first = execute(&e, &rule.id).await;
    assert_eq!(first.state, SubscriptionRunState::Complete);
    assert_eq!(first.records, 1);
    let (lease, same) = e.start(&rule.id, &first.id, true).await.unwrap();
    assert!(lease.is_none());
    assert_eq!(same.id, first.id);
    assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    let other = e
        .store
        .create(
            SubscriptionSpec {
                name: "Other rule".into(),
                request: rule.spec.request.clone(),
                ..Default::default()
            },
            now_ms(),
        )
        .await
        .unwrap();
    assert!(e.start(&other.id, &first.id, true).await.is_err());
}
#[tokio::test]
async fn retry_after_is_durable_and_manual_check_does_not_request_early() {
    let mut error = SourceFailure::new(SourceFailureKind::RateLimited, "slow down", true);
    error.retry_after_seconds = Some(3600);
    let a = source("openalex", vec![Step::Fail(error)]);
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    let first = execute(&e, &rule.id).await;
    assert_eq!(first.pages_started, 1);
    assert_eq!(first.pages_committed, 0);
    assert_eq!(first.reserved_records, 0);
    let retry = e.store.get(&rule.id).await.unwrap().sources[0]
        .retry_at_ms
        .unwrap();
    assert!(retry >= first.started_ms + 3_600_000);
    let next = execute(&e, &rule.id).await;
    assert_eq!(next.state, SubscriptionRunState::Partial);
    assert_eq!(next.pages_started, 0);
    assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    assert!(
        e.store
            .get(&rule.id)
            .await
            .unwrap()
            .last_success_ms
            .is_none()
    );
}
#[tokio::test]
async fn cancellation_drops_provider_before_finishing_and_keeps_checkpoint() {
    let a = source("openalex", vec![Step::Hold]);
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    let (lease, run) = e
        .start(&rule.id, &uuid::Uuid::new_v4().to_string(), true)
        .await
        .unwrap();
    let (sender, receiver) = watch::channel(false);
    let task = tokio::spawn({
        let e = e.clone();
        async move { e.execute(lease.unwrap(), receiver).await }
    });
    tokio::time::timeout(Duration::from_secs(5), a.started.notified())
        .await
        .unwrap();
    sender.send(true).unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(ended.id, run.id);
    assert_eq!(ended.state, SubscriptionRunState::Cancelled);
    assert_eq!(ended.pages_committed, 0);
    assert_eq!(ended.reserved_records, 0);
    assert_eq!(a.dropped.load(Ordering::SeqCst), 1);
    assert!(e.store.inbox(None, 10).await.unwrap().0.is_empty());
}
#[tokio::test]
async fn durable_pause_is_observed_by_heartbeat_and_drops_inflight_io() {
    let a = source("openalex", vec![Step::Hold]);
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    let (lease, _) = e
        .start(&rule.id, &uuid::Uuid::new_v4().to_string(), true)
        .await
        .unwrap();
    let (_sender, receiver) = watch::channel(false);
    let task = tokio::spawn({
        let e = e.clone();
        async move { e.execute(lease.unwrap(), receiver).await }
    });
    tokio::time::timeout(Duration::from_secs(5), a.started.notified())
        .await
        .unwrap();
    e.store
        .set_enabled(&rule.id, rule.version, false, now_ms())
        .await
        .unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(ended.state, SubscriptionRunState::Cancelled);
    assert_eq!(a.dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn heartbeat_renews_a_short_original_lease_while_provider_waits() {
    let release = Arc::new(Notify::new());
    let a = source("openalex", vec![Step::Wait(release.clone())]);
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    let raw = e
        .store
        .claim(
            &rule.id,
            &uuid::Uuid::new_v4().to_string(),
            now_ms(),
            1000,
            true,
        )
        .await
        .unwrap();
    let (lease, _) = e
        .store
        .begin_run(&raw, &uuid::Uuid::new_v4().to_string(), true, now_ms())
        .await
        .unwrap();
    let (_sender, receiver) = watch::channel(false);
    let task = tokio::spawn({
        let e = e.clone();
        async move { e.execute(lease, receiver).await }
    });
    tokio::time::timeout(Duration::from_secs(5), a.started.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(now_ms() > raw.expires_ms);
    release.notify_one();
    let ended = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(ended.state, SubscriptionRunState::Complete);
    assert_eq!(ended.pages_committed, 1);
}
#[tokio::test]
async fn permanent_source_error_does_not_stop_other_sources_or_claim_global_success() {
    let a = source(
        "openalex",
        vec![Step::Fail(SourceFailure::new(
            SourceFailureKind::AuthRequired,
            "HTTP 403",
            false,
        ))],
    );
    let b = source("semantic_scholar", vec![Step::Page(None)]);
    let (e, rule) = executor(&[a.clone(), b.clone()], 4).await;
    let ended = execute(&e, &rule.id).await;
    assert_eq!(ended.state, SubscriptionRunState::Partial);
    assert_eq!(ended.pages_started, 2);
    assert_eq!(ended.pages_committed, 1);
    assert!(
        ended
            .sources
            .iter()
            .find(|s| s.source == "openalex")
            .unwrap()
            .error
            .is_some()
    );
    assert!(
        ended
            .sources
            .iter()
            .find(|s| s.source == "semantic_scholar")
            .unwrap()
            .done
    );
    assert!(
        e.store
            .get(&rule.id)
            .await
            .unwrap()
            .last_success_ms
            .is_none()
    );
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn releasing_execution_owner_cancels_the_provider_future() {
    let a = source("openalex", vec![Step::Hold]);
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    let (lease, _) = e
        .start(&rule.id, &uuid::Uuid::new_v4().to_string(), true)
        .await
        .unwrap();
    let (sender, receiver) = watch::channel(false);
    let task = tokio::spawn({
        let e = e.clone();
        async move { e.execute(lease.unwrap(), receiver).await }
    });
    tokio::time::timeout(Duration::from_secs(5), a.started.notified())
        .await
        .unwrap();
    drop(sender);
    let ended = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(ended.state, SubscriptionRunState::Cancelled);
    assert_eq!(a.dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn shared_bundle_uses_injected_gateway_and_does_not_start_requests_on_construction() {
    let a = source("openalex", vec![Step::Page(None)]);
    let mut shared = bib_base::BibShared::open_in_memory().await.unwrap();
    shared.gateway = Arc::new(LiteratureGateway::new().with_source(a.clone()));
    let e = shared.subscription_executor();
    let rule = shared
        .subscriptions
        .create(
            SubscriptionSpec {
                name: "Injected shared bundle".into(),
                enabled: true,
                request: DiscoveryRequest {
                    query: "immune".into(),
                    sources: vec!["openalex".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
            now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(a.calls.load(Ordering::SeqCst), 0);
    assert!(
        shared
            .subscriptions
            .runs(&rule.id, None, 10)
            .await
            .unwrap()
            .0
            .is_empty()
    );
    let ended = execute(&e, &rule.id).await;
    assert_eq!(ended.state, SubscriptionRunState::Complete);
    assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        shared.subscriptions.inbox(None, 10).await.unwrap().0.len(),
        1
    );
}

#[tokio::test]
async fn quota_failure_blocks_unchanged_requery_and_resumes_after_policy_change() {
    let a = source("openalex", vec![Step::Page(None), Step::Page(None)]);
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    let mut policy = e.store.limits().await.unwrap();
    policy.max_page_bytes = 1;
    e.store.set_limits(policy.clone()).await.unwrap();
    let (lease, run) = e
        .start(&rule.id, &uuid::Uuid::new_v4().to_string(), true)
        .await
        .unwrap();
    let (_sender, receiver) = watch::channel(false);
    assert!(matches!(
        e.execute(lease.unwrap(), receiver).await,
        Err(SubscriptionError::Budget(_))
    ));
    assert_eq!(
        e.store.run(&run.id).await.unwrap().state,
        SubscriptionRunState::Failed
    );
    assert!(e.store.get(&rule.id).await.unwrap().resource_wait.is_some());
    assert!(e.store.inbox(None, 10).await.unwrap().0.is_empty());
    assert!(
        e.start(&rule.id, &uuid::Uuid::new_v4().to_string(), true)
            .await
            .is_err()
    );
    assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    policy.max_page_bytes = 2 * 1024 * 1024;
    e.store.set_limits(policy).await.unwrap();
    let ended = execute(&e, &rule.id).await;
    assert_eq!(ended.state, SubscriptionRunState::Complete);
    assert_eq!(ended.pages_committed, 1);
    assert_eq!(a.calls.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn exhausted_history_blocks_before_io_and_deleting_history_releases_wait() {
    let a = source("openalex", vec![Step::Page(None), Step::Page(None)]);
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    let mut policy = e.store.limits().await.unwrap();
    policy.max_runs = 1;
    e.store.set_limits(policy).await.unwrap();
    let first = execute(&e, &rule.id).await;
    assert!(matches!(
        e.start(&rule.id, &uuid::Uuid::new_v4().to_string(), true)
            .await,
        Err(SubscriptionError::Budget(_))
    ));
    assert!(e.store.get(&rule.id).await.unwrap().resource_wait.is_some());
    assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    let (lease, same) = e.start(&rule.id, &first.id, true).await.unwrap();
    assert!(lease.is_none());
    assert_eq!(same.id, first.id);
    e.store.delete_run(&first.id).await.unwrap();
    assert!(e.store.get(&rule.id).await.unwrap().resource_wait.is_none());
    assert_eq!(
        execute(&e, &rule.id).await.state,
        SubscriptionRunState::Complete
    );
    assert_eq!(a.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn automatic_scheduler_starts_enabled_rules_and_skips_paused_rules() {
    use bib_base::subscription_scheduler::SubscriptionScheduler;
    let a = source("openalex", vec![Step::Page(None)]);
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    e.store
        .create(
            SubscriptionSpec {
                name: "Paused".into(),
                request: rule.spec.request.clone(),
                ..Default::default()
            },
            now_ms() - 100,
        )
        .await
        .unwrap();
    let scheduler = SubscriptionScheduler::start(e.clone(), 2).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if e.store
                .get(&rule.id)
                .await
                .unwrap()
                .last_success_ms
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    scheduler.shutdown().await;
    assert!(!scheduler.status().running);
}
#[tokio::test]
async fn scheduler_bounds_workers_and_last_handle_drop_cancels_all_active_requests() {
    use bib_base::subscription_scheduler::SubscriptionScheduler;
    let a = source("openalex", vec![Step::Hold, Step::Hold]);
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    for name in ["Second", "Third"] {
        e.store
            .create(
                SubscriptionSpec {
                    name: name.into(),
                    request: rule.spec.request.clone(),
                    enabled: true,
                    ..Default::default()
                },
                now_ms(),
            )
            .await
            .unwrap();
    }
    let scheduler = SubscriptionScheduler::start(e.clone(), 2).unwrap();
    let mut status = scheduler.observe();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if a.calls.load(Ordering::SeqCst) == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(a.calls.load(Ordering::SeqCst), 2);
    assert_eq!(scheduler.status().active, 2);
    let copy = scheduler.clone();
    drop(scheduler);
    assert_eq!(a.dropped.load(Ordering::SeqCst), 0);
    drop(copy);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !status.borrow().running {
                break;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(a.dropped.load(Ordering::SeqCst), 2);
    let all = e.store.list().await.unwrap();
    let mut cancelled = 0;
    for rule in all {
        for run in e.store.runs(&rule.id, None, 10).await.unwrap().0 {
            if run.state == SubscriptionRunState::Cancelled {
                cancelled += 1;
            }
        }
    }
    assert_eq!(cancelled, 2);
}
#[tokio::test]
async fn manual_scheduler_checks_paused_rule_idempotently_and_cancels_by_run_id() {
    use bib_base::subscription_scheduler::SubscriptionScheduler;
    let a = source("openalex", vec![Step::Hold]);
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    e.store
        .set_enabled(&rule.id, rule.version, false, now_ms())
        .await
        .unwrap();
    let scheduler = SubscriptionScheduler::start(e.clone(), 1).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let started = scheduler.check_now(&rule.id, &id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), a.started.notified())
        .await
        .unwrap();
    let same = scheduler.check_now(&rule.id, &id).await.unwrap();
    assert_eq!(same.id, started.id);
    assert!(
        scheduler
            .check_now(&rule.id, &uuid::Uuid::new_v4().to_string())
            .await
            .is_err()
    );
    let cancelled = scheduler.cancel(&id).await.unwrap();
    assert_eq!(cancelled.state, SubscriptionRunState::Cancelled);
    scheduler.shutdown().await;
    assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    assert_eq!(a.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(e.store.runs(&rule.id, None, 10).await.unwrap().0.len(), 1);
}

#[tokio::test]
async fn scheduler_waits_for_resources_then_retries_once_when_policy_changes() {
    use bib_base::subscription_scheduler::SubscriptionScheduler;
    let a = source("openalex", vec![Step::Page(None), Step::Page(None)]);
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    let mut policy = e.store.limits().await.unwrap();
    policy.max_page_bytes = 1;
    e.store.set_limits(policy.clone()).await.unwrap();
    let scheduler = SubscriptionScheduler::start(e.clone(), 2).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if e.store.get(&rule.id).await.unwrap().resource_wait.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    policy.max_page_bytes = 2 * 1024 * 1024;
    e.store.set_limits(policy).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if e.store
                .get(&rule.id)
                .await
                .unwrap()
                .last_success_ms
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(a.calls.load(Ordering::SeqCst), 2);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn expired_cursor_is_persisted_as_replayable_and_not_a_permanent_background_block() {
    let a = source(
        "openalex",
        vec![
            Step::Page(Some("expired-token")),
            Step::Fail(SourceFailure::new(
                SourceFailureKind::CursorExpired,
                "old cursor rejected",
                false,
            )),
        ],
    );
    let (e, rule) = executor(std::slice::from_ref(&a), 3).await;
    let ended = execute(&e, &rule.id).await;
    assert_eq!(ended.state, SubscriptionRunState::Partial);
    let state = e.store.get(&rule.id).await.unwrap().sources.remove(0);
    assert!(state.cursor.is_none());
    assert!(state.last_error.as_ref().unwrap().retryable);
    let retry = state.retry_at_ms.unwrap();
    let lease = e
        .store
        .claim(
            &rule.id,
            &uuid::Uuid::new_v4().to_string(),
            retry.max(e.store.get(&rule.id).await.unwrap().next_due_ms) + 1,
            60000,
            false,
        )
        .await
        .unwrap();
    let now = retry.max(e.store.get(&rule.id).await.unwrap().next_due_ms) + 2;
    let (lease, _) = e
        .store
        .begin_run(&lease, &uuid::Uuid::new_v4().to_string(), false, now)
        .await
        .unwrap();
    let replay = e
        .store
        .prepare_run_source(&lease, "openalex", now + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.plan, state.plan);
    assert_eq!(replay.window, state.window);
    assert_eq!(replay.scan.unwrap().id, state.scan.unwrap().id);
    e.store
        .reserve_run_page(&lease, "openalex", replay.sequence, now + 2)
        .await
        .unwrap();
}
