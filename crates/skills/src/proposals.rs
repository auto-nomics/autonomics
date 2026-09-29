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
use sha2::{Digest, Sha256};

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
    Pending,
    Approved,
    Rejected,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalStatus::Pending => "pending",
            ProposalStatus::Approved => "approved",
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
    /// Set on approval: where the skill landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_path: Option<String>,
}

/// Stable hash of a cluster's observation ids — the idempotency key.
pub fn cluster_hash(observation_ids: &BTreeSet<String>) -> String {
    let mut hasher = Sha256::new();
    for id in observation_ids {
        hasher.update(id.as_bytes());
        hasher.update(b"\0");
    }
    format!("c-{}", hex(&hasher.finalize())[..16].to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The proposal area rooted at `<state_dir>/skill-proposals/`.
#[derive(Debug, Clone)]
pub struct Proposals {
    root: PathBuf,
    processed_root: PathBuf,
}

/// Outcome of approving one proposal.
pub struct ApproveOutcome {
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

    /// Register a proposal directory written by a distiller: validate
    /// strictly, then write the manifest. An existing rejected or
    /// approved proposal under the same name is refused — statuses
    /// are terminal (EvoScientist contract, kept).
    pub fn submit(
        &self,
        name: &str,
        cluster_hash_value: &str,
        rationale: &str,
        source_observation_ids: Vec<String>,
    ) -> Result<Proposal, SkillError> {
        let errors = self.validate_dir(name);
        if !errors.is_empty() {
            return Err(SkillError::invalid_frontmatter(
                self.root.join(name),
                errors.join("; "),
            ));
        }
        if let Some(existing) = self.load(name)? {
            return Err(SkillError::invalid_frontmatter(
                self.root.join(name),
                format!(
                    "a proposal named {name:?} already exists with status {}",
                    existing.status.as_str()
                ),
            ));
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
            approved_path: None,
        };
        self.save(&proposal)?;
        Ok(proposal)
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

    /// Approve a pending proposal: re-validate, copy into the global
    /// tier, flip the status, consume the cluster. Bumps nothing
    /// itself — the caller (the manager) owns the generation.
    pub fn approve(
        &self,
        name: &str,
        global_skills_root: &Path,
    ) -> Result<ApproveOutcome, SkillError> {
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
        let destination = global_skills_root.join(name);
        if destination.exists() {
            return Err(SkillError::invalid_frontmatter(
                source,
                format!(
                    "a skill named {name:?} is already installed at {}",
                    destination.display()
                ),
            ));
        }
        install::install_from_local(&source, global_skills_root, None)?;
        proposal.status = ProposalStatus::Approved;
        proposal.updated_at = unix_now();
        proposal.approved_path = Some(destination.display().to_string());
        self.save(&proposal)?;
        self.mark_processed(&proposal.cluster_hash)?;
        Ok(ApproveOutcome {
            name: name.to_string(),
            destination,
        })
    }

    /// Reject a pending proposal and consume its cluster.
    pub fn reject(&self, name: &str) -> Result<Proposal, SkillError> {
        let Some(mut proposal) = self
            .load(name)?
            .filter(|p| p.status == ProposalStatus::Pending)
        else {
            return Err(SkillError::invalid_frontmatter(
                self.root.join(name),
                format!("no pending proposal named {name:?}"),
            ));
        };
        proposal.status = ProposalStatus::Rejected;
        proposal.updated_at = unix_now();
        self.save(&proposal)?;
        self.mark_processed(&proposal.cluster_hash)?;
        Ok(proposal)
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
            )
            .unwrap();
        assert_eq!(proposal.status, ProposalStatus::Pending);
        assert_eq!(area.list().len(), 1);

        // Duplicate submit over a terminal/pending proposal refused.
        assert!(
            area.submit("file-to-dataframe-schema", "c-abc", "again", vec![])
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
        let err = area.submit("mismatch", "c-1", "r", vec![]).unwrap_err();
        assert!(err.to_string().contains("frontmatter name"));

        // TODO in body.
        write_proposal(&area, "has-todo", " <!-- TODO --> ");
        let err = area.submit("has-todo", "c-2", "r", vec![]).unwrap_err();
        assert!(err.to_string().contains("TODO"));
    }

    #[test]
    fn approve_copies_validates_and_consumes_cluster() {
        let tmp = tempfile::tempdir().unwrap();
        let area = area(tmp.path());
        let global = tmp.path().join("skills");
        write_proposal(&area, "good-fix", "");
        area.submit("good-fix", "c-good", "r", vec!["O-a".into()])
            .unwrap();

        let outcome = area.approve("good-fix", &global).unwrap();
        assert!(outcome.destination.ends_with("good-fix"));
        assert!(global.join("good-fix").join("SKILL.md").is_file());

        // Status terminal; re-approve refused; cluster consumed.
        assert_eq!(area.list()[0].status, ProposalStatus::Approved);
        assert!(area.approve("good-fix", &global).is_err());
        assert!(area.processed_clusters().contains("c-good"));

        // A new pending proposal with the same cluster is refused via
        // submit's existing-name check, and distill skips it via
        // processed_clusters anyway.
    }

    #[test]
    fn reject_consumes_cluster() {
        let tmp = tempfile::tempdir().unwrap();
        let area = area(tmp.path());
        write_proposal(&area, "bad-fix", "");
        area.submit("bad-fix", "c-bad", "r", vec![]).unwrap();
        let rejected = area.reject("bad-fix").unwrap();
        assert_eq!(rejected.status, ProposalStatus::Rejected);
        assert!(area.processed_clusters().contains("c-bad"));
        assert!(area.reject("bad-fix").is_err());
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
        area.submit("clash", "c-clash", "r", vec![]).unwrap();
        assert!(area.approve("clash", &global).is_err());
    }

    #[test]
    fn hand_edited_proposal_cannot_sneak_through_approve() {
        let tmp = tempfile::tempdir().unwrap();
        let area = area(tmp.path());
        let global = tmp.path().join("skills");
        write_proposal(&area, "editable", "");
        area.submit("editable", "c-edit", "r", vec![]).unwrap();
        // Tamper after submit: break the body contract.
        std::fs::write(
            area.root.join("editable").join("SKILL.md"),
            "---\nname: editable\ndescription: d\n---\nTODO: fill this in\n",
        )
        .unwrap();
        assert!(area.approve("editable", &global).is_err());
    }
}
