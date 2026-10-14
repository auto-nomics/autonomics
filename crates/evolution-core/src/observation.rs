//! Skill observations: the raw signal the distillation loop feeds on.
//!
//! An observation is one durable, reusable fact learned during work —
//! a failure and its fix, a caveat, a verified recipe. Two capture
//! channels feed the store:
//!
//! - **agent**: the `evo_observe` tool — the working agent records
//!   procedural knowledge in the moment (replaces EvoScientist's
//!   background memory-worker distillation with first-person capture;
//!   no auxiliary model, no transcript re-reading)
//! - **automatic**: the runtime records eval failures and workflow
//!   run failures as `failure` observations anchored on the failing
//!   node kind and error text
//!
//! Schema only lives here. The Turso database backend and the shared
//! handle are [`crate::storage::TursoObservationStorage`] and
//! [`crate::hub::ObservationHub`].
//!
//! Deliberately absent: EvoScientist's LLM-built observation graph
//! (complements/contradicts/supersedes links). Clustering in
//! [`crate::distill`] keys on the structured `(node_kind, error
//! signature)` anchor instead — deterministic, free, and stable
//! where text similarity is not.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Node-kind renames applied on load. When a stored observation carries
/// `node_kind` matching a tuple's left side, the in-memory value is
/// rewritten to the right side and the row updated, so subsequent reads
/// see the new name.
///
/// Rationale: distillation clusters on `(node_kind, signature, kind)` —
/// an observation anchored on a removed node kind contributes to
/// nothing. The id is content-hashed over `(kind, summary, body)`
/// alone, never over `node_kind`, so re-anchoring is safe and does
/// not collide with the deduplication the store already does.
///
/// One entry per historical rename. Add new entries here when retiring
/// a node kind rather than expecting callers to migrate.
pub(crate) const NODE_KIND_RENAMES: &[(&str, &str)] = &[
    // `literature_search` (the original tool-style name) was retired when
    // literature retrieval was migrated into DAG evidence nodes; the
    // canonical kind is now `source_literature`.
    ("literature_search", "source_literature"),
];

/// What kind of signal this observation carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

impl ObservationSource {
    pub fn into_str(&self) -> &str {
        match self {
            ObservationSource::Agent => "agent",
            ObservationSource::Eval => "eval",
            ObservationSource::WorkflowRun => "workflow_run",
            ObservationSource::Cli => "cli",
        }
    }
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

impl ObservationKind {
    /// Stable lowercase label for display, tool args, and rationales.
    pub fn label(&self) -> &'static str {
        match self {
            ObservationKind::Failure => "failure",
            ObservationKind::Recipe => "recipe",
            ObservationKind::Caveat => "caveat",
        }
    }
}

impl Observation {
    /// Human-readable kind label for tool output.
    pub fn kind_label(&self) -> &'static str {
        self.kind.label()
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
    let digest = hex(&hasher.finalize());
    format!("O-{}", &digest[..16])
}

fn kind_name(kind: ObservationKind) -> &'static str {
    kind.label()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::ObservationInput;
    use crate::storage::{ObservationStorage, TursoObservationStorage};

    #[tokio::test]
    async fn content_hashing_is_stable_across_input_padding() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = TursoObservationStorage::new(tmp.path());
        let first = storage
            .record(ObservationInput {
                kind: ObservationKind::Failure,
                source: ObservationSource::Agent,
                summary: "s".into(),
                body: "b".into(),
                node_kind: Some("file_to_dataframe".into()),
                error: Some("schema mismatch".into()),
            })
            .await
            .unwrap();
        let second = storage
            .record(ObservationInput {
                kind: ObservationKind::Failure,
                source: ObservationSource::Agent,
                summary: " s ".into(),
                body: " b ".into(),
                node_kind: Some("file_to_dataframe".into()),
                error: Some("schema mismatch".into()),
            })
            .await
            .unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(storage.list().await.len(), 1);
        assert!(first.id.starts_with("O-"));
    }

    #[tokio::test]
    async fn different_content_gets_different_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = TursoObservationStorage::new(tmp.path());
        for body in ["b1", "b2"] {
            storage
                .record(ObservationInput {
                    kind: ObservationKind::Failure,
                    source: ObservationSource::Agent,
                    summary: "s".into(),
                    body: body.into(),
                    node_kind: Some("file_to_dataframe".into()),
                    error: Some("schema mismatch".into()),
                })
                .await
                .unwrap();
        }
        assert_eq!(storage.list().await.len(), 2);
    }

    /// Pre-rename observations (e.g. anchored on the now-retired
    /// `literature_search` kind) get rewritten to the current DAG kind
    /// on first read, and the row is updated so the rename sticks
    /// across loads. The id is unaffected.
    #[tokio::test]
    async fn renames_literature_search_to_source_literature_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = TursoObservationStorage::new(tmp.path());
        // Record directly with the stale anchor — the store accepts any
        // node kind, standing in for a row written before the rename.
        let recorded = storage
            .record(ObservationInput {
                kind: ObservationKind::Recipe,
                source: ObservationSource::Agent,
                summary: "lit audit".into(),
                body: "audit body".into(),
                node_kind: Some("literature_search".into()),
                error: None,
            })
            .await
            .unwrap();

        // First list() returns the rewritten kind and persists it.
        let first = storage.list().await;
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, recorded.id);
        assert_eq!(first[0].node_kind.as_deref(), Some("source_literature"));

        // A fresh handle over the same database sees the rewritten row.
        let reopened = TursoObservationStorage::new(tmp.path());
        let second = reopened.list().await;
        assert_eq!(second[0].node_kind.as_deref(), Some("source_literature"));
    }
}
