//! Durable subscription contracts. Enabling a rule or explicitly checking it is
//! required before network work; notifications never import canonical articles.
use bib_types::discovery::{DiscoveryRecord, DiscoveryRequest, SourceFailure, SourcePlan};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum SubscriptionError {
    #[error("invalid subscription: {0}")]
    Invalid(String),
    #[error("subscription conflict: {0}")]
    Conflict(String),
    #[error("subscription not found")]
    NotFound,
    #[error("subscription budget exceeded: {0}")]
    Budget(String),
    #[error("subscription run budget exhausted: {0}")]
    RunBudget(String),
    #[error(transparent)]
    Store(#[from] turso::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, SubscriptionError>;
pub(crate) fn invalid(message: &str) -> SubscriptionError {
    SubscriptionError::Invalid(message.into())
}
pub(crate) fn conflict(message: &str) -> SubscriptionError {
    SubscriptionError::Conflict(message.into())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubscriptionSpec {
    pub name: String,
    pub request: DiscoveryRequest,
    pub enabled: bool,
    pub interval_seconds: u32,
    pub initial_days: u16,
    pub overlap_days: u16,
    pub window_days: u16,
    pub max_pages_per_run: u16,
    pub max_records_per_run: u32,
    pub run_timeout_ms: u64,
}
impl Default for SubscriptionSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            request: DiscoveryRequest::default(),
            enabled: false,
            interval_seconds: 3600,
            initial_days: 30,
            overlap_days: 3,
            window_days: 1,
            max_pages_per_run: 20,
            max_records_per_run: 1000,
            run_timeout_ms: 120_000,
        }
    }
}
impl SubscriptionSpec {
    pub fn validate(mut self) -> Result<Self> {
        self.name = self.name.trim().into();
        if self.name.is_empty() || self.name.len() > 256 || self.name.chars().any(char::is_control)
        {
            return Err(invalid("name must contain 1..256 bytes without controls"));
        }
        if !(60..=2_678_400).contains(&self.interval_seconds)
            || !(1..=3650).contains(&self.initial_days)
            || self.overlap_days > 30
            || !(1..=31).contains(&self.window_days)
            || !(1..=1000).contains(&self.max_pages_per_run)
            || !(1..=100_000).contains(&self.max_records_per_run)
            || !(1000..=3_600_000).contains(&self.run_timeout_ms)
        {
            return Err(invalid(
                "schedule or execution budget outside supported bounds",
            ));
        }
        self.request = crate::discovery_plan::normalize(self.request)
            .map_err(|e| SubscriptionError::Invalid(e.message))?;
        if self.max_records_per_run < u32::from(self.request.page_size) {
            return Err(invalid(
                "run record budget must fit one complete source page",
            ));
        }
        if self.request.sources.iter().any(|s| {
            !matches!(
                s.as_str(),
                "pubmed" | "crossref" | "openalex" | "arxiv" | "semantic_scholar"
            )
        }) {
            return Err(invalid("subscription source is not supported"));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionWindowKind {
    PubmedCreatedOrRevised,
    CrossrefIndexed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionWindow {
    pub kind: SubscriptionWindowKind,
    /// Inclusive UTC millisecond bounds, frozen before the first request.
    pub from_ms: i64,
    pub through_ms: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionSourceState {
    pub source: String,
    pub sequence: u64,
    pub watermark_ms: Option<i64>,
    pub window: Option<SubscriptionWindow>,
    pub plan: Option<SourcePlan>,
    pub cursor: Option<String>,
    pub complete: bool,
    pub scanned_records: u64,
    pub last_checked_ms: Option<i64>,
    pub completed_epoch: u64,
    pub retry_at_ms: Option<i64>,
    pub last_error: Option<SourceFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_page: Option<SubscriptionPageReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<SubscriptionScan>,
}
impl SubscriptionSourceState {
    pub(crate) fn new(source: String) -> Self {
        Self {
            source,
            sequence: 0,
            watermark_ms: None,
            window: None,
            plan: None,
            cursor: None,
            complete: true,
            scanned_records: 0,
            last_checked_ms: None,
            completed_epoch: 0,
            retry_at_ms: None,
            last_error: None,
            last_page: None,
            scan: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionPageReceipt {
    pub expected_sequence: u64,
    pub sequence: u64,
    pub page_hash: String,
    pub new_records: usize,
    pub new_notifications: usize,
    pub duplicate_records: usize,
    pub complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionInboxItem {
    pub id: String,
    pub rule_id: String,
    pub rule_version: u64,
    pub group_id: String,
    pub first_matched_ms: i64,
    pub records: Vec<DiscoveryRecord>,
    /// Shared identifiers with conflicting identities or ambiguous bridges.
    pub related_groups: Vec<String>,
    pub read: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionRule {
    pub id: String,
    pub version: u64,
    pub spec: SubscriptionSpec,
    pub next_due_ms: i64,
    pub last_success_ms: Option<i64>,
    pub sources: Vec<SubscriptionSourceState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_wait: Option<SubscriptionResourceWait>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionResourceWait {
    pub since_ms: i64,
    pub reason: String,
    pub observed_epoch: i64,
}
/// A process-local capability. Every durable mutation rechecks all fields and
/// expiry in the same writer transaction; possession alone grants no lease.
#[derive(Debug, Clone)]
pub struct SubscriptionLease {
    pub rule_id: String,
    pub version: u64,
    pub owner: String,
    pub epoch: u64,
    pub expires_ms: i64,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubscriptionLimits {
    pub max_rules: u32,
    pub max_seen: u32,
    pub max_seen_bytes: u64,
    pub max_inbox: u32,
    pub max_inbox_bytes: u64,
    pub max_page_bytes: u32,
    pub max_checkpoints: u32,
    pub max_runs: u32,
    pub max_run_bytes: u32,
    pub max_history_bytes: u64,
}
impl Default for SubscriptionLimits {
    fn default() -> Self {
        Self {
            max_rules: 128,
            max_seen: 100_000,
            max_seen_bytes: 256 * 1024 * 1024,
            max_inbox: 10_000,
            max_inbox_bytes: 128 * 1024 * 1024,
            max_page_bytes: 2 * 1024 * 1024,
            max_checkpoints: 100_000,
            max_runs: 100_000,
            max_run_bytes: 4 * 1024 * 1024,
            max_history_bytes: 256 * 1024 * 1024,
        }
    }
}
impl SubscriptionLimits {
    pub fn validate(&self) -> Result<()> {
        if !(1..=10_000).contains(&self.max_rules)
            || !(1..=10_000_000).contains(&self.max_seen)
            || !(1..=1_000_000).contains(&self.max_inbox)
            || !(1..=1024_u64.pow(4)).contains(&self.max_seen_bytes)
            || !(1..=1024_u64.pow(4)).contains(&self.max_inbox_bytes)
            || !(1..=16 * 1024 * 1024).contains(&self.max_page_bytes)
            || !(1..=10_000_000).contains(&self.max_checkpoints)
            || !(1..=1_000_000).contains(&self.max_runs)
            || !(65536..=16 * 1024 * 1024).contains(&self.max_run_bytes)
            || !(65536..=1024_u64.pow(4)).contains(&self.max_history_bytes)
        {
            return Err(invalid("storage limits outside supported bounds"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionScanStrategy {
    PubmedCreatedOrRevised,
    CrossrefIndexed,
    BoundedReplay,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionScan {
    pub id: String,
    pub strategy: SubscriptionScanStrategy,
    pub started_ms: i64,
    pub target_ms: Option<i64>,
    pub next_from_ms: Option<i64>,
    pub done: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionRunState {
    Running,
    Complete,
    Partial,
    Failed,
    Cancelled,
    Interrupted,
}
impl SubscriptionRunState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionPendingPage {
    pub sequence: u64,
    pub cursor: Option<String>,
    pub page_size: u16,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionWindowExecution {
    pub plan: SourcePlan,
    pub window: Option<SubscriptionWindow>,
    pub initial_cursor: Option<String>,
    pub pages_started: u32,
    pub pages_committed: u32,
    pub records: u32,
    pub complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionRunSource {
    pub source: String,
    pub strategy: SubscriptionScanStrategy,
    pub target_ms: Option<i64>,
    pub scan_id: Option<String>,
    pub done: bool,
    pub pending: Option<SubscriptionPendingPage>,
    pub windows: Vec<SubscriptionWindowExecution>,
    pub error: Option<SourceFailure>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionRun {
    pub id: String,
    pub rule_id: String,
    pub rule_version: u64,
    pub lease_epoch: u64,
    pub spec: SubscriptionSpec,
    pub manual: bool,
    pub started_ms: i64,
    pub deadline_ms: i64,
    pub finished_ms: Option<i64>,
    pub state: SubscriptionRunState,
    pub reason: Option<String>,
    pub pages_started: u32,
    pub pages_committed: u32,
    pub records: u32,
    pub new_notifications: u32,
    pub reserved_records: u32,
    pub sources: Vec<SubscriptionRunSource>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionRunSummary {
    pub id: String,
    pub rule_id: String,
    pub rule_version: u64,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub state: SubscriptionRunState,
    pub pages_started: u32,
    pub pages_committed: u32,
    pub records: u32,
    pub new_notifications: u32,
}
