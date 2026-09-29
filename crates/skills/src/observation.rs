//! Skill observations: the raw signal the distillation loop feeds on.
//!
//! An observation is one durable, reusable fact learned during work —
//! a failure and its fix, a caveat, a verified recipe. Two capture
//! channels feed the store:
//!
//! - **agent**: the `skill_observe` tool — the working agent records
//!   procedural knowledge in the moment (replaces EvoScientist's
//!   background memory-worker distillation with first-person capture;
//!   no auxiliary model, no transcript re-reading)
//! - **automatic**: the runtime records eval failures and workflow
//!   run failures as `failure` observations anchored on the failing
//!   node kind and error text
//!
//! Storage is one TOML file per observation under
//! `<state_dir>/skill-observations/`, keyed by a content hash of the
//! canonical record — recording the same fact twice is a no-op
//! (EvoScientist derives the same idempotency from content-hashed
//! memory files; here it is explicit and total).
//!
//! Deliberately absent: EvoScientist's LLM-built observation graph
//! (complements/contradicts/supersedes links). Clustering in
//! [`crate::distill`] keys on the structured `(node_kind, error
//! signature)` anchor instead — deterministic, free, and stable
//! where text similarity is not.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::SkillError;

/// Directory under the state dir holding observation files.
pub const OBSERVATIONS_DIR: &str = "skill-observations";

/// What kind of signal this observation carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKind {
    /// A failure occurred and this is what fixed (or caused) it.
    Failure,
    /// A verified recipe or constraint worth repeating.
    Recipe,
    /// A caveat: where a usual approach breaks.
    Caveat,
}

/// Where the observation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSource {
    Agent,
    Eval,
    WorkflowRun,
    Cli,
}

/// Input for recording an observation; the store derives the id.
#[derive(Debug, Clone)]
pub struct ObservationInput {
    pub kind: ObservationKind,
    pub source: ObservationSource,
    /// One line a future search would find; name the component.
    pub summary: String,
    /// The reusable pattern, fix, or condition — not a run transcript.
    pub body: String,
    /// Optional structural anchor: the DAG node kind involved.
    pub node_kind: Option<String>,
    /// Optional structural anchor: the error text signature involved.
    pub error: Option<String>,
}

/// One stored observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub created_at: i64,
    pub kind: ObservationKind,
    pub source: ObservationSource,
    pub summary: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Observation {
    /// Human-readable kind label for tool output.
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            ObservationKind::Failure => "failure",
            ObservationKind::Recipe => "recipe",
            ObservationKind::Caveat => "caveat",
        }
    }

    /// Build from input plus the derived id and timestamp.
    pub fn from_input(input: ObservationInput, created_at: i64) -> Self {
        let id = observation_id(input.kind, &input.summary, &input.body);
        Self {
            id,
            created_at,
            kind: input.kind,
            source: input.source,
            summary: input.summary,
            body: input.body,
            node_kind: input.node_kind,
            error: input.error,
        }
    }
}

/// Content-hash id: the same fact recorded twice yields the same id
/// and the second write is a no-op.
fn observation_id(kind: ObservationKind, summary: &str, body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind_name(kind).as_bytes());
    hasher.update(b"\0");
    hasher.update(summary.trim().as_bytes());
    hasher.update(b"\0");
    hasher.update(body.trim().as_bytes());
    format!("O-{}", hex(&hasher.finalize())[..16].to_string())
}

fn kind_name(kind: ObservationKind) -> &'static str {
    match kind {
        ObservationKind::Failure => "failure",
        ObservationKind::Recipe => "recipe",
        ObservationKind::Caveat => "caveat",
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The observation store: a directory of `<id>.toml` files.
#[derive(Debug, Clone)]
pub struct ObservationStore {
    root: PathBuf,
}

impl ObservationStore {
    /// Open (or lazily create) the store under
    /// `<state_dir>/skill-observations/`.
    pub fn open(state_dir: &Path) -> Self {
        Self {
            root: state_dir.join(OBSERVATIONS_DIR),
        }
    }

    /// Record one observation. Idempotent on content: an existing id
    /// is returned unchanged without rewriting the file.
    pub fn record(&self, input: ObservationInput) -> Result<Observation, SkillError> {
        let observation = Observation::from_input(input, unix_now());
        let path = self.root.join(format!("{}.toml", observation.id));
        if path.exists() {
            return Ok(observation);
        }
        std::fs::create_dir_all(&self.root)?;
        let text = toml::to_string_pretty(&observation)
            .map_err(|_| SkillError::BadManifest(path.clone()))?;
        std::fs::write(&path, text)?;
        Ok(observation)
    }

    /// Every stored observation, sorted by id (i.e. grouped by kind
    /// and content, stable across runs).
    pub fn list(&self) -> Vec<Observation> {
        let Ok(read_dir) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in read_dir.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".toml") || name.starts_with('.') {
                continue;
            }
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| toml::from_str::<Observation>(&t).map_err(|e| e.to_string()))
            {
                Ok(observation) => out.push(observation),
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = e,
                        "skipping malformed observation"
                    );
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(summary: &str, body: &str) -> ObservationInput {
        ObservationInput {
            kind: ObservationKind::Failure,
            source: ObservationSource::Agent,
            summary: summary.into(),
            body: body.into(),
            node_kind: Some("file_to_dataframe".into()),
            error: Some("schema mismatch".into()),
        }
    }

    #[test]
    fn recording_is_idempotent_on_content() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(tmp.path());
        let first = store.record(input("s", "b")).unwrap();
        let second = store.record(input(" s ", " b ")).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].id, first.id);
        assert!(first.id.starts_with("O-"));
    }

    #[test]
    fn different_content_gets_different_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(tmp.path());
        store.record(input("s", "b1")).unwrap();
        store.record(input("s", "b2")).unwrap();
        assert_eq!(store.list().len(), 2);
    }

    #[test]
    fn malformed_files_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(tmp.path());
        store.record(input("s", "b")).unwrap();
        std::fs::write(
            tmp.path().join(OBSERVATIONS_DIR).join("O-bad.toml"),
            "not = toml",
        )
        .unwrap();
        let listed = store.list();
        assert_eq!(listed.len(), 1);
    }
}
