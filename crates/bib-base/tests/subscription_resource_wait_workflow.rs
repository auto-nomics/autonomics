//! Durable quota waits must prevent unchanged retries without losing wakeups.
use bib_base::{subscription_store::SubscriptionStore, subscriptions::*};
use bib_types::discovery::DiscoveryRequest;
const NOW: i64 = 1_730_000_000_000;
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn spec(enabled: bool) -> SubscriptionSpec {
    SubscriptionSpec {
        name: "Resource wait fixture".into(),
        enabled,
        request: DiscoveryRequest {
            query: "immune".into(),
            sources: vec!["openalex".into()],
            ..Default::default()
        },
        ..Default::default()
    }
}
#[tokio::test]
async fn waits_survive_reopen_and_cannot_be_bypassed_by_manual_claim_or_noop_policy_save() {
    let dir = std::env::temp_dir().join(format!("bib-resource-{}", id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("library.db").to_string_lossy().into_owned();
    let store = SubscriptionStore::open(&path).await.unwrap();
    let rule = store.create(spec(true), NOW).await.unwrap();
    let epoch = store.resource_epoch().await.unwrap();
    let lease = store
        .claim(&rule.id, &id(), NOW, 60000, true)
        .await
        .unwrap();
    store
        .wait_for_resources(&lease, "inbox quota", epoch, NOW + 1)
        .await
        .unwrap();
    store.release(&lease, NOW + 2, 0).await.unwrap();
    let policy = store.limits().await.unwrap();
    store.set_limits(policy).await.unwrap();
    assert_eq!(store.resource_epoch().await.unwrap(), epoch);
    drop(store);
    let reopened = SubscriptionStore::open(&path).await.unwrap();
    assert_eq!(
        reopened
            .get(&rule.id)
            .await
            .unwrap()
            .resource_wait
            .unwrap()
            .reason,
        "inbox quota"
    );
    assert!(
        reopened
            .claim(&rule.id, &id(), NOW + 3, 60000, true)
            .await
            .is_err()
    );
    assert!(
        reopened
            .claim(&rule.id, &id(), NOW + 3, 60000, false)
            .await
            .is_err()
    );
    assert!(reopened.due(NOW + 3, 128).await.unwrap().is_empty());
    drop(reopened);
    let _ = std::fs::remove_dir_all(dir);
}
#[tokio::test]
async fn a_policy_change_wakes_waiters_without_evicting_retained_data() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let rule = store.create(spec(true), NOW).await.unwrap();
    let lease = store
        .claim(&rule.id, &id(), NOW, 60000, true)
        .await
        .unwrap();
    store
        .wait_for_resources(
            &lease,
            "seen bytes",
            store.resource_epoch().await.unwrap(),
            NOW + 1,
        )
        .await
        .unwrap();
    store.release(&lease, NOW + 2, 0).await.unwrap();
    let mut policy = store.limits().await.unwrap();
    policy.max_seen_bytes += 1;
    store.set_limits(policy).await.unwrap();
    assert!(store.get(&rule.id).await.unwrap().resource_wait.is_none());
    assert_eq!(store.due(NOW + 3, 1).await.unwrap(), vec![rule.id.clone()]);
    store
        .claim(&rule.id, &id(), NOW + 3, 60000, false)
        .await
        .unwrap();
}
#[tokio::test]
async fn a_resource_change_between_failure_and_recording_wait_is_not_lost() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let rule = store.create(spec(true), NOW).await.unwrap();
    let lease = store
        .claim(&rule.id, &id(), NOW, 60000, true)
        .await
        .unwrap();
    let observed = store.resource_epoch().await.unwrap();
    let other = store.create(spec(false), NOW).await.unwrap();
    store.delete(&other.id, other.version).await.unwrap();
    store
        .wait_for_resources(&lease, "history quota", observed, NOW + 1)
        .await
        .unwrap();
    store.release(&lease, NOW + 2, 0).await.unwrap();
    assert!(store.get(&rule.id).await.unwrap().resource_wait.is_none());
    store
        .claim(&rule.id, &id(), NOW + 3, 60000, false)
        .await
        .unwrap();
}
#[tokio::test]
async fn due_limit_does_not_let_paused_blocked_or_live_owned_rules_starve_due_work() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    for _ in 0..5 {
        store.create(spec(false), NOW - 100).await.unwrap();
    }
    let blocked = store.create(spec(true), NOW - 100).await.unwrap();
    let live = store.create(spec(true), NOW - 100).await.unwrap();
    let lease = store
        .claim(&blocked.id, &id(), NOW, 60000, true)
        .await
        .unwrap();
    store
        .wait_for_resources(
            &lease,
            "quota",
            store.resource_epoch().await.unwrap(),
            NOW + 1,
        )
        .await
        .unwrap();
    store.release(&lease, NOW + 2, 0).await.unwrap();
    store
        .claim(&live.id, &id(), NOW, 60000, false)
        .await
        .unwrap();
    let ready = store.create(spec(true), NOW).await.unwrap();
    assert_eq!(store.due(NOW + 3, 1).await.unwrap(), vec![ready.id]);
}
#[tokio::test]
async fn cancelling_a_terminal_old_run_does_not_release_its_successor() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let rule = store.create(spec(true), NOW).await.unwrap();
    let raw = store
        .claim(&rule.id, &id(), NOW, 60000, true)
        .await
        .unwrap();
    let (lease, run) = store.begin_run(&raw, &id(), true, NOW + 1).await.unwrap();
    let ended = store.cancel_run(&run.id, NOW + 2).await.unwrap();
    assert_eq!(ended.state, SubscriptionRunState::Cancelled);
    assert!(store.heartbeat(&lease, NOW + 3, 60000).await.is_err());
    let next = store
        .claim(&rule.id, &id(), NOW + 3, 60000, true)
        .await
        .unwrap();
    store.cancel_run(&run.id, NOW + 4).await.unwrap();
    store.heartbeat(&next, NOW + 5, 60000).await.unwrap();
}

#[tokio::test]
async fn resource_recovery_wakes_a_waiter_before_ordinary_next_due_and_is_consumed_once() {
    let store = SubscriptionStore::open(":memory:").await.unwrap();
    let rule = store.create(spec(true), NOW).await.unwrap();
    let lease = store
        .claim(&rule.id, &id(), NOW, 60000, true)
        .await
        .unwrap();
    store
        .wait_for_resources(
            &lease,
            "quota",
            store.resource_epoch().await.unwrap(),
            NOW + 1,
        )
        .await
        .unwrap();
    store.release(&lease, NOW + 2, 60000).await.unwrap();
    let mut policy = store.limits().await.unwrap();
    policy.max_inbox += 1;
    store.set_limits(policy).await.unwrap();
    assert_eq!(store.due(NOW + 3, 1).await.unwrap(), vec![rule.id.clone()]);
    let next = store
        .claim(&rule.id, &id(), NOW + 3, 60000, false)
        .await
        .unwrap();
    store.release(&next, NOW + 4, 60000).await.unwrap();
    assert!(store.due(NOW + 5, 1).await.unwrap().is_empty());
}

#[tokio::test]
async fn independent_connections_share_wait_and_observe_resource_recovery() {
    let dir = std::env::temp_dir().join(format!("bib-resource-connections-{}", id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("library.db").to_string_lossy().into_owned();
    let first = SubscriptionStore::open(&path).await.unwrap();
    let second = SubscriptionStore::open(&path).await.unwrap();
    let rule = first.create(spec(true), NOW).await.unwrap();
    let lease = first
        .claim(&rule.id, &id(), NOW, 60000, true)
        .await
        .unwrap();
    first
        .wait_for_resources(
            &lease,
            "retained payload quota",
            first.resource_epoch().await.unwrap(),
            NOW + 1,
        )
        .await
        .unwrap();
    first.release(&lease, NOW + 2, 60000).await.unwrap();
    assert!(second.get(&rule.id).await.unwrap().resource_wait.is_some());
    assert!(
        second
            .claim(&rule.id, &id(), NOW + 3, 60000, true)
            .await
            .is_err()
    );
    let mut policy = second.limits().await.unwrap();
    policy.max_inbox_bytes += 1;
    second.set_limits(policy).await.unwrap();
    assert!(first.get(&rule.id).await.unwrap().resource_wait.is_none());
    assert_eq!(first.due(NOW + 4, 1).await.unwrap(), vec![rule.id.clone()]);
    first
        .claim(&rule.id, &id(), NOW + 4, 60000, false)
        .await
        .unwrap();
    drop(first);
    drop(second);
    let _ = std::fs::remove_dir_all(dir);
}
