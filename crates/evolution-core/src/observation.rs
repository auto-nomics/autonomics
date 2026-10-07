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

use crate::error::{Error, Result};

/// Directory under the state dir holding observation files.
pub const OBSERVATIONS_DIR: &str = "skill-observations";

/// Node-kind renames applied on load. When a stored observation carries
/// `node_kind` matching a tuple's left side, the in-memory value is
/// rewritten to the right side and the on-disk TOML is re-emitted
/// (atomically, via temp + rename) so subsequent reads see the new name.
///
/// Rationale: distillation clusters on `(node_kind, signature, kind)` —
/// an observation anchored on a removed node kind contributes to
/// nothing. The id is content-hashed over `(kind, summary, body)`
/// alone, never over `node_kind`, so re-anchoring is safe and does
/// not collide with the deduplication the store already does.
///
/// One entry per historical rename. Add new entries here when retiring
/// a node kind rather than expecting callers to migrate.
const NODE_KIND_RENAMES: &[(&str, &str)] = &[
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
    pub fn record(&self, input: ObservationInput) -> Result<Observation> {
        let observation = Observation::from_input(input, unix_now());
        let path = self.root.join(format!("{}.toml", observation.id));
        if path.exists() {
            return Ok(observation);
        }
        std::fs::create_dir_all(&self.root)?;
        let text = toml::to_string_pretty(&observation).map_err(|source| Error::Serialize {
            path: path.clone(),
            source,
        })?;
        std::fs::write(&path, text).map_err(|source| Error::Write {
            path: path.clone(),
            source,
        })?;
        Ok(observation)
    }

    /// Every stored observation, sorted by id (i.e. grouped by kind
    /// and content, stable across runs). Applies [`NODE_KIND_RENAMES`]
    /// on load: an observation whose `node_kind` matches a stale entry
    /// has its in-memory value rewritten and the on-disk TOML
    /// re-emitted (atomically, via temp + rename) so the rename is
    /// sticky across the next read.
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
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "skipping unreadable observation"
                    );
                    continue;
                }
            };
            let mut observation: Observation = match toml::from_str(&text) {
                Ok(o) => o,
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "skipping malformed observation"
                    );
                    continue;
                }
            };
            // On-load node_kind migration: rewrite stale anchors in the
            // data, then re-emit the file so the rename sticks.
            if let Some(rewritten) = rewrite_node_kind(&mut observation) {
                tracing::warn!(
                    id = %observation.id,
                    old = %rewritten.0,
                    new = %rewritten.1,
                    "rewrote skill observation node_kind to current DAG kind"
                );
                if let Err(error) = atomic_write_toml(&path, &observation) {
                    tracing::warn!(
                        path = %path.display(),
                        error = %error,
                        "failed to persist rewritten node_kind; in-memory value still applied"
                    );
                }
            }
            out.push(observation);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }
}

/// If `observation.node_kind` matches a stale entry in
/// [`NODE_KIND_RENAMES`], mutate it to the current DAG kind and return
/// the (old, new) pair so the caller can log. Returns `None` when no
/// rewrite was needed.
fn rewrite_node_kind(observation: &mut Observation) -> Option<(&'static str, &'static str)> {
    let old = observation.node_kind.as_deref()?;
    let (from, to) = NODE_KIND_RENAMES.iter().find(|(o, _)| *o == old)?;
    observation.node_kind = Some((*to).to_string());
    Some((*from, *to))
}

/// Atomically re-emit the observation TOML: write to a sibling temp file
/// in the same directory and `rename` over the original. Same shape as
/// the manifest installer in `install.rs` — keeps readers from ever
/// seeing a half-written file.
fn atomic_write_toml(path: &Path, observation: &Observation) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "observation path has no parent",
        )
    })?;
    let tmp = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("observation")
    ));
    let text = toml::to_string_pretty(observation).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("toml encode: {error}"),
        )
    })?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
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

    /// Pre-rename observations (e.g. anchored on the now-retired
    /// `literature_search` kind) get rewritten to the current DAG kind
    /// on first read; the on-disk TOML is also re-emitted so the
    /// rename is sticky across subsequent loads. The id is unaffected.
    #[test]
    fn renames_literature_search_to_source_literature_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(tmp.path());
        let recorded = store
            .record(ObservationInput {
                kind: ObservationKind::Recipe,
                source: ObservationSource::Agent,
                summary: "lit audit".into(),
                body: "audit body".into(),
                node_kind: Some("literature_search".into()),
                error: None,
            })
            .unwrap();
        // Manually rewrite the persisted TOML back to the stale name to
        // simulate an observation recorded before the migration.
        let path = tmp
            .path()
            .join(OBSERVATIONS_DIR)
            .join(format!("{}.toml", recorded.id));
        std::fs::write(
            &path,
            format!(
                "id = '{}'\ncreated_at = 0\nkind = 'recipe'\nsource = 'agent'\nsummary = 'lit audit'\nbody = 'audit body'\nnode_kind = 'literature_search'\n",
                recorded.id
            ),
        )
        .unwrap();

        // First list() returns the rewritten kind and persists the change.
        let first = store.list();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, recorded.id);
        assert_eq!(first[0].node_kind.as_deref(), Some("source_literature"));
        // The TOML on disk now carries the new kind.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains("node_kind = \"source_literature\""),
            "TOML was not rewritten: {on_disk}"
        );
        assert!(!on_disk.contains("literature_search\""));

        // Subsequent list() observes the same rewritten value with no
        // further migration; no temp files are left in the dir.
        let second = store.list();
        assert_eq!(second[0].node_kind.as_deref(), Some("source_literature"));
        let entries: Vec<_> = std::fs::read_dir(tmp.path().join(OBSERVATIONS_DIR))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            entries.iter().all(|n| !n.ends_with(".tmp")),
            "leftover tmp: {entries:?}"
        );
    }
}
