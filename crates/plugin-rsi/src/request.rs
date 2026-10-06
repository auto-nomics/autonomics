use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Where a plugin-change request originated.
pub enum RequestSource {
    /// Explicit request from a human user or operator.
    User,
    /// Request inferred or submitted by an Agent during its work.
    Agent,
    /// Request generated from a failed workflow run.
    WorkflowRun,
    /// Request generated from a failed skill or plugin evaluation.
    Eval,
    /// Request distilled from a durable observation.
    Observation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// The kind of plugin change a request asks the system to consider.
pub enum RequestIntent {
    /// Expose a capability that is not currently available.
    NewNode,
    /// Improve an existing capability without changing its core contract.
    OptimizeNode,
    /// Repair an existing capability that is failing or incorrect.
    FixNode,
    /// Clarify documentation or an interface contract.
    ClarifyContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Lifecycle state of a request, independent of the plugin implementing it.
pub enum RequestStatus {
    /// Recorded and waiting for triage.
    Open,
    /// An RSI run has claimed the request.
    Working,
    /// A plugin has been submitted for review.
    ReviewPending,
    /// A reviewed plugin has resolved the request.
    Consumed,
    /// Rejected by triage or review; retained as negative feedback.
    Rejected,
}

/// A durable request for a plugin change.
///
/// A request describes demand and evidence. It intentionally does not define
/// implementation decisions such as node kinds, scripts, or environments; those
/// belong to the plugin workspace produced in response to the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestRecord {
    /// Content-derived stable identifier, assigned by the store.
    pub id: String,
    /// Unix timestamp assigned when the request was first recorded.
    pub created_at: i64,
    /// Channel that produced the request.
    pub source: RequestSource,
    /// Broad change intent used by later triage.
    pub intent: RequestIntent,
    /// One-line statement a future search can find.
    pub summary: String,
    /// Full demand, constraints, observed failure, or requested behavior.
    pub body: String,
    /// Existing plugin the request appears to target, when known.
    ///
    /// This is a triage hint rather than a binding implementation decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
    /// Observations, runs, evaluations, or other records supporting the request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
    /// Current request lifecycle state.
    pub status: RequestStatus,
}

/// Content-addressed storage for plugin-change requests.
#[derive(Debug, Clone)]
pub struct RequestStore {
    root: PathBuf,
}

impl RequestStore {
    /// Open the request store beneath `<state_dir>/plugin-rsi/requests`.
    pub fn open(state_dir: &Path) -> Self {
        Self {
            root: state_dir.join("plugin-rsi").join("requests"),
        }
    }

    /// Record a request, assigning its identity and initial status.
    ///
    /// Recording the same source, intent, summary, and body is idempotent.
    pub fn record(&self, input: RequestRecord) -> Result<RequestRecord> {
        let summary = input.summary.trim();
        let body = input.body.trim();
        if summary.is_empty() || body.is_empty() {
            return Err(Error::InvalidRequest(
                "summary and body are required".into(),
            ));
        }
        if summary.len() > 512 || body.len() > 16 * 1024 {
            return Err(Error::InvalidRequest(
                "summary must be <= 512 bytes and body <= 16 KiB".into(),
            ));
        }
        if let Some(name) = input.plugin_name.as_deref() {
            crate::validate_plugin_name(name)?;
        }

        let mut record = input.clone();
        record.summary = summary.to_string();
        record.body = body.to_string();
        record.id = request_id(input.source, input.intent, &record.summary, &record.body);
        record.created_at = unix_now();
        record.status = RequestStatus::Open;

        let path = self.root.join(format!("{}.toml", record.id));
        if path.exists() {
            let mut existing = self.find(&record.id)?.ok_or(Error::InvalidRequest(
                "existing request record is unreadable".into(),
            ))?;
            let mut changed = false;
            for evidence_id in record.evidence_ids {
                if !existing.evidence_ids.contains(&evidence_id) {
                    existing.evidence_ids.push(evidence_id);
                    changed = true;
                }
            }
            if changed {
                atomic_toml(&path, &existing)?;
            }
            return Ok(existing);
        }
        std::fs::create_dir_all(&self.root)?;
        atomic_toml(&path, &record)?;
        Ok(record)
    }

    /// List all readable requests sorted by id.
    pub fn list(&self) -> Vec<RequestRecord> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut records: Vec<RequestRecord> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.ends_with(".toml") || name.starts_with('.') {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Ok(record) = toml::from_str::<RequestRecord>(&text) {
                records.push(record);
            }
        }
        records.sort_by(|a, b| a.id.cmp(&b.id));
        records
    }

    /// Find one request by id.
    pub fn find(&self, id: &str) -> Result<Option<RequestRecord>> {
        let path = self.root.join(format!("{id}.toml"));
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|source| Error::ReadFile {
            path: path.clone(),
            source,
        })?;
        toml::from_str(&text)
            .map(Some)
            .map_err(|source| Error::ParseToml { path, source })
    }

    /// Atomically update one request's lifecycle state.
    pub fn set_status(&self, id: &str, status: RequestStatus) -> Result<RequestRecord> {
        let mut record = self
            .find(id)?
            .ok_or_else(|| Error::InvalidRequest(format!("unknown request {id:?}")))?;
        record.status = status;
        let path = self.root.join(format!("{id}.toml"));
        atomic_toml(&path, &record)?;
        Ok(record)
    }

    /// Return the subset of ids that cannot be resolved by this store.
    pub fn missing_ids(&self, ids: &BTreeSet<String>) -> Vec<String> {
        ids.iter()
            .filter(|id| self.find(id).map(|found| found.is_none()).unwrap_or(true))
            .cloned()
            .collect()
    }
}

fn request_id(source: RequestSource, intent: RequestIntent, summary: &str, body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_string(&(source, intent)).unwrap_or_default());
    hasher.update(b"\0");
    hasher.update(summary.trim().as_bytes());
    hasher.update(b"\0");
    hasher.update(body.trim().as_bytes());
    format!("R-{}", &hex(&hasher.finalize())[..16])
}

pub(crate) fn atomic_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(value).map_err(|source| Error::SerializeToml {
        path: path.to_path_buf(),
        source,
    })?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|source| Error::WriteFile {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| Error::WriteFile {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(summary: &str) -> RequestRecord {
        RequestRecord {
            id: String::new(),
            created_at: 0,
            source: RequestSource::User,
            intent: RequestIntent::NewNode,
            summary: summary.into(),
            body: "Create a deterministic file adapter.".into(),
            plugin_name: Some("demo-plugin".into()),
            evidence_ids: Vec::new(),
            status: RequestStatus::Open,
        }
    }

    #[test]
    fn request_is_content_addressed_and_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RequestStore::open(tmp.path());
        let first = store.record(input("Create demo")).unwrap();
        let second = store.record(input("Create demo")).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(store.list().len(), 1);

        let mut updated = first.clone();
        updated.evidence_ids.push("O-second".into());
        let merged = store.record(updated).unwrap();
        assert_eq!(merged.id, first.id);
        assert_eq!(merged.evidence_ids, vec!["O-second".to_string()]);
    }

    #[test]
    fn malformed_requests_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RequestStore::open(tmp.path());
        let mut bad = input("Create demo");
        bad.body = "  ".into();
        assert!(store.record(bad).is_err());
    }
}
