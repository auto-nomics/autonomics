//! Bounded external discovery sessions. Network futures run independently per
//! source; cancellation/epochs prevent any old attempt from publishing later.
use crate::{
    discovery_merge::CandidateStore,
    query::{LiteratureGateway, LiteratureSource},
};
use bib_types::discovery::*;
use chrono::Utc;
use futures::FutureExt;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};

#[derive(Debug, thiserror::Error)]
pub enum DiscoverySessionError {
    #[error("{0}")]
    Invalid(String),
    #[error("discovery session not found or expired")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Capacity(String),
}
type Result<T> = std::result::Result<T, DiscoverySessionError>;

#[derive(Debug, Clone)]
pub struct DiscoveryLimits {
    pub max_sessions: usize,
    pub ttl: Duration,
    pub max_records_per_session: usize,
    pub max_records_total: usize,
    /// Accounting bound on retained serialized contributions, not an RSS SLO.
    pub max_payload_bytes: usize,
    pub retained_events: usize,
    pub concurrent_requests: usize,
    pub concurrent_requests_per_source: usize,
    pub max_attempts: usize,
    pub retry_backoff: Duration,
}
impl Default for DiscoveryLimits {
    fn default() -> Self {
        Self {
            max_sessions: 32,
            ttl: Duration::from_secs(1800),
            max_records_per_session: 10_000,
            max_records_total: 50_000,
            max_payload_bytes: 128 * 1024 * 1024,
            retained_events: 256,
            concurrent_requests: 8,
            concurrent_requests_per_source: 2,
            max_attempts: 3,
            retry_backoff: Duration::from_millis(200),
        }
    }
}

struct SourceResources {
    permits: Arc<Semaphore>,
    concurrency_limit: usize,
    cooldown: Mutex<Option<Instant>>,
    blocked: AtomicBool,
}
struct Resources {
    limits: DiscoveryLimits,
    permits: Arc<Semaphore>,
    sources: Mutex<HashMap<String, Arc<SourceResources>>>,
    records: AtomicUsize,
    bytes: AtomicUsize,
}
impl Resources {
    async fn acquire(
        &self,
        name: &str,
    ) -> std::result::Result<(OwnedSemaphorePermit, OwnedSemaphorePermit), SourceFailure> {
        let source = self.source(name);
        loop {
            if source.blocked.load(Ordering::Acquire) {
                return Err(SourceFailure::new(
                    SourceFailureKind::RateLimited,
                    "source retry window exceeds the process clock range",
                    false,
                ));
            }
            let cooldown = *source.cooldown.lock().unwrap();
            if let Some(until) = cooldown {
                tokio::time::sleep(until.saturating_duration_since(Instant::now())).await;
            }
            let source_permit = source
                .permits
                .clone()
                .acquire_owned()
                .await
                .expect("source semaphore stays open");
            let global_permit = self
                .permits
                .clone()
                .acquire_owned()
                .await
                .expect("global semaphore stays open");
            if source.blocked.load(Ordering::Acquire) {
                return Err(SourceFailure::new(
                    SourceFailureKind::RateLimited,
                    "source retry window exceeds the process clock range",
                    false,
                ));
            }
            if source
                .cooldown
                .lock()
                .unwrap()
                .is_some_and(|until| until > Instant::now())
            {
                drop(global_permit);
                drop(source_permit);
                continue;
            }
            return Ok((source_permit, global_permit));
        }
    }
    fn failure_delay(&self, name: &str, failure: &SourceFailure, attempt: usize) -> Duration {
        let delay = self
            .limits
            .retry_backoff
            .saturating_mul(1u32 << (attempt - 1))
            .max(Duration::from_secs(
                failure.retry_after_seconds.unwrap_or(0),
            ));
        if failure.kind == SourceFailureKind::RateLimited {
            let source = self.source(name);
            if let Some(until) = Instant::now().checked_add(delay) {
                let mut cooldown = source.cooldown.lock().unwrap();
                *cooldown = Some(cooldown.map(|old| old.max(until)).unwrap_or(until));
            } else {
                source.blocked.store(true, Ordering::Release);
            }
        }
        delay
    }
    fn source(&self, name: &str) -> Arc<SourceResources> {
        self.sources
            .lock()
            .unwrap()
            .entry(name.into())
            .or_insert_with(|| {
                let concurrency_limit = if matches!(name, "arxiv" | "crossref") {
                    1
                } else {
                    self.limits.concurrent_requests_per_source
                };
                Arc::new(SourceResources {
                    permits: Arc::new(Semaphore::new(concurrency_limit)),
                    concurrency_limit,
                    cooldown: Mutex::new(None),
                    blocked: AtomicBool::new(false),
                })
            })
            .clone()
    }
    fn reserve(self: &Arc<Self>, records: usize, bytes: usize) -> Option<Reservation> {
        if self
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                v.checked_add(bytes)
                    .filter(|v| *v <= self.limits.max_payload_bytes)
            })
            .is_err()
        {
            return None;
        }
        if self
            .records
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                v.checked_add(records)
                    .filter(|v| *v <= self.limits.max_records_total)
            })
            .is_err()
        {
            self.bytes.fetch_sub(bytes, Ordering::AcqRel);
            return None;
        }
        Some(Reservation {
            resources: self.clone(),
            records,
            bytes,
            committed: false,
        })
    }
}
struct Reservation {
    resources: Arc<Resources>,
    records: usize,
    bytes: usize,
    committed: bool,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.committed {
            self.resources
                .records
                .fetch_sub(self.records, Ordering::AcqRel);
            self.resources.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        }
    }
}

struct SourceSlot {
    provider: Arc<dyn LiteratureSource>,
    status: DiscoverySourceStatus,
    cursor: Option<String>,
    epoch: u64,
    cancel: Option<watch::Sender<bool>>,
}
struct State {
    sources: Vec<SourceSlot>,
    candidates: CandidateStore,
    events: VecDeque<DiscoveryEvent>,
    sequence: u64,
    cancelled: bool,
    completion_emitted: bool,
    retained_records: usize,
    retained_bytes: usize,
}
struct Session {
    id: String,
    request: DiscoveryRequest,
    created: Instant,
    created_at: String,
    expires_at: String,
    state: Mutex<State>,
    changed: watch::Sender<u64>,
    lifetime: watch::Sender<bool>,
    resources: Arc<Resources>,
}
impl Drop for Session {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        self.resources
            .records
            .fetch_sub(state.retained_records, Ordering::AcqRel);
        self.resources
            .bytes
            .fetch_sub(state.retained_bytes, Ordering::AcqRel);
    }
}
fn active(phase: DiscoverySourcePhase) -> bool {
    matches!(
        phase,
        DiscoverySourcePhase::Queued
            | DiscoverySourcePhase::Running
            | DiscoverySourcePhase::WaitingRetry
    )
}
impl Session {
    fn phase(&self, state: &State) -> DiscoverySessionPhase {
        if state.sources.iter().any(|s| active(s.status.phase)) {
            DiscoverySessionPhase::Running
        } else if state.cancelled {
            DiscoverySessionPhase::Cancelled
        } else {
            DiscoverySessionPhase::Idle
        }
    }
    fn snapshot(&self, state: &State) -> DiscoverySessionSnapshot {
        DiscoverySessionSnapshot {
            id: self.id.clone(),
            request: self.request.clone(),
            created_at: self.created_at.clone(),
            expires_at: self.expires_at.clone(),
            phase: self.phase(state),
            sequence: state.sequence,
            merged_candidates: state.candidates.len(),
            sources: state.sources.iter().map(|s| s.status.clone()).collect(),
        }
    }
    fn emit(&self, state: &mut State, payload: DiscoveryEventPayload) {
        state.sequence += 1;
        state.events.push_back(DiscoveryEvent {
            sequence: state.sequence,
            emitted_at: Utc::now().to_rfc3339(),
            payload,
        });
        while state.events.len() > self.resources.limits.retained_events {
            state.events.pop_front();
        }
        self.changed.send_replace(state.sequence);
    }
    fn complete_if_idle(&self, state: &mut State) {
        let phase = self.phase(state);
        if phase != DiscoverySessionPhase::Running && !state.completion_emitted {
            state.completion_emitted = true;
            self.emit(state, DiscoveryEventPayload::Completed { phase });
        }
    }
}

struct Inner {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    cancelled_creations: Mutex<HashMap<String, Instant>>,
    resources: Arc<Resources>,
    janitor_started: AtomicBool,
}
impl Drop for Inner {
    fn drop(&mut self) {
        for session in self
            .sessions
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            session.lifetime.send_replace(true);
        }
    }
}
#[derive(Clone)]
pub struct DiscoverySessions {
    inner: Arc<Inner>,
}
impl Default for DiscoverySessions {
    fn default() -> Self {
        Self::new(Default::default()).expect("valid default discovery limits")
    }
}
impl DiscoverySessions {
    /// One persistent-background page, using the exact same global/source
    /// permits and cooldown as interactive sessions. Persistence and retry
    /// scheduling belong to the caller; dropping the future cancels queued I/O.
    pub async fn execute_planned_page(
        &self,
        gateway: Arc<LiteratureGateway>,
        plan: SourcePlan,
        cursor: Option<String>,
        deadline: Instant,
    ) -> std::result::Result<SourcePage, SourceFailure> {
        let timeout = || {
            SourceFailure::new(
                SourceFailureKind::Timeout,
                "source page deadline expired, including queue and cooldown time",
                true,
            )
        };
        if Instant::now() >= deadline {
            return Err(timeout());
        }
        let provider = gateway
            .discovery_sources()
            .into_iter()
            .find(|p| p.name() == plan.source)
            .ok_or_else(|| {
                SourceFailure::new(
                    SourceFailureKind::Unsupported,
                    "source is not registered",
                    false,
                )
            })?;
        if !(1..=100).contains(&plan.page_size) {
            return Err(SourceFailure::new(
                SourceFailureKind::InvalidQuery,
                "invalid planned page size",
                false,
            ));
        }
        let operation = async {
            let (_source, _global) = self.inner.resources.acquire(&plan.source).await?;
            if Instant::now() >= deadline {
                return Err(timeout());
            }
            let result = provider
                .discovery_page(&plan, cursor.as_deref(), plan.page_size)
                .await;
            if let Err(error) = &result {
                self.inner.resources.failure_delay(&plan.source, error, 1);
            }
            result
        };
        match tokio::time::timeout_at(
            deadline.into(),
            std::panic::AssertUnwindSafe(operation).catch_unwind(),
        )
        .await
        {
            Err(_) => Err(timeout()),
            Ok(Err(_)) => Err(SourceFailure::new(
                SourceFailureKind::InvalidResponse,
                "source provider panicked",
                false,
            )),
            Ok(Ok(result)) => result,
        }
    }
    pub fn source_runtime(&self) -> Vec<DiscoverySourceRuntime> {
        let mut status = self
            .inner
            .resources
            .sources
            .lock()
            .unwrap()
            .iter()
            .map(|(name, source)| {
                let remaining = source
                    .cooldown
                    .lock()
                    .unwrap()
                    .map(|until| {
                        until
                            .saturating_duration_since(Instant::now())
                            .as_millis()
                            .min(u128::from(u64::MAX)) as u64
                    })
                    .unwrap_or(0);
                DiscoverySourceRuntime {
                    source: name.clone(),
                    occupied_slots: source.concurrency_limit - source.permits.available_permits(),
                    concurrency_limit: source.concurrency_limit,
                    cooldown_remaining_ms: remaining,
                    blocked: source.blocked.load(Ordering::Acquire),
                }
            })
            .collect::<Vec<_>>();
        status.sort_by(|a, b| a.source.cmp(&b.source));
        status
    }
    pub fn new(limits: DiscoveryLimits) -> Result<Self> {
        if limits.max_sessions == 0
            || limits.ttl.is_zero()
            || limits.max_records_per_session == 0
            || limits.max_records_total == 0
            || limits.max_payload_bytes == 0
            || limits.retained_events == 0
            || limits.concurrent_requests == 0
            || limits.concurrent_requests_per_source == 0
            || limits.max_attempts == 0
            || limits.max_attempts > 10
            || limits.ttl > Duration::from_secs(86400)
        {
            return Err(DiscoverySessionError::Invalid(
                "invalid discovery limits".into(),
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                sessions: Mutex::new(HashMap::new()),
                cancelled_creations: Mutex::new(HashMap::new()),
                resources: Arc::new(Resources {
                    permits: Arc::new(Semaphore::new(limits.concurrent_requests)),
                    sources: Mutex::new(HashMap::new()),
                    records: AtomicUsize::new(0),
                    bytes: AtomicUsize::new(0),
                    limits,
                }),
                janitor_started: AtomicBool::new(false),
            }),
        })
    }
    fn cleanup(&self) {
        let ttl = self.inner.resources.limits.ttl;
        self.inner.sessions.lock().unwrap().retain(|_, session| {
            if session.created.elapsed() >= ttl {
                session.lifetime.send_replace(true);
                false
            } else {
                true
            }
        });
        self.inner
            .cancelled_creations
            .lock()
            .unwrap()
            .retain(|_, when| when.elapsed() < Duration::from_secs(60));
    }
    fn get(&self, id: &str) -> Result<Arc<Session>> {
        self.cleanup();
        self.inner
            .sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or(DiscoverySessionError::NotFound)
    }
    fn ensure_janitor(&self) {
        if self.inner.janitor_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        let interval = self
            .inner
            .resources
            .limits
            .ttl
            .min(Duration::from_secs(30))
            .max(Duration::from_millis(10));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                Self { inner }.cleanup();
            }
        });
    }
    pub fn create(
        &self,
        gateway: Arc<LiteratureGateway>,
        request: DiscoveryRequest,
    ) -> Result<DiscoverySessionSnapshot> {
        self.create_with_id(gateway, request, None)
    }
    /// A client-known ID lets close/cancel win even before POST completes.
    pub fn create_with_id(
        &self,
        gateway: Arc<LiteratureGateway>,
        request: DiscoveryRequest,
        client_id: Option<&str>,
    ) -> Result<DiscoverySessionSnapshot> {
        let id = match client_id {
            Some(id) => uuid::Uuid::parse_str(id)
                .map_err(|_| {
                    DiscoverySessionError::Invalid("client session ID must be a UUID".into())
                })?
                .to_string(),
            None => uuid::Uuid::new_v4().to_string(),
        };
        self.cleanup();
        if self
            .inner
            .cancelled_creations
            .lock()
            .unwrap()
            .contains_key(&id)
        {
            return Err(DiscoverySessionError::Conflict(
                "session creation was cancelled; use a new ID".into(),
            ));
        }
        let existing = self.inner.sessions.lock().unwrap().get(&id).cloned();
        if let Some(existing) = existing {
            if existing.request != request {
                return Err(DiscoverySessionError::Conflict(
                    "session ID is already bound to another request".into(),
                ));
            }
            let state = existing.state.lock().unwrap();
            return Ok(existing.snapshot(&state));
        }
        let original_request = request.clone();
        let request = crate::discovery_plan::normalize(request)
            .map_err(|e| DiscoverySessionError::Invalid(e.message))?;
        let providers = gateway.discovery_sources();
        let mut slots = Vec::new();
        for source in &request.sources {
            let provider = providers
                .iter()
                .find(|provider| provider.name() == source)
                .cloned()
                .ok_or_else(|| {
                    DiscoverySessionError::Invalid(format!("unknown source: {source}"))
                })?;
            let plan = provider.plan_discovery(&request).and_then(|plan| {
                if plan.source != *source || plan.page_size != request.page_size {
                    Err(SourceFailure::new(
                        SourceFailureKind::InvalidResponse,
                        "source planner returned an inconsistent plan",
                        false,
                    ))
                } else {
                    Ok(plan)
                }
            });
            let (plan, failure) = match plan {
                Ok(plan) => (Some(plan), None),
                Err(error) => (None, Some(error)),
            };
            let phase = if failure.is_some() {
                DiscoverySourcePhase::Failed
            } else {
                DiscoverySourcePhase::Queued
            };
            let notices = plan.as_ref().map(|p| p.notices.clone()).unwrap_or_default();
            slots.push(SourceSlot {
                provider,
                status: DiscoverySourceStatus {
                    source: source.clone(),
                    phase,
                    plan,
                    failure,
                    total: None,
                    delivered_records: 0,
                    pages: 0,
                    has_more: false,
                    attempt: 0,
                    retry_at: None,
                    executed_parameters: Default::default(),
                    query_translation: None,
                    notices,
                },
                cursor: None,
                epoch: 0,
                cancel: None,
            });
        }
        let now = Utc::now();
        let ttl = chrono::Duration::from_std(self.inner.resources.limits.ttl)
            .map_err(|_| DiscoverySessionError::Invalid("invalid session TTL".into()))?;
        let session = Arc::new(Session {
            id,
            request: original_request,
            created: Instant::now(),
            created_at: now.to_rfc3339(),
            expires_at: (now + ttl).to_rfc3339(),
            state: Mutex::new(State {
                sources: slots,
                candidates: Default::default(),
                events: VecDeque::new(),
                sequence: 0,
                cancelled: false,
                completion_emitted: false,
                retained_records: 0,
                retained_bytes: 0,
            }),
            changed: watch::channel(0).0,
            lifetime: watch::channel(false).0,
            resources: self.inner.resources.clone(),
        });
        self.cleanup();
        {
            let mut sessions = self.inner.sessions.lock().unwrap();
            if self
                .inner
                .cancelled_creations
                .lock()
                .unwrap()
                .contains_key(&session.id)
            {
                return Err(DiscoverySessionError::Conflict(
                    "session creation was cancelled; use a new ID".into(),
                ));
            }
            if let Some(existing) = sessions.get(&session.id) {
                if existing.request != session.request {
                    return Err(DiscoverySessionError::Conflict(
                        "session ID is already bound to another request".into(),
                    ));
                }
                let state = existing.state.lock().unwrap();
                return Ok(existing.snapshot(&state));
            }
            if sessions.len() >= self.inner.resources.limits.max_sessions {
                return Err(DiscoverySessionError::Capacity(
                    "too many retained discovery sessions; close an old session".into(),
                ));
            }
            sessions.insert(session.id.clone(), session.clone());
        }
        self.ensure_janitor();
        let mut jobs = Vec::new();
        let snapshot = {
            let mut state = session.state.lock().unwrap();
            session.emit(&mut state, DiscoveryEventPayload::Started);
            for index in 0..state.sources.len() {
                if let Some(failure) = state.sources[index].status.failure.clone() {
                    let source = state.sources[index].status.source.clone();
                    session.emit(
                        &mut state,
                        DiscoveryEventPayload::SourceFailed { source, failure },
                    );
                } else {
                    jobs.push(Self::prepare(&session, &mut state, index));
                }
            }
            session.complete_if_idle(&mut state);
            session.snapshot(&state)
        };
        for job in jobs {
            tokio::spawn(job.run());
        }
        Ok(snapshot)
    }
    fn prepare(session: &Arc<Session>, state: &mut State, index: usize) -> Job {
        state.cancelled = false;
        state.completion_emitted = false;
        let slot = &mut state.sources[index];
        slot.epoch += 1;
        let (cancel, receiver) = watch::channel(false);
        slot.cancel = Some(cancel);
        slot.status.phase = DiscoverySourcePhase::Queued;
        slot.status.failure = None;
        slot.status.retry_at = None;
        slot.status.attempt = 0;
        Job {
            session: session.clone(),
            index,
            epoch: slot.epoch,
            provider: slot.provider.clone(),
            plan: slot.status.plan.clone().expect("only planned sources run"),
            cursor: slot.cursor.clone(),
            cancel: receiver,
            lifetime: session.lifetime.subscribe(),
            deadline: Instant::now()
                + Duration::from_millis(
                    session
                        .request
                        .source_timeout_ms
                        .min(session.request.deadline_ms),
                ),
        }
    }
    pub fn snapshot(&self, id: &str) -> Result<DiscoverySessionSnapshot> {
        let session = self.get(id)?;
        let state = session.state.lock().unwrap();
        Ok(session.snapshot(&state))
    }
    pub fn next_page(&self, id: &str, source: &str) -> Result<DiscoverySessionSnapshot> {
        self.schedule(id, source, false)
    }
    pub fn retry(&self, id: &str, source: &str) -> Result<DiscoverySessionSnapshot> {
        self.schedule(id, source, true)
    }
    fn schedule(&self, id: &str, source: &str, retry: bool) -> Result<DiscoverySessionSnapshot> {
        let session = self.get(id)?;
        let (job, snapshot) = {
            let mut state = session.state.lock().unwrap();
            let index = state
                .sources
                .iter()
                .position(|s| s.status.source == source)
                .ok_or_else(|| {
                    DiscoverySessionError::Invalid("source is outside this session".into())
                })?;
            let slot = &state.sources[index];
            if slot.status.plan.is_none() {
                return Err(DiscoverySessionError::Conflict(
                    "this source's query is unsupported; change the search".into(),
                ));
            }
            if active(slot.status.phase) {
                return Err(DiscoverySessionError::Conflict(
                    "source already has an operation in progress".into(),
                ));
            }
            if retry {
                if !matches!(
                    slot.status.phase,
                    DiscoverySourcePhase::Failed | DiscoverySourcePhase::Cancelled
                ) {
                    return Err(DiscoverySessionError::Conflict(
                        "retry requires a failed or cancelled source".into(),
                    ));
                }
            } else if slot.status.phase != DiscoverySourcePhase::Ready || slot.cursor.is_none() {
                return Err(DiscoverySessionError::Conflict(
                    "source has no next page".into(),
                ));
            }
            let job = Self::prepare(&session, &mut state, index);
            let snapshot = session.snapshot(&state);
            (job, snapshot)
        };
        tokio::spawn(job.run());
        Ok(snapshot)
    }
    pub fn cancel(&self, id: &str, source: Option<&str>) -> Result<DiscoverySessionSnapshot> {
        let session = self.get(id)?;
        let mut state = session.state.lock().unwrap();
        if source.is_some_and(|source| !state.sources.iter().any(|s| s.status.source == source)) {
            return Err(DiscoverySessionError::Invalid(
                "source is outside this session".into(),
            ));
        }
        let newly_cancelled = source.is_none() && !state.cancelled;
        if source.is_none() {
            state.cancelled = true;
        }
        let mut cancelled = Vec::new();
        for slot in &mut state.sources {
            if source.is_some_and(|source| slot.status.source != source) {
                continue;
            }
            if active(slot.status.phase) || slot.status.phase == DiscoverySourcePhase::Ready {
                slot.epoch += 1;
                if let Some(cancel) = slot.cancel.take() {
                    cancel.send_replace(true);
                }
                slot.status.phase = DiscoverySourcePhase::Cancelled;
                slot.status.retry_at = None;
                slot.status.failure = Some(SourceFailure::new(
                    SourceFailureKind::Cancelled,
                    "source operation cancelled",
                    true,
                ));
                cancelled.push(slot.status.source.clone());
            }
        }
        for source in cancelled {
            session.emit(
                &mut state,
                DiscoveryEventPayload::SourceCancelled { source },
            );
        }
        if newly_cancelled {
            state.completion_emitted = false;
        }
        session.complete_if_idle(&mut state);
        Ok(session.snapshot(&state))
    }
    pub fn delete(&self, id: &str) -> Result<()> {
        self.cleanup();
        let id = uuid::Uuid::parse_str(id)
            .map_err(|_| DiscoverySessionError::NotFound)?
            .to_string();
        let mut sessions = self.inner.sessions.lock().unwrap();
        let mut cancelled = self.inner.cancelled_creations.lock().unwrap();
        if cancelled.len() >= 1024 && !cancelled.contains_key(&id) {
            if let Some(oldest) = cancelled
                .iter()
                .min_by_key(|(_, when)| **when)
                .map(|(key, _)| key.clone())
            {
                cancelled.remove(&oldest);
            }
        }
        cancelled.insert(id.clone(), Instant::now());
        if let Some(session) = sessions.remove(&id) {
            session.lifetime.send_replace(true);
        }
        Ok(())
    }
    fn events(session: &Session, after: u64) -> Result<DiscoveryEventPage> {
        let state = session.state.lock().unwrap();
        if after > state.sequence {
            return Err(DiscoverySessionError::Invalid(
                "event cursor is ahead of this session".into(),
            ));
        }
        let reset_required = state
            .events
            .front()
            .is_some_and(|event| after.saturating_add(1) < event.sequence);
        Ok(DiscoveryEventPage {
            events: if reset_required {
                Vec::new()
            } else {
                state
                    .events
                    .iter()
                    .filter(|event| event.sequence > after)
                    .cloned()
                    .collect()
            },
            sequence: state.sequence,
            reset_required,
        })
    }
    pub async fn events_after(
        &self,
        id: &str,
        after: u64,
        wait: Duration,
    ) -> Result<DiscoveryEventPage> {
        let session = self.get(id)?;
        let mut changed = session.changed.subscribe();
        let mut lifetime = session.lifetime.subscribe();
        if *lifetime.borrow() {
            return Err(DiscoverySessionError::NotFound);
        }
        let page = Self::events(&session, after)?;
        let running = {
            let state = session.state.lock().unwrap();
            session.phase(&state) == DiscoverySessionPhase::Running
        };
        if page.reset_required || !page.events.is_empty() || !running || wait.is_zero() {
            return Ok(page);
        }
        tokio::select! {biased;
            _=lifetime.changed()=>return Err(DiscoverySessionError::NotFound),
            _=tokio::time::sleep(wait.min(Duration::from_secs(25)))=>{},
            _=changed.changed()=>{},
        }
        if *session.lifetime.borrow() || session.created.elapsed() >= session.resources.limits.ttl {
            return Err(DiscoverySessionError::NotFound);
        }
        Self::events(&session, after)
    }
    pub fn results(
        &self,
        id: &str,
        source: Option<&str>,
        order: DiscoveryResultOrder,
        offset: usize,
        limit: usize,
        expected_sequence: Option<u64>,
    ) -> Result<DiscoveryResultPage> {
        if !(1..=100).contains(&limit) {
            return Err(DiscoverySessionError::Invalid(
                "result limit must be between 1 and 100".into(),
            ));
        }
        let session = self.get(id)?;
        let state = session.state.lock().unwrap();
        if expected_sequence.is_some_and(|expected| expected != state.sequence) {
            return Err(DiscoverySessionError::Conflict(
                "discovery results changed; refresh the result page".into(),
            ));
        }
        if source.is_some_and(|source| !state.sources.iter().any(|s| s.status.source == source)) {
            return Err(DiscoverySessionError::Invalid(
                "source is outside this session".into(),
            ));
        }
        let (candidates, total) = state.candidates.results(source, order, offset, limit);
        let end = offset.saturating_add(candidates.len());
        Ok(DiscoveryResultPage {
            candidates,
            obtained_candidates: total,
            offset,
            next_offset: if end < total { Some(end) } else { None },
            sequence: state.sequence,
        })
    }
    /// Capture user-visible source status and candidates under one session
    /// lock. Independent snapshot/results calls could mix distinct sequences.
    pub fn capture_obtained(
        &self,
        id: &str,
        expected_sequence: u64,
        order: DiscoveryResultOrder,
        max_items: usize,
    ) -> Result<(DiscoverySessionSnapshot, DiscoveryResultPage)> {
        if !(1..=10_000).contains(&max_items) {
            return Err(DiscoverySessionError::Invalid(
                "capture limit must be 1..10000".into(),
            ));
        }
        let session = self.get(id)?;
        let state = session.state.lock().unwrap();
        if state.sequence != expected_sequence {
            return Err(DiscoverySessionError::Conflict(
                "discovery results changed; refresh before saving".into(),
            ));
        }
        let (candidates, total) = state.candidates.results(None, order, 0, max_items);
        let end = candidates.len();
        Ok((
            session.snapshot(&state),
            DiscoveryResultPage {
                candidates,
                obtained_candidates: total,
                offset: 0,
                next_offset: (end < total).then_some(end),
                sequence: state.sequence,
            },
        ))
    }
    pub fn candidate(&self, id: &str, candidate: &str) -> Result<DiscoveryCandidate> {
        let session = self.get(id)?;
        let state = session.state.lock().unwrap();
        state
            .candidates
            .candidate(candidate)
            .ok_or(DiscoverySessionError::NotFound)
    }
    pub fn import_records(
        &self,
        id: &str,
        selections: &[DiscoveryImportSelection],
    ) -> Result<Vec<DiscoveryImportRecord>> {
        if selections.is_empty() || selections.len() > 100 {
            return Err(DiscoverySessionError::Invalid(
                "select between 1 and 100 candidates".into(),
            ));
        }
        let session = self.get(id)?;
        let state = session.state.lock().unwrap();
        let mut seen = std::collections::HashSet::new();
        let mut records = Vec::new();
        for selection in selections {
            let candidate = state
                .candidates
                .candidate(&selection.candidate_id)
                .ok_or(DiscoverySessionError::NotFound)?;
            if candidate.fingerprint != selection.expected_fingerprint {
                return Err(DiscoverySessionError::Conflict(
                    "candidate metadata changed; refresh the import preview".into(),
                ));
            }
            if !seen.insert(candidate.id.clone()) {
                return Err(DiscoverySessionError::Invalid(
                    "candidate is selected more than once".into(),
                ));
            }
            let selected = candidate
                .records
                .get(selection.record_index)
                .cloned()
                .ok_or_else(|| {
                    DiscoverySessionError::Invalid("source record selection is out of range".into())
                })?;
            records.push(DiscoveryImportRecord {
                schema_version: 1,
                session_id: session.id.clone(),
                candidate_id: candidate.id,
                fingerprint: candidate.fingerprint,
                selected,
                records: candidate.records,
                request: session.request.clone(),
                plans: state
                    .sources
                    .iter()
                    .filter_map(|source| source.status.plan.clone())
                    .collect(),
            });
        }
        Ok(records)
    }
}

struct Job {
    session: Arc<Session>,
    index: usize,
    epoch: u64,
    provider: Arc<dyn LiteratureSource>,
    plan: SourcePlan,
    cursor: Option<String>,
    cancel: watch::Receiver<bool>,
    lifetime: watch::Receiver<bool>,
    deadline: Instant,
}
impl Job {
    fn publish_attempt(&self, attempt: usize, retry: Option<(SourceFailure, Duration)>) -> bool {
        let mut state = self.session.state.lock().unwrap();
        let slot = &mut state.sources[self.index];
        if slot.epoch != self.epoch || *self.session.lifetime.borrow() {
            return false;
        }
        slot.status.attempt = attempt;
        let payload = if let Some((failure, delay)) = retry {
            let retry_at = (Utc::now()
                + chrono::Duration::from_std(delay).unwrap_or(chrono::Duration::MAX))
            .to_rfc3339();
            slot.status.phase = DiscoverySourcePhase::WaitingRetry;
            slot.status.failure = Some(failure.clone());
            slot.status.retry_at = Some(retry_at.clone());
            DiscoveryEventPayload::SourceRetrying {
                source: self.plan.source.clone(),
                attempt,
                failure,
                retry_at,
            }
        } else {
            slot.status.phase = DiscoverySourcePhase::Running;
            slot.status.retry_at = None;
            DiscoveryEventPayload::SourceStarted {
                source: self.plan.source.clone(),
                attempt,
            }
        };
        self.session.emit(&mut state, payload);
        true
    }
    async fn operation(&self, deadline: Instant) -> std::result::Result<SourcePage, SourceFailure> {
        let resources = &self.session.resources;
        for attempt in 1..=resources.limits.max_attempts {
            let (_source, _global) = resources.acquire(&self.plan.source).await?;
            if !self.publish_attempt(attempt, None) {
                return Err(SourceFailure::new(
                    SourceFailureKind::Cancelled,
                    "source operation cancelled",
                    true,
                ));
            }
            let result = self
                .provider
                .discovery_page(&self.plan, self.cursor.as_deref(), self.plan.page_size)
                .await;
            drop(_global);
            drop(_source);
            match result {
                Ok(page) => return Ok(page),
                Err(failure) => {
                    let delay = resources.failure_delay(&self.plan.source, &failure, attempt);
                    if !failure.retryable
                        || attempt == resources.limits.max_attempts
                        || delay >= deadline.saturating_duration_since(Instant::now())
                    {
                        return Err(failure);
                    }
                    if !self.publish_attempt(attempt, Some((failure, delay))) {
                        return Err(SourceFailure::new(
                            SourceFailureKind::Cancelled,
                            "source operation cancelled",
                            true,
                        ));
                    }
                    tokio::time::sleep(delay).await;
                }
            }
        }
        unreachable!("max attempts is positive")
    }
    async fn run(mut self) {
        if *self.cancel.borrow() || *self.lifetime.borrow() {
            return;
        }
        let deadline = self.deadline;
        let budget = deadline.saturating_duration_since(Instant::now());
        if budget.is_zero() {
            self.finish(Err(SourceFailure::new(
                SourceFailureKind::Timeout,
                "source operation exceeded its query budget",
                true,
            )));
            return;
        }
        let result = {
            let operation = std::panic::AssertUnwindSafe(self.operation(deadline)).catch_unwind();
            let mut cancelled = self.cancel.clone();
            let mut lifetime = self.lifetime.clone();
            tokio::select! {biased;
                _=cancelled.changed()=>Err(SourceFailure::new(SourceFailureKind::Cancelled,"source operation cancelled",true)),
                _=lifetime.changed()=>Err(SourceFailure::new(SourceFailureKind::Cancelled,"discovery session closed",true)),
                _=tokio::time::sleep(budget)=>Err(SourceFailure::new(SourceFailureKind::Timeout,"source operation exceeded its query budget",true)),
                result=operation=>result.unwrap_or_else(|_|Err(SourceFailure::new(SourceFailureKind::Upstream,"source adapter failed unexpectedly",false))),
            }
        };
        self.finish(result);
    }
    fn finish(&mut self, result: std::result::Result<SourcePage, SourceFailure>) {
        let mut state = self.session.state.lock().unwrap();
        if state.sources[self.index].epoch != self.epoch || *self.session.lifetime.borrow() {
            return;
        }
        let result = result.and_then(|page| {
            if page.source != self.plan.source
                || page.articles.len() > usize::from(self.plan.page_size)
                || page
                    .articles
                    .iter()
                    .any(|article| article.id.trim().is_empty())
                || page.next_cursor.as_ref().is_some_and(|cursor| {
                    page.articles.is_empty() || Some(cursor.as_str()) == self.cursor.as_deref()
                })
                || page
                    .next_cursor
                    .as_ref()
                    .is_some_and(|cursor| cursor.len() > 32_768)
            {
                return Err(SourceFailure::new(
                    SourceFailureKind::InvalidResponse,
                    "source returned an invalid page",
                    false,
                ));
            }
            let records = page.articles.len();
            if state.retained_records.saturating_add(records)
                > self.session.resources.limits.max_records_per_session
            {
                return Err(SourceFailure::new(
                    SourceFailureKind::Capacity,
                    "session record limit reached; narrow the query or export this session",
                    false,
                ));
            }
            let bytes = serde_json::to_vec(&page)
                .map_err(|_| {
                    SourceFailure::new(
                        SourceFailureKind::InvalidResponse,
                        "source returned an invalid page payload",
                        false,
                    )
                })?
                .len();
            let reservation = self
                .session
                .resources
                .reserve(records, bytes)
                .ok_or_else(|| {
                    SourceFailure::new(
                        SourceFailureKind::Capacity,
                        "discovery cache budget reached; close another session and retry",
                        true,
                    )
                })?;
            Ok((page, reservation))
        });
        match result {
            Ok((page, mut reservation)) => {
                let rank = state.sources[self.index].status.delivered_records;
                let retrieved_at = Utc::now().to_rfc3339();
                let executed_parameters = page.executed_parameters.clone();
                let query_translation = page.query_translation.clone();
                let records = page
                    .articles
                    .into_iter()
                    .enumerate()
                    .map(|(i, article)| DiscoveryRecord {
                        source: self.plan.source.clone(),
                        article,
                        source_rank: rank + i + 1,
                        query_fingerprint: self.plan.fingerprint.clone(),
                        retrieved_at: retrieved_at.clone(),
                        executed_parameters: executed_parameters.clone(),
                        query_translation: query_translation.clone(),
                    })
                    .collect();
                let delta = state.candidates.ingest(records);
                let count = state.candidates.len();
                state.retained_records += reservation.records;
                state.retained_bytes += reservation.bytes;
                reservation.committed = true;
                let slot = &mut state.sources[self.index];
                slot.cursor = page.next_cursor;
                slot.status.phase = if slot.cursor.is_some() {
                    DiscoverySourcePhase::Ready
                } else {
                    DiscoverySourcePhase::Exhausted
                };
                slot.status.failure = None;
                slot.status.retry_at = None;
                slot.status.total = page.total;
                slot.status.delivered_records += reservation.records;
                slot.status.pages += 1;
                slot.status.has_more = slot.cursor.is_some();
                slot.status.executed_parameters = page.executed_parameters;
                slot.status.query_translation = page.query_translation;
                slot.status.notices = page.notices;
                slot.cancel = None;
                self.session.emit(
                    &mut state,
                    DiscoveryEventPayload::Batch {
                        source: self.plan.source.clone(),
                        upserted_ids: delta.upserted_ids,
                        removed_ids: delta.removed_ids,
                        merged_candidates: count,
                    },
                );
            }
            Err(failure) => {
                let slot = &mut state.sources[self.index];
                slot.status.phase = if failure.kind == SourceFailureKind::Cancelled {
                    DiscoverySourcePhase::Cancelled
                } else {
                    DiscoverySourcePhase::Failed
                };
                slot.status.failure = Some(failure.clone());
                slot.status.retry_at = None;
                slot.cancel = None;
                self.session.emit(
                    &mut state,
                    DiscoveryEventPayload::SourceFailed {
                        source: self.plan.source.clone(),
                        failure,
                    },
                );
            }
        }
        self.session.complete_if_idle(&mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::SourceBatch;
    use bib_types::{Article, IdKind, Identifier, StructuredSearch};
    use tokio::sync::Notify;

    enum Step {
        Page(SourcePage),
        Fail(SourceFailure),
        Hold(Arc<Notify>, SourcePage),
        Panic,
    }
    struct MockSource {
        name: &'static str,
        steps: Mutex<VecDeque<Step>>,
        calls: Mutex<Vec<Option<String>>>,
        dropped: Arc<AtomicUsize>,
        unsupported: bool,
    }
    impl MockSource {
        fn new(name: &'static str, steps: Vec<Step>) -> Arc<Self> {
            Arc::new(Self {
                name,
                steps: Mutex::new(steps.into()),
                calls: Mutex::new(Vec::new()),
                dropped: Arc::new(AtomicUsize::new(0)),
                unsupported: false,
            })
        }
        fn calls(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }
    struct DropWitness(Arc<AtomicUsize>);
    impl Drop for DropWitness {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }
    #[async_trait::async_trait]
    impl LiteratureSource for MockSource {
        fn name(&self) -> &'static str {
            self.name
        }
        fn plan_discovery(
            &self,
            request: &DiscoveryRequest,
        ) -> std::result::Result<SourcePlan, SourceFailure> {
            if self.unsupported {
                return Err(SourceFailure::new(
                    SourceFailureKind::Unsupported,
                    "unsupported fixture query",
                    false,
                ));
            }
            Ok(SourcePlan {
                source: self.name.into(),
                page_size: request.page_size,
                parameters: Default::default(),
                fingerprint: "fixture-query".into(),
                notices: Vec::new(),
            })
        }
        async fn discovery_page(
            &self,
            _: &SourcePlan,
            cursor: Option<&str>,
            _: u16,
        ) -> std::result::Result<SourcePage, SourceFailure> {
            self.calls.lock().unwrap().push(cursor.map(str::to_owned));
            let _drop = DropWitness(self.dropped.clone());
            let step = self
                .steps
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected fixture request");
            match step {
                Step::Page(page) => Ok(page),
                Step::Fail(error) => Err(error),
                Step::Hold(gate, page) => {
                    gate.notified().await;
                    Ok(page)
                }
                Step::Panic => panic!("fixture provider panic"),
            }
        }
        async fn search(&self, _: &StructuredSearch, _: usize) -> crate::Result<SourceBatch> {
            unreachable!()
        }
        async fn fetch(&self, _: &Identifier) -> crate::Result<Option<Article>> {
            unreachable!()
        }
    }
    fn page(source: &str, ids: &[&str], next: Option<&str>) -> SourcePage {
        SourcePage {
            source: source.into(),
            articles: ids
                .iter()
                .map(|id| {
                    let mut a = Article::new(*id, format!("Paper {id}"));
                    a.identifiers
                        .push(Identifier::new(IdKind::Doi, format!("10.1000/{id}")));
                    a
                })
                .collect(),
            total: Some(10),
            next_cursor: next.map(str::to_owned),
            executed_parameters: Default::default(),
            query_translation: None,
            notices: Vec::new(),
        }
    }
    fn gateway(sources: Vec<Arc<MockSource>>) -> Arc<LiteratureGateway> {
        Arc::new(
            sources
                .into_iter()
                .fold(LiteratureGateway::new(), |gateway, source| {
                    gateway.with_source(source)
                }),
        )
    }
    fn request(sources: &[&str]) -> DiscoveryRequest {
        DiscoveryRequest {
            query: "gene".into(),
            sources: sources.iter().map(|s| (*s).into()).collect(),
            page_size: 2,
            source_timeout_ms: 1000,
            deadline_ms: 1000,
            ..Default::default()
        }
    }
    async fn wait_until(mut test: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !test() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("fixture condition timed out");
    }
    async fn idle(manager: &DiscoverySessions, id: &str) -> DiscoverySessionSnapshot {
        wait_until(|| manager.snapshot(id).unwrap().phase != DiscoverySessionPhase::Running).await;
        manager.snapshot(id).unwrap()
    }
    #[tokio::test]
    async fn arxiv_sessions_share_one_slot_and_cancel_queued_work_without_starting_it() {
        let gate = Arc::new(Notify::new());
        let source = MockSource::new(
            "arxiv",
            vec![
                Step::Hold(gate.clone(), page("arxiv", &["one"], None)),
                Step::Page(page("arxiv", &["two"], None)),
            ],
        );
        let manager = DiscoverySessions::default();
        let gateway = gateway(vec![source.clone()]);
        let first = manager
            .create(gateway.clone(), request(&["arxiv"]))
            .unwrap();
        wait_until(|| source.calls() == 1).await;
        let second = manager.create(gateway, request(&["arxiv"])).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            source.calls(),
            1,
            "arXiv must not inherit the generic two-request source concurrency"
        );
        assert_eq!(manager.source_runtime()[0].concurrency_limit, 1);
        manager.cancel(&second.id, None).unwrap();
        gate.notify_one();
        idle(&manager, &first.id).await;
        assert_eq!(
            manager.snapshot(&second.id).unwrap().phase,
            DiscoverySessionPhase::Cancelled
        );
        assert_eq!(source.calls(), 1);
    }
    #[tokio::test]
    async fn persistent_pages_share_interactive_slots_and_queue_deadlines() {
        let gate = Arc::new(Notify::new());
        let source = MockSource::new(
            "arxiv",
            vec![
                Step::Hold(gate.clone(), page("arxiv", &["one"], None)),
                Step::Page(page("arxiv", &["two"], None)),
            ],
        );
        let manager = DiscoverySessions::default();
        let gateway = gateway(vec![source.clone()]);
        let interactive = manager
            .create(gateway.clone(), request(&["arxiv"]))
            .unwrap();
        wait_until(|| source.calls() == 1).await;
        let plan = source.plan_discovery(&request(&["arxiv"])).unwrap();
        let error = manager
            .execute_planned_page(
                gateway.clone(),
                plan.clone(),
                None,
                Instant::now() + Duration::from_millis(30),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind, SourceFailureKind::Timeout);
        assert_eq!(source.calls(), 1);
        gate.notify_one();
        idle(&manager, &interactive.id).await;
        let result = manager
            .execute_planned_page(gateway, plan, None, Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(result.articles.len(), 1);
        assert_eq!(source.calls(), 2);
    }
    #[tokio::test]
    async fn persistent_rate_limits_block_interactive_and_background_retries() {
        let mut error =
            SourceFailure::new(SourceFailureKind::RateLimited, "provider cooldown", true);
        error.retry_after_seconds = Some(60);
        let source = MockSource::new("arxiv", vec![Step::Fail(error)]);
        let manager = DiscoverySessions::default();
        let gateway = gateway(vec![source.clone()]);
        let plan = source.plan_discovery(&request(&["arxiv"])).unwrap();
        let result = manager
            .execute_planned_page(
                gateway.clone(),
                plan.clone(),
                None,
                Instant::now() + Duration::from_secs(1),
            )
            .await
            .unwrap_err();
        assert_eq!(result.kind, SourceFailureKind::RateLimited);
        let interactive = manager
            .create(gateway.clone(), request(&["arxiv"]))
            .unwrap();
        let result = manager
            .execute_planned_page(
                gateway,
                plan,
                None,
                Instant::now() + Duration::from_millis(30),
            )
            .await
            .unwrap_err();
        assert_eq!(result.kind, SourceFailureKind::Timeout);
        assert_eq!(source.calls(), 1);
        assert!(manager.source_runtime()[0].cooldown_remaining_ms > 0);
        manager.cancel(&interactive.id, None).unwrap();
    }
    #[tokio::test]
    async fn fast_source_delivers_before_slow_source_and_cancellation_keeps_results() {
        let gate = Arc::new(Notify::new());
        let fast = MockSource::new("fast", vec![Step::Page(page("fast", &["one"], None))]);
        let slow = MockSource::new(
            "slow",
            vec![Step::Hold(gate.clone(), page("slow", &["two"], None))],
        );
        let manager = DiscoverySessions::default();
        let started = manager
            .create(
                gateway(vec![fast.clone(), slow.clone()]),
                request(&["fast", "slow"]),
            )
            .unwrap();
        assert_eq!(started.request.query, "gene");
        wait_until(|| {
            manager.snapshot(&started.id).unwrap().merged_candidates == 1 && slow.calls() == 1
        })
        .await;
        assert_eq!(
            manager.snapshot(&started.id).unwrap().phase,
            DiscoverySessionPhase::Running
        );
        let events = manager
            .events_after(&started.id, 0, Duration::ZERO)
            .await
            .unwrap();
        assert!(events.events.iter().any(
            |e| matches!(&e.payload,DiscoveryEventPayload::Batch{source,..} if source=="fast")
        ));
        let cancelled = manager.cancel(&started.id, None).unwrap();
        assert_eq!(cancelled.phase, DiscoverySessionPhase::Cancelled);
        let seq = cancelled.sequence;
        assert_eq!(
            manager.cancel(&started.id, None).unwrap().sequence,
            seq,
            "cancel is idempotent"
        );
        wait_until(|| slow.dropped.load(Ordering::Acquire) == 1).await;
        gate.notify_waiters();
        tokio::task::yield_now().await;
        let results = manager
            .results(&started.id, None, Default::default(), 0, 10, None)
            .unwrap();
        assert_eq!(results.obtained_candidates, 1);
        assert_eq!(results.candidates[0].article.id, "one");
        assert_eq!(results.sequence, seq);
    }
    #[tokio::test]
    async fn automatic_retry_and_single_source_next_page_use_the_same_cursor() {
        let error = SourceFailure::new(SourceFailureKind::Transport, "transient", true);
        let source = MockSource::new(
            "source",
            vec![
                Step::Fail(error),
                Step::Page(page("source", &["one"], Some("cursor-one"))),
                Step::Page(page("source", &["one", "two"], None)),
            ],
        );
        let manager = DiscoverySessions::new(DiscoveryLimits {
            retry_backoff: Duration::from_millis(5),
            ..Default::default()
        })
        .unwrap();
        let started = manager
            .create(gateway(vec![source.clone()]), request(&["source"]))
            .unwrap();
        let first = idle(&manager, &started.id).await;
        assert_eq!(first.sources[0].phase, DiscoverySourcePhase::Ready);
        assert_eq!(first.sources[0].attempt, 2);
        let old = manager
            .results(&started.id, None, Default::default(), 0, 1, None)
            .unwrap();
        manager.next_page(&started.id, "source").unwrap();
        let final_state = idle(&manager, &started.id).await;
        assert_eq!(final_state.merged_candidates, 2);
        assert_eq!(final_state.sources[0].delivered_records, 3);
        assert_eq!(
            final_state.sources[0].phase,
            DiscoverySourcePhase::Exhausted
        );
        assert_eq!(
            *source.calls.lock().unwrap(),
            vec![None, None, Some("cursor-one".into())]
        );
        assert!(matches!(
            manager.results(
                &started.id,
                None,
                Default::default(),
                0,
                10,
                Some(old.sequence)
            ),
            Err(DiscoverySessionError::Conflict(_))
        ));
        let results = manager
            .results(&started.id, None, Default::default(), 0, 10, None)
            .unwrap();
        assert_eq!(results.candidates[0].records.len(), 1);
        assert_eq!(results.candidates[1].records[0].source_rank, 3);
        assert!(manager.next_page(&started.id, "source").is_err());
    }
    #[tokio::test]
    async fn source_failure_can_retry_without_replaying_other_sources() {
        let bad = MockSource::new(
            "bad",
            vec![
                Step::Fail(SourceFailure::new(
                    SourceFailureKind::Upstream,
                    "failed",
                    false,
                )),
                Step::Page(page("bad", &["two"], None)),
            ],
        );
        let good = MockSource::new("good", vec![Step::Page(page("good", &["one"], None))]);
        let manager = DiscoverySessions::default();
        let start = manager
            .create(
                gateway(vec![bad.clone(), good.clone()]),
                request(&["bad", "good"]),
            )
            .unwrap();
        let failed = idle(&manager, &start.id).await;
        assert_eq!(failed.sources[0].phase, DiscoverySourcePhase::Failed);
        manager.retry(&start.id, "bad").unwrap();
        assert_eq!(idle(&manager, &start.id).await.merged_candidates, 2);
        assert_eq!(good.calls(), 1);
        assert_eq!(bad.calls(), 2);
    }
    #[tokio::test]
    async fn queued_cancel_and_timeout_drop_futures_without_publishing_old_results() {
        let gate = Arc::new(Notify::new());
        let source = MockSource::new(
            "source",
            vec![Step::Hold(gate.clone(), page("source", &["one"], None))],
        );
        let manager = DiscoverySessions::new(DiscoveryLimits {
            concurrent_requests_per_source: 1,
            ..Default::default()
        })
        .unwrap();
        let g = gateway(vec![source.clone()]);
        let one = manager.create(g.clone(), request(&["source"])).unwrap();
        wait_until(|| source.calls() == 1).await;
        let two = manager.create(g.clone(), request(&["source"])).unwrap();
        manager.cancel(&two.id, None).unwrap();
        tokio::task::yield_now().await;
        assert_eq!(source.calls(), 1);
        let mut timed = request(&["source"]);
        timed.source_timeout_ms = 100;
        let timeout = manager.create(g, timed).unwrap();
        let result = idle(&manager, &timeout.id).await;
        assert_eq!(
            result.sources[0].failure.as_ref().unwrap().kind,
            SourceFailureKind::Timeout
        );
        assert_eq!(source.calls(), 1);
        manager.cancel(&one.id, None).unwrap();
        wait_until(|| source.dropped.load(Ordering::Acquire) == 1).await;
        assert_eq!(manager.snapshot(&two.id).unwrap().merged_candidates, 0);
    }
    #[tokio::test]
    async fn capacity_delete_expiry_and_event_history_are_bounded_and_explicit() {
        let source = MockSource::new(
            "source",
            vec![
                Step::Page(page("source", &["one"], None)),
                Step::Page(page("source", &["two"], None)),
                Step::Page(page("source", &["three"], None)),
            ],
        );
        let manager = DiscoverySessions::new(DiscoveryLimits {
            max_records_total: 1,
            retained_events: 2,
            ttl: Duration::from_secs(1),
            ..Default::default()
        })
        .unwrap();
        let g = gateway(vec![source]);
        let a = manager.create(g.clone(), request(&["source"])).unwrap();
        idle(&manager, &a.id).await;
        assert!(
            manager
                .events_after(&a.id, 0, Duration::ZERO)
                .await
                .unwrap()
                .reset_required
        );
        let b = manager.create(g.clone(), request(&["source"])).unwrap();
        let b_state = idle(&manager, &b.id).await;
        assert_eq!(
            b_state.sources[0].failure.as_ref().unwrap().kind,
            SourceFailureKind::Capacity
        );
        manager.delete(&a.id).unwrap();
        wait_until(|| manager.inner.resources.records.load(Ordering::Acquire) == 0).await;
        manager.retry(&b.id, "source").unwrap();
        assert_eq!(idle(&manager, &b.id).await.merged_candidates, 1);
        assert!(manager.candidate(&a.id, "missing").is_err());
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(matches!(
            manager.snapshot(&b.id),
            Err(DiscoverySessionError::NotFound)
        ));
        wait_until(|| manager.inner.resources.records.load(Ordering::Acquire) == 0).await;
    }
    #[tokio::test]
    async fn retry_after_is_shared_across_sessions_and_never_bypassed_by_manual_retry() {
        let mut limited = SourceFailure::new(SourceFailureKind::RateLimited, "cooldown", true);
        limited.retry_after_seconds = Some(1);
        let source = MockSource::new("source", vec![Step::Fail(limited)]);
        let manager = DiscoverySessions::default();
        let g = gateway(vec![source.clone()]);
        let mut r = request(&["source"]);
        r.source_timeout_ms = 100;
        r.deadline_ms = 100;
        let a = manager.create(g.clone(), r.clone()).unwrap();
        assert_eq!(
            idle(&manager, &a.id).await.sources[0]
                .failure
                .as_ref()
                .unwrap()
                .kind,
            SourceFailureKind::RateLimited
        );
        manager.retry(&a.id, "source").unwrap();
        let b = manager.create(g, r).unwrap();
        idle(&manager, &a.id).await;
        idle(&manager, &b.id).await;
        assert_eq!(
            source.calls(),
            1,
            "cooldown must prevent another HTTP start"
        );
    }
    #[tokio::test]
    async fn long_poll_wakes_on_a_batch_and_manager_drop_cancels_network_work() {
        let gate = Arc::new(Notify::new());
        let source = MockSource::new(
            "source",
            vec![
                Step::Hold(gate.clone(), page("source", &["one"], None)),
                Step::Hold(Arc::new(Notify::new()), page("source", &["two"], None)),
            ],
        );
        let manager = DiscoverySessions::default();
        let g = gateway(vec![source.clone()]);
        let a = manager.create(g.clone(), request(&["source"])).unwrap();
        wait_until(|| source.calls() == 1).await;
        let seq = manager.snapshot(&a.id).unwrap().sequence;
        let waiting = {
            let manager = manager.clone();
            let id = a.id.clone();
            tokio::spawn(async move {
                manager
                    .events_after(&id, seq, Duration::from_secs(1))
                    .await
                    .unwrap()
            })
        };
        gate.notify_one();
        let events = waiting.await.unwrap();
        assert!(
            events
                .events
                .iter()
                .any(|event| matches!(event.payload, DiscoveryEventPayload::Batch { .. }))
        );
        let _b = manager.create(g, request(&["source"])).unwrap();
        wait_until(|| source.calls() == 2).await;
        drop(manager);
        wait_until(|| source.dropped.load(Ordering::Acquire) == 2).await;
    }
    #[tokio::test]
    async fn provider_panics_and_invalid_continuations_become_source_failures() {
        let source = MockSource::new(
            "source",
            vec![
                Step::Panic,
                Step::Page(page("source", &["one"], Some("same"))),
                Step::Page(page("source", &["two"], Some("same"))),
            ],
        );
        let manager = DiscoverySessions::default();
        let a = manager
            .create(gateway(vec![source]), request(&["source"]))
            .unwrap();
        assert_eq!(
            idle(&manager, &a.id).await.sources[0]
                .failure
                .as_ref()
                .unwrap()
                .kind,
            SourceFailureKind::Upstream
        );
        manager.retry(&a.id, "source").unwrap();
        idle(&manager, &a.id).await;
        manager.next_page(&a.id, "source").unwrap();
        let state = idle(&manager, &a.id).await;
        assert_eq!(
            state.sources[0].failure.as_ref().unwrap().kind,
            SourceFailureKind::InvalidResponse
        );
        assert_eq!(state.merged_candidates, 1);
    }

    #[tokio::test]
    async fn cancellation_before_polling_and_queued_deadline_prevent_http_starts() {
        let source = MockSource::new("source", vec![Step::Page(page("source", &["one"], None))]);
        let manager = DiscoverySessions::default();
        let g = gateway(vec![source.clone()]);
        let cancelled = manager.create(g.clone(), request(&["source"])).unwrap();
        manager.cancel(&cancelled.id, None).unwrap();
        tokio::task::yield_now().await;
        assert_eq!(source.calls(), 0);
        let mut r = request(&["source"]);
        r.source_timeout_ms = 100;
        let queued = manager.create(g, r).unwrap();
        // Delay the worker's first poll beyond its submission deadline.
        std::thread::sleep(Duration::from_millis(120));
        let state = idle(&manager, &queued.id).await;
        assert_eq!(
            state.sources[0].failure.as_ref().unwrap().kind,
            SourceFailureKind::Timeout
        );
        assert_eq!(source.calls(), 0);
    }

    #[tokio::test]
    async fn unsupported_sources_finish_explicitly_without_network_and_cooldown_is_observable() {
        let mut source = MockSource::new("source", Vec::new());
        Arc::get_mut(&mut source).unwrap().unsupported = true;
        let manager = DiscoverySessions::default();
        let state = manager
            .create(gateway(vec![source.clone()]), request(&["source"]))
            .unwrap();
        assert_eq!(state.phase, DiscoverySessionPhase::Idle);
        assert_eq!(
            state.sources[0].failure.as_ref().unwrap().kind,
            SourceFailureKind::Unsupported
        );
        assert_eq!(source.calls(), 0);
        assert!(manager.retry(&state.id, "source").is_err());
        let mut error = SourceFailure::new(SourceFailureKind::RateLimited, "cooldown", true);
        error.retry_after_seconds = Some(1);
        let limited = MockSource::new("limited", vec![Step::Fail(error)]);
        let mut r = request(&["limited"]);
        r.source_timeout_ms = 100;
        let state = manager.create(gateway(vec![limited]), r).unwrap();
        idle(&manager, &state.id).await;
        let status = manager.source_runtime();
        assert_eq!(status[0].source, "limited");
        assert!(status[0].cooldown_remaining_ms > 0);
        assert_eq!(status[0].occupied_slots, 0);
    }

    #[tokio::test]
    async fn client_known_ids_are_idempotent_and_early_close_prevents_orphans() {
        let source = MockSource::new("source", vec![Step::Page(page("source", &["one"], None))]);
        let g = gateway(vec![source.clone()]);
        let manager = DiscoverySessions::default();
        let cancelled = uuid::Uuid::new_v4().to_string();
        manager.delete(&cancelled).unwrap();
        manager.delete(&cancelled).unwrap();
        assert!(matches!(
            manager.create_with_id(g.clone(), request(&["source"]), Some(&cancelled)),
            Err(DiscoverySessionError::Conflict(_))
        ));
        assert_eq!(source.calls(), 0);
        let id = uuid::Uuid::new_v4().to_string();
        let r = request(&["source"]);
        let first = manager
            .create_with_id(g.clone(), r.clone(), Some(&id))
            .unwrap();
        let second = manager
            .create_with_id(g.clone(), r.clone(), Some(&id))
            .unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(idle(&manager, &id).await.merged_candidates, 1);
        assert_eq!(source.calls(), 1);
        let mut changed = r.clone();
        changed.query = "other".into();
        assert!(matches!(
            manager.create_with_id(g.clone(), changed, Some(&id)),
            Err(DiscoverySessionError::Conflict(_))
        ));
        manager.delete(&id).unwrap();
        assert!(
            manager
                .create_with_id(g.clone(), r.clone(), Some(&id))
                .is_err()
        );
        assert!(matches!(
            manager.create_with_id(g, r, Some("invalid-id")),
            Err(DiscoverySessionError::Invalid(_))
        ));
        for _ in 0..1030 {
            manager.delete(&uuid::Uuid::new_v4().to_string()).unwrap();
        }
        assert_eq!(
            manager.inner.cancelled_creations.lock().unwrap().len(),
            1024
        );
    }
}
