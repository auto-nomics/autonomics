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
pub enum RequestSource {
    User,
    Agent,
    WorkflowRun,
    Eval,
    Observation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestIntent {
    NewNode,
    OptimizeNode,
    FixNode,
    ClarifyContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    Open,
    Working,
    ProposalPending,
    Consumed,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestRecord {
    pub id: String,
    pub created_at: i64,
    pub source: RequestSource,
    pub intent: RequestIntent,
    pub summary: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
    pub status: RequestStatus,
}

#[derive(Debug, Clone)]
pub struct RequestStore {
    root: PathBuf,
}

impl RequestStore {
    pub fn open(state_dir: &Path) -> Self {
        Self {
            root: state_dir.join("plugin-rsi").join("requests"),
        }
    }

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
            return self.find(&record.id)?.ok_or(Error::InvalidRequest(
                "existing request record is unreadable".into(),
            ));
        }
        std::fs::create_dir_all(&self.root)?;
        atomic_toml(&path, &record)?;
        Ok(record)
    }

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

    pub fn set_status(&self, id: &str, status: RequestStatus) -> Result<RequestRecord> {
        let mut record = self
            .find(id)?
            .ok_or_else(|| Error::InvalidRequest(format!("unknown request {id:?}")))?;
        record.status = status;
        let path = self.root.join(format!("{id}.toml"));
        atomic_toml(&path, &record)?;
        Ok(record)
    }

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
            node_kind: None,
            image_id: Some("demo".into()),
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
