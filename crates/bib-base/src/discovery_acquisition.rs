//! Bounded, process-local full-text jobs independent of discovery session TTL.
//! Cancellation skips queued work; an active file import finishes atomically.
use crate::{BibShared, discovery_sessions::DiscoverySessionError};
use bib_types::{Article, discovery::*};
use chrono::Utc;
use futures::FutureExt;
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, watch};

const MAX_ACTIVE: usize = 16;
const MAX_RETAINED: usize = 64;
const RETAIN_FOR: Duration = Duration::from_secs(1800);
const JOB_BUDGET: Duration = Duration::from_secs(900);
const ITEM_BUDGET: Duration = Duration::from_secs(120);
type Result<T> = std::result::Result<T, DiscoverySessionError>;

struct Entry {
    job: DiscoveryAcquisitionJob,
    finished: Option<Instant>,
    cancelled: watch::Sender<bool>,
}
struct Inner {
    entries: Mutex<HashMap<String, Entry>>,
    permits: Arc<Semaphore>,
}
#[derive(Clone)]
pub struct DiscoveryAcquisitions {
    inner: Arc<Inner>,
}
impl Default for DiscoveryAcquisitions {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                entries: Mutex::new(HashMap::new()),
                permits: Arc::new(Semaphore::new(2)),
            }),
        }
    }
}
/// Reserve capacity before writing metadata. Dropping an unused reservation
/// releases it, including import validation/storage failures.
pub struct AcquisitionReservation {
    manager: DiscoveryAcquisitions,
    id: String,
    started: bool,
}
impl Drop for AcquisitionReservation {
    fn drop(&mut self) {
        if !self.started {
            self.manager.inner.entries.lock().unwrap().remove(&self.id);
        }
    }
}
impl DiscoveryAcquisitions {
    pub fn reserve(&self) -> Result<AcquisitionReservation> {
        let mut entries = self.inner.entries.lock().unwrap();
        entries.retain(|_, entry| entry.finished.is_none_or(|at| at.elapsed() < RETAIN_FOR));
        if entries
            .values()
            .filter(|entry| entry.finished.is_none())
            .count()
            >= MAX_ACTIVE
        {
            return Err(DiscoverySessionError::Capacity(
                "full-text acquisition queue is full".into(),
            ));
        }
        if entries.len() >= MAX_RETAINED {
            let oldest = entries
                .iter()
                .filter_map(|(id, entry)| entry.finished.map(|at| (id.clone(), at)))
                .min_by_key(|(_, at)| *at)
                .map(|(id, _)| id);
            if let Some(id) = oldest {
                entries.remove(&id);
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        entries.insert(
            id.clone(),
            Entry {
                job: DiscoveryAcquisitionJob {
                    id: id.clone(),
                    created_at: Utc::now().to_rfc3339(),
                    completed_at: None,
                    cancel_requested: false,
                    items: Vec::new(),
                },
                finished: None,
                cancelled: watch::channel(false).0,
            },
        );
        Ok(AcquisitionReservation {
            manager: self.clone(),
            id,
            started: false,
        })
    }
    pub fn snapshot(&self, id: &str) -> Result<DiscoveryAcquisitionJob> {
        let mut entries = self.inner.entries.lock().unwrap();
        entries.retain(|_, entry| entry.finished.is_none_or(|at| at.elapsed() < RETAIN_FOR));
        entries
            .get(id)
            .map(|entry| entry.job.clone())
            .ok_or(DiscoverySessionError::NotFound)
    }
    pub fn cancel(&self, id: &str) -> Result<DiscoveryAcquisitionJob> {
        let mut entries = self.inner.entries.lock().unwrap();
        let entry = entries
            .get_mut(id)
            .filter(|entry| entry.finished.is_none_or(|at| at.elapsed() < RETAIN_FOR))
            .ok_or(DiscoverySessionError::NotFound)?;
        if entry.finished.is_none() {
            entry.job.cancel_requested = true;
            entry.cancelled.send_replace(true);
            for item in &mut entry.job.items {
                if item.status == DiscoveryAcquisitionStatus::Queued {
                    item.status = DiscoveryAcquisitionStatus::Cancelled;
                }
            }
        }
        Ok(entry.job.clone())
    }
}
impl AcquisitionReservation {
    pub fn start(
        self,
        shared: BibShared,
        articles: Vec<Article>,
    ) -> Result<DiscoveryAcquisitionJob> {
        let articles = articles
            .into_iter()
            .map(|article| Article::new(article.id, ""))
            .collect();
        self.start_with(articles, move |article| {
            let shared = shared.clone();
            async move {
                let article = shared.bib.get_article(&article.id).await.map_err(|_| "could not read library item".to_string())?.ok_or_else(|| "library item was deleted".to_string())?;
                crate::oa_fetch::fetch_pdf(&shared, &article).await.map(|fulltext| fulltext.is_some()).map_err(|error| {
                    tracing::warn!(article_id=%article.id, %error, "discovery full-text acquisition failed");
                    "full-text acquisition failed; retry the item or inspect server logs".into()
                })
            }
        })
    }
    fn start_with<F, Fut>(
        mut self,
        articles: Vec<Article>,
        fetch: F,
    ) -> Result<DiscoveryAcquisitionJob>
    where
        F: Fn(Article) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::result::Result<bool, String>> + Send + 'static,
    {
        if articles.is_empty() || articles.len() > 100 {
            return Err(DiscoverySessionError::Invalid(
                "acquisition needs between 1 and 100 articles".into(),
            ));
        }
        let mut seen = HashSet::new();
        let articles: Vec<_> = articles
            .into_iter()
            .filter(|article| seen.insert(article.id.clone()))
            .collect();
        let initial = {
            let mut entries = self.manager.inner.entries.lock().unwrap();
            let entry = entries
                .get_mut(&self.id)
                .ok_or(DiscoverySessionError::NotFound)?;
            entry.job.items = articles
                .iter()
                .map(|article| DiscoveryAcquisitionItem {
                    article_id: article.id.clone(),
                    status: DiscoveryAcquisitionStatus::Queued,
                    error: None,
                })
                .collect();
            entry.job.clone()
        };
        self.started = true;
        let inner = self.manager.inner.clone();
        let id = self.id.clone();
        let mut cancelled = inner
            .entries
            .lock()
            .unwrap()
            .get(&id)
            .unwrap()
            .cancelled
            .subscribe();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + JOB_BUDGET;
            for (index, article) in articles.into_iter().enumerate() {
                let cancel_requested = inner
                    .entries
                    .lock()
                    .unwrap()
                    .get(&id)
                    .is_none_or(|entry| entry.job.cancel_requested);
                if cancel_requested {
                    break;
                }
                let permit = tokio::select! {
                    permit = tokio::time::timeout_at(deadline, inner.permits.clone().acquire_owned()) => permit,
                    _ = cancelled.changed() => break,
                };
                let Ok(Ok(permit)) = permit else {
                    if let Some(entry) = inner.entries.lock().unwrap().get_mut(&id) {
                        for item in &mut entry.job.items[index..] {
                            if item.status == DiscoveryAcquisitionStatus::Queued {
                                item.status = DiscoveryAcquisitionStatus::Failed;
                                item.error = Some("acquisition job deadline exceeded".into());
                            }
                        }
                    }
                    break;
                };
                {
                    let mut entries = inner.entries.lock().unwrap();
                    let Some(entry) = entries.get_mut(&id) else {
                        break;
                    };
                    if entry.job.cancel_requested {
                        break;
                    }
                    entry.job.items[index].status = DiscoveryAcquisitionStatus::Running;
                }
                let item_deadline = deadline.min(tokio::time::Instant::now() + ITEM_BUDGET);
                let result = tokio::time::timeout_at(
                    item_deadline,
                    std::panic::AssertUnwindSafe(async { fetch(article).await }).catch_unwind(),
                )
                .await;
                drop(permit);
                let (status, error) = match result {
                    Ok(Ok(Ok(true))) => (DiscoveryAcquisitionStatus::Available, None),
                    Ok(Ok(Ok(false))) => (DiscoveryAcquisitionStatus::Unavailable, None),
                    Ok(Ok(Err(error))) => (DiscoveryAcquisitionStatus::Failed, Some(error)),
                    Ok(Err(_)) => (
                        DiscoveryAcquisitionStatus::Failed,
                        Some("full-text provider failed".into()),
                    ),
                    Err(_) => (
                        DiscoveryAcquisitionStatus::Failed,
                        Some("full-text acquisition timed out".into()),
                    ),
                };
                if let Some(entry) = inner.entries.lock().unwrap().get_mut(&id) {
                    entry.job.items[index].status = status;
                    entry.job.items[index].error = error;
                }
            }
            if let Some(entry) = inner.entries.lock().unwrap().get_mut(&id) {
                entry.job.completed_at = Some(Utc::now().to_rfc3339());
                entry.finished = Some(Instant::now());
            }
        });
        Ok(initial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;
    async fn wait(
        manager: &DiscoveryAcquisitions,
        id: &str,
        done: impl Fn(&DiscoveryAcquisitionJob) -> bool,
    ) -> DiscoveryAcquisitionJob {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let job = manager.snapshot(id).unwrap();
                if done(&job) {
                    break job;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn cancel_finishes_current_import_and_skips_queued_items() {
        let manager = DiscoveryAcquisitions::default();
        let gate = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let job = manager
            .reserve()
            .unwrap()
            .start_with(vec![Article::new("a", "a"), Article::new("b", "b")], {
                let gate = gate.clone();
                let calls = calls.clone();
                move |_| {
                    let gate = gate.clone();
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::AcqRel);
                        gate.notified().await;
                        Ok(true)
                    }
                }
            })
            .unwrap();
        wait(&manager, &job.id, |job| {
            job.items[0].status == DiscoveryAcquisitionStatus::Running
        })
        .await;
        manager.cancel(&job.id).unwrap();
        gate.notify_one();
        let done = wait(&manager, &job.id, |job| job.completed_at.is_some()).await;
        assert_eq!(done.items[0].status, DiscoveryAcquisitionStatus::Available);
        assert_eq!(done.items[1].status, DiscoveryAcquisitionStatus::Cancelled);
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }
    #[tokio::test]
    async fn reservations_release_capacity_and_peer_failures_do_not_abort_jobs() {
        let manager = DiscoveryAcquisitions::default();
        let reservations: Vec<_> = (0..MAX_ACTIVE)
            .map(|_| manager.reserve().unwrap())
            .collect();
        assert!(matches!(
            manager.reserve(),
            Err(DiscoverySessionError::Capacity(_))
        ));
        drop(reservations);
        let job = manager
            .reserve()
            .unwrap()
            .start_with(
                vec![
                    Article::new("a", "a"),
                    Article::new("b", "b"),
                    Article::new("c", "c"),
                ],
                |article| async move {
                    match article.id.as_str() {
                        "a" => Err("fixture storage error".into()),
                        "b" => Ok(false),
                        _ => Ok(true),
                    }
                },
            )
            .unwrap();
        let done = wait(&manager, &job.id, |job| job.completed_at.is_some()).await;
        assert_eq!(
            done.items
                .iter()
                .map(|item| item.status)
                .collect::<Vec<_>>(),
            vec![
                DiscoveryAcquisitionStatus::Failed,
                DiscoveryAcquisitionStatus::Unavailable,
                DiscoveryAcquisitionStatus::Available
            ]
        );
    }
    #[tokio::test]
    async fn global_permits_apply_across_jobs_and_queued_cancellation_never_calls_provider() {
        let manager = DiscoveryAcquisitions::default();
        let held = manager
            .inner
            .permits
            .clone()
            .acquire_many_owned(2)
            .await
            .unwrap();
        let job = manager
            .reserve()
            .unwrap()
            .start_with(vec![Article::new("a", "a")], |_| async {
                panic!("queued cancelled job must not start")
            })
            .unwrap();
        manager.cancel(&job.id).unwrap();
        let done = wait(&manager, &job.id, |job| job.completed_at.is_some()).await;
        assert_eq!(done.items[0].status, DiscoveryAcquisitionStatus::Cancelled);
        drop(held);
    }
}
