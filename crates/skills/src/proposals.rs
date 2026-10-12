//! Skill proposals: the staging area between distillation and the
//! live library.
//!
//! A proposal is a directory `<state_dir>/skill-proposals/<name>/`
//! holding a draft `SKILL.md` plus a `proposal.toml` manifest:
//!
//! ```toml
//! status = "pending"            # pending | approved | rejected
//! cluster_hash = "c4f0…"        # idempotency key from distillation
//! rationale = "why this skill"
//! created_at = 1730000000
//! updated_at = 1730000000
//! source_observation_ids = ["O-…", "O-…"]
//! ```
//!
//! The lifecycle mirrors EvoScientist's proposals module, tightened
//! where determinism allows:
//!
//! - **staging is physical**: the proposal area and the live global
//!   tier are separate directories; promotion is an explicit copy
//!   through [`Proposals::approve`]
//! - **approval validates**: the strict contract (kebab-case name
//!   matching the directory, description rules, no TODO placeholders)
//!   is enforced at both submit and approve time — a hand-edited
//!   proposal cannot sneak through
//! - **clusters are consumed exactly once**: approve and reject both
//!   write a processed marker keyed by `cluster_hash`, so a rejected
//!   pattern is never re-proposed (negative feedback counts too)
//! - **the generation counter advances** on every approve, so cached
//!   views (prompt index) refresh without anyone remembering

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::SkillError;
use crate::format::{self, SkillMeta};
use crate::install;

/// Directory under the state dir holding proposals.
pub const PROPOSALS_DIR: &str = "skill-proposals";
/// Processed-cluster markers, so consumed clusters never re-propose.
pub const PROCESSED_DIR: &str = "skill-proposals-processed";
/// The manifest filename inside a proposal directory.
pub const MANIFEST_FILE: &str = "proposal.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    /// Written by distillation, not yet installed — transient: the
    /// same cycle (or the next, past the install cap) applies it.
    Pending,
    /// Installed into the live library. Reads legacy `"approved"`
    /// manifests written before the review gate was removed.
    #[serde(alias = "approved")]
    Applied,
    /// Legacy only: a human rejection from the review era. Kept so
    /// old state dirs keep consuming their clusters; nothing new
    /// lands here.
    Rejected,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalStatus::Pending => "pending",
            ProposalStatus::Applied => "applied",
            ProposalStatus::Rejected => "rejected",
        }
    }
}

/// A proposal's parsed manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub name: String,
    pub status: ProposalStatus,
    pub cluster_hash: String,
    #[serde(default)]
    pub rationale: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub source_observation_ids: Vec<String>,
    /// True when this proposal updates an already-installed skill
    /// instead of creating one. The ids then record the FULL cluster
    /// membership at update time, so the delta against the previous
    /// manifest is exactly the new evidence.
    #[serde(default)]
    pub update: bool,
    /// Who drafted the proposal. Only the distiller writes new ones;
    /// legacy manifests may carry `agent` (the removed `skill_propose`
    /// channel) — those are consumed, never installed.
    #[serde(default = "default_authored_by")]
    pub authored_by: String,
    /// Set on application: where the skill landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_path: Option<String>,
}

/// Stable hash of a cluster's observation ids — the idempotency key
/// proposals are keyed and consumed by. Owned by evolution-core.
pub use evolution_core::cluster::cluster_hash;

fn default_authored_by() -> String {
    "distiller".to_string()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Snapshot the skill's current SKILL.md into `<skill>/.history/`
/// before an update overlays it — the rollback material.
fn archive_current(skill_dir: &Path) -> Result<(), SkillError> {
    let history = skill_dir.join(".history");
    std::fs::create_dir_all(&history)?;
    let archive = history.join(format!("{}.md", unix_now()));
    std::fs::copy(skill_dir.join("SKILL.md"), archive)?;
    Ok(())
}

/// The proposal area rooted at `<state_dir>/skill-proposals/`.
#[derive(Debug, Clone)]
pub struct Proposals {
    root: PathBuf,
    processed_root: PathBuf,
}

/// Outcome of applying one proposal.
#[derive(Debug)]
pub struct ApplyOutcome {
    pub name: String,
    /// Where the skill was installed (the global tier).
    pub destination: PathBuf,
}

impl Proposals {
    pub fn open(state_dir: &Path) -> Self {
        Self {
            root: state_dir.join(PROPOSALS_DIR),
            processed_root: state_dir.join(PROCESSED_DIR),
        }
    }

    /// The proposal-area directory (for distillation writers).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Register a proposal directory written by the distiller:
    /// validate strictly, then write the manifest.
    ///
    /// Lifecycle:
    /// - no existing proposal → register as pending
    /// - existing **applied** proposal → only an `update` proposal may
    ///   re-register (the continuing-evolution channel); its status
    ///   resets to pending until the cycle applies it
    /// - existing **pending** or **rejected** → refused (no duplicates,
    ///   rejected patterns never revive)
    pub fn submit(
        &self,
        name: &str,
        cluster_hash_value: &str,
        rationale: &str,
        source_observation_ids: Vec<String>,
        update: bool,
    ) -> Result<Proposal, SkillError> {
        let errors = self.validate_dir(name);
        if !errors.is_empty() {
            return Err(SkillError::invalid_frontmatter(
                self.root.join(name),
                errors.join("; "),
            ));
        }
        if let Some(existing) = self.load(name)? {
            let allowed = existing.status == ProposalStatus::Applied && update;
            if !allowed {
                return Err(SkillError::invalid_frontmatter(
                    self.root.join(name),
                    format!(
                        "a proposal named {name:?} already exists with status {} \
                         (only update proposals may follow an applied one)",
                        existing.status.as_str()
                    ),
                ));
            }
        }
        let now = unix_now();
        let proposal = Proposal {
            name: name.to_string(),
            status: ProposalStatus::Pending,
            cluster_hash: cluster_hash_value.to_string(),
            rationale: rationale.to_string(),
            created_at: now,
            updated_at: now,
            source_observation_ids,
            update,
            authored_by: default_authored_by(),
            approved_path: None,
        };
        self.save(&proposal)?;
        Ok(proposal)
    }

    /// The stored proposal by name, when its manifest exists and
    /// parses.
    pub fn find(&self, name: &str) -> Option<Proposal> {
        self.load(name).ok().flatten()
    }

    /// The full observation-id set the latest manifest for `name`
    /// covers — the baseline an update proposal's delta is measured
    /// against.
    pub fn covered_observation_ids(&self, name: &str) -> BTreeSet<String> {
        self.find(name)
            .map(|p| p.source_observation_ids.into_iter().collect())
            .unwrap_or_default()
    }

    /// Strict validation of a proposal directory: kebab-case name
    /// matching the directory, parseable SKILL.md, description
    /// sanity, no TODO placeholders in the body.
    pub fn validate_dir(&self, name: &str) -> Vec<String> {
        let mut errors = Vec::new();
        if format::sanitize_name(name).as_deref() != Some(name) {
            errors.push(format!(
                "name {name:?} must be lowercase kebab-case and match the proposal directory"
            ));
            return errors;
        }
        let dir = self.root.join(name);
        let skill_md = dir.join("SKILL.md");
        let Ok(content) = std::fs::read_to_string(&skill_md) else {
            errors.push(format!("missing {}/SKILL.md", name));
            return errors;
        };
        let (meta, body) = match format::parse_skill_md(&content) {
            Ok(parsed) => parsed,
            Err(e) => {
                errors.push(e.to_string());
                return errors;
            }
        };
        if meta.name != name {
            errors.push(format!(
                "SKILL.md frontmatter name must be {name:?}, got {:?}",
                meta.name
            ));
        }
        if let Err(e) = format::validate_meta(&meta) {
            errors.push(e.to_string());
        }
        if body.contains("TODO") {
            errors.push("SKILL.md body must not contain TODO placeholders".into());
        }
        errors
    }

    /// All proposals, sorted by name. Malformed manifests are skipped
    /// with a warning — a corrupt sidecar never hides the rest.
    pub fn list(&self) -> Vec<Proposal> {
        let Ok(read_dir) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            match self.load(name) {
                Ok(Some(proposal)) => out.push(proposal),
                Ok(None) => {
                    tracing::warn!(proposal = name, "proposal directory has no manifest");
                }
                Err(e) => {
                    tracing::warn!(proposal = name, error = %e, "malformed proposal manifest");
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Cluster hashes already consumed by a terminal proposal or an
    /// explicit skip marker.
    pub fn processed_clusters(&self) -> BTreeSet<String> {
        let mut out: BTreeSet<String> = self
            .list()
            .into_iter()
            .filter(|p| p.status != ProposalStatus::Pending)
            .map(|p| p.cluster_hash)
            .collect();
        if let Ok(read_dir) = std::fs::read_dir(&self.processed_root) {
            for entry in read_dir.flatten() {
                let name = entry.file_name();
                if let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".toml")) {
                    out.insert(stem.to_string());
                }
            }
        }
        out
    }

    /// Mark a cluster consumed without a proposal (e.g. name
    /// collision with an installed skill).
    pub fn mark_processed(&self, hash: &str) -> Result<(), SkillError> {
        std::fs::create_dir_all(&self.processed_root)?;
        let marker = self.processed_root.join(format!("{hash}.toml"));
        let text = format!("cluster_hash = {hash:?}\nprocessed_at = {}\n", unix_now());
        std::fs::write(&marker, text)?;
        Ok(())
    }

    /// Apply a pending proposal into `dest_root`, flip the status,
    /// consume the cluster. Bumps nothing itself — the caller (the
    /// manager) owns the generation.
    ///
    /// Two installation shapes, by manifest:
    ///
    /// - **create** — refuses when the destination already exists, then
    ///   replaces-installs the proposal directory (minus the manifest).
    /// - **update** — requires the destination to exist, archives the
    ///   current `SKILL.md` to `<skill>/.history/<ts>.md`, then
    ///   overlays: same-relative-path files replace, omitted files are
    ///   preserved. Every update is reversible by restoring the
    ///   archive.
    pub fn apply(&self, name: &str, dest_root: &Path) -> Result<ApplyOutcome, SkillError> {
        let Some(mut proposal) = self
            .load(name)?
            .filter(|p| p.status == ProposalStatus::Pending)
        else {
            return Err(SkillError::invalid_frontmatter(
                self.root.join(name),
                format!("no pending proposal named {name:?}"),
            ));
        };
        let errors = self.validate_dir(name);
        if !errors.is_empty() {
            return Err(SkillError::invalid_frontmatter(
                self.root.join(name),
                errors.join("; "),
            ));
        }
        let source = self.root.join(name);
        let destination = dest_root.join(name);
        if proposal.update {
            if !destination.join("SKILL.md").is_file() {
                return Err(SkillError::invalid_frontmatter(
                    source,
                    format!(
                        "update proposal for {name:?} but no installed skill at {}",
                        destination.display()
                    ),
                ));
            }
            archive_current(&destination)?;
        } else if destination.exists() {
            return Err(SkillError::invalid_frontmatter(
                source,
                format!(
                    "a skill named {name:?} is already installed at {}",
                    destination.display()
                ),
            ));
        }
        install::install_overlay(&source, &destination, &[MANIFEST_FILE])?;
        proposal.status = ProposalStatus::Applied;
        proposal.updated_at = unix_now();
        proposal.approved_path = Some(destination.display().to_string());
        self.save(&proposal)?;
        self.mark_processed(&proposal.cluster_hash)?;
        Ok(ApplyOutcome {
            name: name.to_string(),
            destination,
        })
    }

    fn load(&self, name: &str) -> Result<Option<Proposal>, SkillError> {
        let path = self.root.join(name).join(MANIFEST_FILE);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)?;
        toml::from_str(&text)
            .map(Some)
            .map_err(|_| SkillError::BadManifest(path))
    }

    fn save(&self, proposal: &Proposal) -> Result<(), SkillError> {
        let dir = self.root.join(&proposal.name);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(MANIFEST_FILE);
        let text =
            toml::to_string_pretty(proposal).map_err(|_| SkillError::BadManifest(path.clone()))?;
        std::fs::write(&path, text)?;
        Ok(())
    }

    /// Parsed metadata of a proposal's SKILL.md, for listings.
    pub fn meta(&self, name: &str) -> Option<SkillMeta> {
        let content = std::fs::read_to_string(self.root.join(name).join("SKILL.md")).ok()?;
        format::parse_skill_md(&content).ok().map(|(meta, _)| meta)
    }

    /// The proposal's SKILL.md split into `(meta, body)` — the
    /// document a proposed-but-not-installed skill would promote.
    pub fn read_skill_md(&self, name: &str) -> Option<(SkillMeta, String)> {
        let content = std::fs::read_to_string(self.root.join(name).join("SKILL.md")).ok()?;
        format::parse_skill_md(&content).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(tmp: &std::path::Path) -> Proposals {
        Proposals::open(tmp)
    }

    fn write_proposal(area: &Proposals, name: &str, body_extra: &str) {
        let dir = area.root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: Auto fix for {name}.\ntags: [auto]\n---\n# Fix\n\nDo the thing.{body_extra}\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn submit_validates_and_lists() {
        let tmp = tempfile::tempdir().unwrap();
        let area = area(tmp.path());
        write_proposal(&area, "file-to-dataframe-schema", "");
        let proposal = area
            .submit(
                "file-to-dataframe-schema",
                "c-abc",
                "three observations",
                vec!["O-1".into(), "O-2".into()],
                false,
            )
            .unwrap();
        assert_eq!(proposal.status, ProposalStatus::Pending);
        assert_eq!(area.list().len(), 1);

        // Duplicate submit over a terminal/pending proposal refused.
        assert!(
            area.submit("file-to-dataframe-schema", "c-abc", "again", vec![], false)
                .is_err()
        );
    }

    #[test]
    fn submit_rejects_contract_violations() {
        let tmp = tempfile::tempdir().unwrap();
        let area = area(tmp.path());
        // Name/dir mismatch.
        write_proposal(&area, "mismatch", "");
        let dir = area.root.join("mismatch");
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: other-name\ndescription: d\n---\nb\n",
        )
        .unwrap();
        let err = area
            .submit("mismatch", "c-1", "r", vec![], false)
            .unwrap_err();
        assert!(err.to_string().contains("frontmatter name"));

        // TODO in body.
        write_proposal(&area, "has-todo", " <!-- TODO --> ");
        let err = area
            .submit("has-todo", "c-2", "r", vec![], false)
            .unwrap_err();
        assert!(err.to_string().contains("TODO"));
    }

    #[test]
    fn apply_copies_validates_and_consumes_cluster() {
        let tmp = tempfile::tempdir().unwrap();
        let area = area(tmp.path());
        let global = tmp.path().join("skills");
        write_proposal(&area, "good-fix", "");
        area.submit("good-fix", "c-good", "r", vec!["O-a".into()], false)
            .unwrap();

        let outcome = area.apply("good-fix", &global).unwrap();
        assert!(outcome.destination.ends_with("good-fix"));
        assert!(global.join("good-fix").join("SKILL.md").is_file());

        // Status terminal; re-apply refused; cluster consumed.
        assert_eq!(area.list()[0].status, ProposalStatus::Applied);
        assert!(area.apply("good-fix", &global).is_err());
        assert!(area.processed_clusters().contains("c-good"));

        // A new pending proposal with the same cluster is refused via
        // submit's existing-name check, and distill skips it via
        // processed_clusters anyway.
    }

    /// A legacy `"approved"` manifest (written before the review gate
    /// was removed) reads back as Applied — old state dirs keep
    /// consuming their clusters.
    #[test]
    fn legacy_approved_manifest_reads_as_applied() {
        let tmp = tempfile::tempdir().unwrap();
        let area = area(tmp.path());
        write_proposal(&area, "old-one", "");
        let dir = area.root.join("old-one");
        std::fs::write(
            dir.join("proposal.toml"),
            "name = 'old-one'\nstatus = 'approved'\ncluster_hash = 'c-old'\ncreated_at = 1\nupdated_at = 1\n",
        )
        .unwrap();
        assert_eq!(area.list()[0].status, ProposalStatus::Applied);
        assert!(area.processed_clusters().contains("c-old"));
    }

    #[test]
    fn approve_refuses_when_skill_already_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let area = area(tmp.path());
        let global = tmp.path().join("skills");
        std::fs::create_dir_all(global.join("clash")).unwrap();
        std::fs::write(
            global.join("clash").join("SKILL.md"),
            "---\nname: clash\ndescription: installed already.\n---\nb\n",
        )
        .unwrap();
        write_proposal(&area, "clash", "");
        area.submit("clash", "c-clash", "r", vec![], false).unwrap();
        assert!(area.apply("clash", &global).is_err());
    }

    #[test]
    fn hand_edited_proposal_cannot_sneak_through_approve() {
        let tmp = tempfile::tempdir().unwrap();
        let area = area(tmp.path());
        let global = tmp.path().join("skills");
        write_proposal(&area, "editable", "");
        area.submit("editable", "c-edit", "r", vec![], false)
            .unwrap();
        // Tamper after submit: break the body contract.
        std::fs::write(
            area.root.join("editable").join("SKILL.md"),
            "---\nname: editable\ndescription: d\n---\nTODO: fill this in\n",
        )
        .unwrap();
        assert!(area.apply("editable", &global).is_err());
    }
}
