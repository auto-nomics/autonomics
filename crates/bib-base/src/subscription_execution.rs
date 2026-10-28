//! Execute one durable subscription attempt with injected shared source resources.
//! The host owns scheduling; dropping this future drops every source request.
use crate::{
    discovery_sessions::DiscoverySessions, query::LiteratureGateway,
    subscription_store::SubscriptionStore, subscriptions::*,
};
use bib_types::discovery::{SourceFailure, SourceFailureKind};
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

pub const LEASE_TTL_MS: u32 = 30_000;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Clone)]
pub struct SubscriptionExecutor {
    pub store: SubscriptionStore,
    pub sessions: DiscoverySessions,
    pub gateway: Arc<LiteratureGateway>,
}
impl SubscriptionExecutor {
    /// Lookup before claiming: a lost response retries the same retained run,
    /// including terminal attempts, without starting a second provider request.
    pub async fn start(
        &self,
        rule_id: &str,
        run_id: &str,
        manual: bool,
    ) -> Result<(Option<SubscriptionLease>, SubscriptionRun)> {
        if uuid::Uuid::parse_str(run_id).is_err() {
            return Err(invalid("invalid run UUID"));
        }
        match self.store.run(run_id).await {
            Ok(run) => {
                if run.rule_id != rule_id {
                    return Err(conflict("run UUID belongs to another rule"));
                }
                return Ok((None, run));
            }
            Err(SubscriptionError::NotFound) => {}
            Err(error) => return Err(error),
        }
        let owner = uuid::Uuid::new_v4().to_string();
        let lease = match self
            .store
            .claim(rule_id, &owner, now_ms(), LEASE_TTL_MS, manual)
            .await
        {
            Ok(lease) => lease,
            Err(error) => {
                // A competing copy of the same client action may just have
                // committed its run. Never return its process-local capability.
                if let Ok(run) = self.store.run(run_id).await {
                    if run.rule_id == rule_id {
                        return Ok((None, run));
                    }
                }
                return Err(error);
            }
        };
        let observed = self.store.resource_epoch().await?;
        match self.store.begin_run(&lease, run_id, manual, now_ms()).await {
            Ok((lease, run)) => Ok((Some(lease), run)),
            Err(error) => {
                // No provider work has begun. Release a claim whose history
                // reservation failed so the rule is not stranded until expiry.
                let waiting = self.budget(&lease, observed, Err::<(), _>(error)).await;
                let _ = self.store.release(&lease, now_ms(), 60_000).await;
                Err(waiting.expect_err("failed begin_run remains an error"))
            }
        }
    }

    /// `cancel=true`, or releasing the last sender, closes this attempt and
    /// drops network I/O. Durable pause/replacement is discovered by heartbeat.
    pub async fn execute(
        &self,
        lease: SubscriptionLease,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<SubscriptionRun> {
        let run_id = lease
            .run_id
            .as_ref()
            .ok_or_else(|| invalid("persistent run required"))?;
        let run = self.store.run(run_id).await?;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Keep the page loop in its own scope so cancellation/lease loss drops
        // the provider future BEFORE settling or returning the saved record.
        let outcome = {
            let work = self.pages(&lease, &run);
            let monitor = async {
                loop {
                    heartbeat.tick().await;
                    self.store.heartbeat(&lease, now_ms(), LEASE_TTL_MS).await?;
                }
                #[allow(unreachable_code)]
                Ok::<(), SubscriptionError>(())
            };
            let cancelled = async {
                loop {
                    if *cancel.borrow() {
                        break;
                    }
                    if cancel.changed().await.is_err() {
                        break;
                    }
                }
            };
            tokio::pin!(work, monitor, cancelled);
            tokio::select! {
                biased;
                _ = &mut cancelled => Ok((SubscriptionRunState::Cancelled, Some("execution owner released or cancelled".into()))),
                result = &mut monitor => Err(result.err().unwrap_or_else(|| conflict("heartbeat stopped"))),
                result = &mut work => result,
            }
        };
        match outcome {
            Ok((state, reason)) => {
                match self.store.finish_run(&lease, state, reason, now_ms()).await {
                    Ok(run) => Ok(run),
                    Err(error) => {
                        let saved = self.store.run(run_id).await?;
                        if saved.state != SubscriptionRunState::Running {
                            Ok(saved)
                        } else {
                            Err(error)
                        }
                    }
                }
            }
            Err(error) => {
                // Pause/replacement may already have atomically settled it.
                let saved = self.store.run(run_id).await?;
                if saved.state != SubscriptionRunState::Running {
                    return Ok(saved);
                }
                // Preserve the original transaction/provider error; if still
                // fenced, leave a terminal record and release the live claim.
                let message = bounded_message(&error.to_string(), 512);
                let _ = self
                    .store
                    .finish_run(
                        &lease,
                        SubscriptionRunState::Failed,
                        Some(message),
                        now_ms(),
                    )
                    .await;
                Err(error)
            }
        }
    }

    async fn budget<T>(
        &self,
        lease: &SubscriptionLease,
        observed: i64,
        result: Result<T>,
    ) -> Result<T> {
        if let Err(SubscriptionError::Budget(reason)) = &result {
            self.store
                .wait_for_resources(lease, &bounded_message(reason, 512), observed, now_ms())
                .await?;
        }
        result
    }

    async fn pages(
        &self,
        lease: &SubscriptionLease,
        initial: &SubscriptionRun,
    ) -> Result<(SubscriptionRunState, Option<String>)> {
        let mut names = initial.spec.request.sources.clone();
        // Rotate the first source across durable epochs, and take one page per
        // source per pass. Even a one-page budget cannot starve later sources.
        let offset = ((lease.epoch - 1) % names.len() as u64) as usize;
        names.rotate_left(offset);
        let mut unavailable = HashSet::new();
        loop {
            let current = self.store.run(&initial.id).await?;
            if current.sources.iter().all(|s| s.done && s.error.is_none()) {
                return Ok((SubscriptionRunState::Complete, None));
            }
            if now_ms() >= current.deadline_ms {
                return Ok((
                    SubscriptionRunState::Partial,
                    Some("execution time budget exhausted".into()),
                ));
            }
            let mut attempted = false;
            for name in &names {
                if unavailable.contains(name) {
                    continue;
                }
                let observed = self.store.resource_epoch().await?;
                let prepared = self.store.prepare_run_source(lease, name, now_ms()).await;
                let state = match self.budget(lease, observed, prepared).await {
                    Ok(Some(state)) => state,
                    Ok(None) => continue,
                    Err(SubscriptionError::RunBudget(reason)) => {
                        return Ok((SubscriptionRunState::Partial, Some(reason)));
                    }
                    Err(SubscriptionError::Invalid(message)) => {
                        // Planning can fail before a source checkpoint exists.
                        // Preserve it in source/run history without provider I/O.
                        let saved = self.store.get(&lease.rule_id).await?;
                        let state = saved
                            .sources
                            .iter()
                            .find(|s| &s.source == name)
                            .ok_or_else(|| invalid("missing rule source"))?;
                        self.store
                            .record_failure(
                                lease,
                                name,
                                state.sequence,
                                SourceFailure::new(
                                    SourceFailureKind::InvalidQuery,
                                    bounded_message(&message, 4096),
                                    false,
                                ),
                                None,
                                false,
                                now_ms(),
                            )
                            .await?;
                        unavailable.insert(name.clone());
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if state.retry_at_ms.is_some_and(|t| t > now_ms())
                    || (!initial.manual && state.last_error.as_ref().is_some_and(|e| !e.retryable))
                {
                    unavailable.insert(name.clone());
                    continue;
                }
                let reserved = self
                    .store
                    .reserve_run_page(lease, name, state.sequence, now_ms())
                    .await;
                match self.budget(lease, observed, reserved).await {
                    Ok(_) => {}
                    Err(SubscriptionError::RunBudget(reason)) => {
                        return Ok((SubscriptionRunState::Partial, Some(reason)));
                    }
                    Err(error) => return Err(error),
                }
                attempted = true;
                let remaining = current.deadline_ms.saturating_sub(now_ms()).max(0) as u64;
                let timeout = remaining.min(initial.spec.request.source_timeout_ms);
                let plan = state
                    .plan
                    .clone()
                    .ok_or_else(|| invalid("prepared source has no plan"))?;
                let result = self
                    .sessions
                    .execute_planned_page(
                        self.gateway.clone(),
                        plan,
                        state.cursor.clone(),
                        Instant::now() + Duration::from_millis(timeout),
                    )
                    .await;
                let failure = match result {
                    Ok(page) => {
                        let observed = self.store.resource_epoch().await?;
                        let committed = self
                            .store
                            .commit_page(
                                lease,
                                name,
                                state.sequence,
                                state.cursor.as_deref(),
                                page,
                                now_ms(),
                            )
                            .await;
                        match self.budget(lease, observed, committed).await {
                            Ok(_) => continue,
                            Err(SubscriptionError::Invalid(message)) => SourceFailure::new(
                                SourceFailureKind::InvalidResponse,
                                bounded_message(&message, 4096),
                                false,
                            ),
                            Err(error) => return Err(error),
                        }
                    }
                    Err(mut failure) => {
                        failure.message = bounded_message(&failure.message, 4096);
                        failure
                    }
                };
                let mut failure = failure;
                if failure.kind == SourceFailureKind::CursorExpired {
                    // Frozen-window replay is recoverable even when an adapter
                    // classifies its obsolete provider cursor as non-retryable.
                    failure.retryable = true;
                }
                let now = now_ms();
                let retry = if failure.retryable || failure.kind == SourceFailureKind::RateLimited {
                    // Never shorten a supplied Retry-After. Saturation errs on
                    // the side of waiting instead of overflowing into the past.
                    let seconds = failure.retry_after_seconds.unwrap_or(60).max(1);
                    Some(now.saturating_add(seconds.saturating_mul(1000).min(i64::MAX as u64) as i64))
                } else {
                    None
                };
                let replay = failure.kind == SourceFailureKind::CursorExpired;
                self.store
                    .record_failure(lease, name, state.sequence, failure, retry, replay, now)
                    .await?;
                unavailable.insert(name.clone());
            }
            if !attempted {
                let saved = self.store.run(&initial.id).await?;
                let complete = saved.sources.iter().all(|s| s.done && s.error.is_none());
                return Ok((
                    if complete {
                        SubscriptionRunState::Complete
                    } else {
                        SubscriptionRunState::Partial
                    },
                    if complete {
                        None
                    } else {
                        Some("sources incomplete, failed or waiting for retry".into())
                    },
                ));
            }
        }
    }
}
fn bounded_message(value: &str, max: usize) -> String {
    let mut end = value.len().min(max);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
