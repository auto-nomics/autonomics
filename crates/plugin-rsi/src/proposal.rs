use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    ApprovedImage, Error, GitRepo, Result,
    request::{RequestStore, atomic_toml, unix_now},
    validate::ImageCatalog,
    workspace::ProposalWorkspace,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalAction {
    NewPlugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Draft,
    Validating,
    NeedsFix,
    PendingReview,
    Approved,
    Publishing,
    Published,
    InstallPending,
    Installed,
    PublishFailed,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub schema_version: u32,
    pub proposal_id: String,
    pub plugin_name: String,
    pub node_kind: String,
    pub action: ProposalAction,
    pub status: ProposalStatus,
    pub authored_by: String,
    pub request_ids: Vec<String>,
    pub image_id: String,
    pub image_reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_report: Option<String>,
    pub rationale: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct ProposalStore {
    root: PathBuf,
    default_branch: String,
    author_name: String,
    author_email: String,
}

impl ProposalStore {
    pub fn open(
        state_dir: &Path,
        default_branch: &str,
        author_name: &str,
        author_email: &str,
    ) -> Self {
        Self {
            root: state_dir.join("plugin-rsi").join("proposals"),
            default_branch: default_branch.to_string(),
            author_name: author_name.to_string(),
            author_email: author_email.to_string(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        plugin_name: &str,
        request_ids: &[String],
        image_id: &str,
        catalog: &ImageCatalog,
        rationale: &str,
        requests: &RequestStore,
    ) -> Result<Proposal> {
        crate::validate_plugin_name(plugin_name)?;
        let node_kind = crate::derive_node_kind(plugin_name)?;
        let requested = BTreeSet::from_iter(request_ids.iter().cloned());
        if requested.is_empty() {
            return Err(Error::InvalidRequest(
                "at least one request id is required".into(),
            ));
        }
        let missing = requests.missing_ids(&requested);
        if !missing.is_empty() {
            return Err(Error::InvalidRequest(format!(
                "unknown request ids: {}",
                missing.join(", ")
            )));
        }
        let image = catalog.get(image_id).ok_or_else(|| {
            Error::Validation(format!("image {image_id:?} is not approved for RSI"))
        })?;
        if rationale.trim().is_empty() {
            return Err(Error::InvalidRequest("rationale is required".into()));
        }

        let now = unix_now();
        let proposal_id = proposal_id(plugin_name, &requested, now);
        let path = self.proposal_path(&proposal_id);
        if path.exists() {
            return Err(Error::InvalidRequest(format!(
                "proposal directory already exists: {}",
                path.display()
            )));
        }
        GitRepo::init(path.join("repo"), &self.default_branch)?;
        let proposal = Proposal {
            schema_version: 1,
            proposal_id,
            plugin_name: plugin_name.to_string(),
            node_kind,
            action: ProposalAction::NewPlugin,
            status: ProposalStatus::Draft,
            authored_by: "agent".to_string(),
            request_ids: request_ids.to_vec(),
            image_id: image_id.to_string(),
            image_reference: image.reference.to_string(),
            source_commit: None,
            remote: None,
            pushed_commit: None,
            latest_report: None,
            rationale: rationale.trim().to_string(),
            created_at: now,
            updated_at: now,
        };
        self.save(&proposal)?;
        Ok(proposal)
    }

    pub fn find(&self, id: &str) -> Result<Option<Proposal>> {
        let path = self.manifest_path(id);
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

    pub fn list(&self) -> Result<Vec<Proposal>> {
        let mut proposals = Vec::new();
        let plugin_dirs = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(proposals),
            Err(source) => return Err(source.into()),
        };
        for plugin_dir in plugin_dirs.flatten() {
            let Ok(proposal_dirs) = std::fs::read_dir(plugin_dir.path()) else {
                continue;
            };
            for proposal_dir in proposal_dirs.flatten() {
                let id = proposal_dir.file_name().to_string_lossy().to_string();
                if let Some(proposal) = self.find(&id)? {
                    proposals.push(proposal);
                }
            }
        }
        proposals.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(proposals)
    }

    pub fn workspace(&self, id: &str) -> Result<Option<ProposalWorkspace>> {
        let proposal = self.find(id)?;
        Ok(proposal.map(|_| ProposalWorkspace::new(self.proposal_path(id).join("repo"))))
    }

    pub fn reports_dir(&self, id: &str) -> PathBuf {
        self.proposal_path(id).join("reports")
    }

    pub fn snapshot(&self, id: &str, message: &str) -> Result<Option<String>> {
        let proposal = self.load(id)?;
        if !matches!(
            proposal.status,
            ProposalStatus::Draft | ProposalStatus::Validating | ProposalStatus::NeedsFix
        ) {
            return Err(Error::InvalidTransition {
                from: format!("{:?}", proposal.status),
                to: "snapshot".into(),
            });
        }
        let repo = GitRepo::open(self.proposal_path(id).join("repo"));
        let commit = repo.snapshot_commit(message, &self.author_name, &self.author_email)?;
        if let Some(commit) = &commit {
            self.update(id, |proposal| {
                proposal.source_commit = Some(commit.clone());
            })?;
        }
        Ok(commit)
    }

    pub fn record_report(
        &self,
        id: &str,
        relative_report: &str,
        report: &crate::ValidationReport,
    ) -> Result<Proposal> {
        self.update(id, |proposal| {
            proposal.latest_report = Some(relative_report.to_string());
            if proposal.status == ProposalStatus::Validating && !report.passed() {
                proposal.status = ProposalStatus::NeedsFix;
            }
        })
    }

    pub fn start_validation(&self, id: &str) -> Result<Proposal> {
        self.transition(id, ProposalStatus::Validating)
    }

    pub fn submit(&self, id: &str) -> Result<Proposal> {
        let proposal = self.load(id)?;
        self.ensure_transition(proposal.status, ProposalStatus::PendingReview)?;
        let repo = GitRepo::open(self.proposal_path(id).join("repo"));
        if !repo.is_clean()? {
            return Err(Error::Validation(
                "working tree is dirty; snapshot the reviewed content first".into(),
            ));
        }
        let head = repo.head()?;
        if proposal.source_commit.as_deref() != Some(head.as_str()) {
            return Err(Error::Validation(
                "proposal source_commit does not match repository HEAD".into(),
            ));
        }
        let report = self.latest_report(&proposal)?;
        if !report.passed() {
            return Err(Error::Validation(
                "latest validation report did not pass".into(),
            ));
        }
        self.transition(id, ProposalStatus::PendingReview)
    }

    pub fn approve(&self, id: &str) -> Result<Proposal> {
        self.transition(id, ProposalStatus::Approved)
    }

    pub fn reject(&self, id: &str) -> Result<Proposal> {
        self.transition(id, ProposalStatus::Rejected)
    }

    pub fn start_publish(&self, id: &str) -> Result<Proposal> {
        self.transition(id, ProposalStatus::Publishing)
    }

    pub fn mark_published(&self, id: &str, remote: &str, commit: &str) -> Result<Proposal> {
        self.update(id, |proposal| {
            if proposal.status != ProposalStatus::Publishing {
                return;
            }
            proposal.status = ProposalStatus::Published;
            proposal.remote = Some(remote.to_string());
            proposal.pushed_commit = Some(commit.to_string());
        })
    }

    pub fn mark_publish_failed(&self, id: &str) -> Result<Proposal> {
        self.transition(id, ProposalStatus::PublishFailed)
    }

    pub fn mark_install_pending(&self, id: &str) -> Result<Proposal> {
        self.transition(id, ProposalStatus::InstallPending)
    }

    pub fn mark_installed(&self, id: &str) -> Result<Proposal> {
        self.transition(id, ProposalStatus::Installed)
    }

    pub fn latest_report(&self, proposal: &Proposal) -> Result<crate::ValidationReport> {
        let relative = proposal
            .latest_report
            .as_deref()
            .ok_or_else(|| Error::Validation("proposal has no validation report".into()))?;
        let path = self.proposal_path(&proposal.proposal_id).join(relative);
        if !path.is_file() {
            return Err(Error::Validation(
                "latest validation report is missing".into(),
            ));
        }
        let text =
            std::fs::read_to_string(&path).map_err(|source| Error::ReadFile { path, source })?;
        serde_json::from_str(&text)
            .map_err(|source| Error::Validation(format!("invalid report JSON: {source}")))
    }

    pub(crate) fn transition(&self, id: &str, to: ProposalStatus) -> Result<Proposal> {
        let proposal = self.load(id)?;
        self.ensure_transition(proposal.status, to)?;
        self.update(id, |proposal| proposal.status = to)
    }

    fn update<F>(&self, id: &str, mutate: F) -> Result<Proposal>
    where
        F: FnOnce(&mut Proposal),
    {
        let mut proposal = self.load(id)?;
        let before = proposal.status;
        mutate(&mut proposal);
        proposal.updated_at = unix_now();
        self.ensure_transition(before, proposal.status)?;
        self.save(&proposal)?;
        Ok(proposal)
    }

    fn ensure_transition(&self, from: ProposalStatus, to: ProposalStatus) -> Result<()> {
        let valid = match (from, to) {
            (ProposalStatus::Draft, ProposalStatus::Validating)
            | (ProposalStatus::NeedsFix, ProposalStatus::Validating)
            | (ProposalStatus::Validating, ProposalStatus::NeedsFix)
            | (ProposalStatus::Validating, ProposalStatus::PendingReview)
            | (ProposalStatus::PendingReview, ProposalStatus::Approved)
            | (ProposalStatus::PendingReview, ProposalStatus::Rejected)
            | (ProposalStatus::Approved, ProposalStatus::Publishing)
            | (ProposalStatus::Publishing, ProposalStatus::Published)
            | (ProposalStatus::Publishing, ProposalStatus::PublishFailed)
            | (ProposalStatus::PublishFailed, ProposalStatus::Approved)
            | (ProposalStatus::Published, ProposalStatus::InstallPending)
            | (ProposalStatus::InstallPending, ProposalStatus::Installed) => true,
            (from, to) if from == to => true,
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(Error::InvalidTransition {
                from: format!("{from:?}").to_lowercase(),
                to: format!("{to:?}").to_lowercase(),
            })
        }
    }

    fn load(&self, id: &str) -> Result<Proposal> {
        self.find(id)?
            .ok_or_else(|| Error::InvalidRequest(format!("unknown proposal {id:?}")))
    }

    fn save(&self, proposal: &Proposal) -> Result<()> {
        atomic_toml(&self.manifest_path(&proposal.proposal_id), proposal)
    }

    fn manifest_path(&self, id: &str) -> PathBuf {
        self.proposal_path(id).join("proposal.toml")
    }

    fn proposal_path(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }
}

fn proposal_id(plugin_name: &str, request_ids: &BTreeSet<String>, now: i64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(plugin_name.as_bytes());
    hasher.update(b"\0");
    for id in request_ids {
        hasher.update(id.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(now.to_le_bytes());
    format!("P-{}", &crate::request::hex(&hasher.finalize())[..16])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{RequestIntent, RequestRecord, RequestSource, RequestStatus};

    fn fixture(tmp: &Path) -> (RequestStore, ProposalStore, ImageCatalog) {
        let requests = RequestStore::open(tmp);
        requests
            .record(RequestRecord {
                id: String::new(),
                created_at: 0,
                source: RequestSource::User,
                intent: RequestIntent::NewNode,
                summary: "Create demo plugin".into(),
                body: "Deterministic adapter".into(),
                plugin_name: Some("demo-plugin".into()),
                node_kind: None,
                image_id: Some("demo".into()),
                evidence_ids: Vec::new(),
                status: RequestStatus::Open,
            })
            .unwrap();
        let proposals = ProposalStore::open(tmp, "main", "RSI Test", "rsi@example.com");
        let mut catalog = ImageCatalog::default();
        catalog.insert(
            "demo",
            ApprovedImage {
                reference: "docker.io/library/hello-world@sha256:5e23090353324d887c48ad5e5c56d294eab81588df9605b07d1afe895f9ccf8".into(),
                interpreters: vec!["sh".into()],
            },
        );
        (requests, proposals, catalog)
    }

    fn first_request(requests: &RequestStore) -> String {
        requests.list()[0].id.clone()
    }

    #[test]
    fn creates_git_backed_workspace_and_enforces_review_gates() {
        let tmp = tempfile::tempdir().unwrap();
        let (requests, proposals, catalog) = fixture(tmp.path());
        let request_id = first_request(&requests);
        let proposal = proposals
            .create(
                "demo-plugin",
                &[request_id],
                "demo",
                &catalog,
                "Needed for testing",
                &requests,
            )
            .unwrap();
        assert!(
            proposals
                .workspace(&proposal.proposal_id)
                .unwrap()
                .is_some()
        );

        let workspace = proposals.workspace(&proposal.proposal_id).unwrap().unwrap();
        workspace.write_text("README.md", "# demo\n").unwrap();
        assert!(proposals.submit(&proposal.proposal_id).is_err());
    }
}
