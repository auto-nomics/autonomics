use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Error, GitRepo, Result,
    development::PluginDevelopment,
    install::InstalledPluginSource,
    request::{RequestStore, atomic_toml, unix_now},
    validate::EnvironmentCatalog,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalAction {
    NewPlugin,
    UpdatePlugin,
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
    PullRequestOpen,
    PullRequestMerged,
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
    /// Runtime kinds currently declared by the proposal's plugin manifest.
    #[serde(default)]
    pub node_kinds: Vec<String>,
    /// Runtime kinds owned by the installed plugin this proposal updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_node_kinds: Option<Vec<String>>,
    pub action: ProposalAction,
    pub status: ProposalStatus,
    pub authored_by: String,
    pub request_ids: Vec<String>,
    /// Approved reusable environment selected by this proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    /// Digest-pinned reference resolved from the environment catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_reference: Option<String>,
    /// Immutable installed revision from which an update proposal started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    /// Installed remote from which an update proposal started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_commit: Option<String>,
    /// GitHub PR created for an update proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_url: Option<String>,
    /// Commit installed after the update PR merged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_report: Option<String>,
    pub rationale: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Creates the baseline workspace for an update proposal.
pub trait PluginSourceFetcher {
    /// Materialize `source.remote` at `source.commit` in `destination`.
    fn fetch(&self, source: &InstalledPluginSource, destination: &Path) -> Result<()>;
}

/// Production fetcher backed by the system git CLI.
#[derive(Debug, Default, Clone, Copy)]
pub struct GitPluginSourceFetcher;

impl PluginSourceFetcher for GitPluginSourceFetcher {
    fn fetch(&self, source: &InstalledPluginSource, destination: &Path) -> Result<()> {
        GitRepo::clone_at(destination, &source.remote, &source.commit, "origin")?;
        Ok(())
    }
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

    pub fn create(
        &self,
        plugin_name: &str,
        request_ids: &[String],
        rationale: &str,
        requests: &RequestStore,
    ) -> Result<PluginDevelopment<'_>> {
        crate::validate_plugin_name(plugin_name)?;
        let requested = self.validate_requests(plugin_name, request_ids, requests, rationale)?;

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
            node_kinds: Vec::new(),
            base_node_kinds: None,
            action: ProposalAction::NewPlugin,
            status: ProposalStatus::Draft,
            authored_by: "agent".to_string(),
            request_ids: request_ids.to_vec(),
            environment_id: None,
            environment_reference: None,
            base_commit: None,
            base_remote: None,
            source_commit: None,
            remote: None,
            pushed_commit: None,
            pull_request_number: None,
            pull_request_url: None,
            merged_commit: None,
            latest_report: None,
            rationale: rationale.trim().to_string(),
            created_at: now,
            updated_at: now,
        };
        self.save(&proposal)?;
        Ok(PluginDevelopment::new(self, proposal))
    }

    /// Create an update proposal from one installed plugin's pinned source.
    ///
    /// The resulting workspace is detached at the installed commit. Existing
    /// node kinds are copied into the proposal so later validation can allow
    /// ownership changes while still rejecting collisions with other plugins.
    pub fn create_update(
        &self,
        plugin_name: &str,
        request_ids: &[String],
        rationale: &str,
        requests: &RequestStore,
        source: &InstalledPluginSource,
        catalog: &EnvironmentCatalog,
        fetcher: &dyn PluginSourceFetcher,
    ) -> Result<PluginDevelopment<'_>> {
        let requested = self.validate_requests(plugin_name, request_ids, requests, rationale)?;
        validate_update_source(source)?;

        let now = unix_now();
        let proposal_id = proposal_id(plugin_name, &requested, now);
        let path = self.proposal_path(&proposal_id);
        if path.exists() {
            return Err(Error::InvalidRequest(format!(
                "proposal directory already exists: {}",
                path.display()
            )));
        }
        let repo_path = path.join("repo");
        fetcher.fetch(source, &repo_path)?;
        let manifest_text =
            std::fs::read_to_string(repo_path.join("manifest.toml")).map_err(|source| {
                Error::ReadFile {
                    path: repo_path.join("manifest.toml"),
                    source,
                }
            })?;
        let manifest: container_plugin::manifest::PluginManifest =
            toml::from_str(&manifest_text).map_err(|source| Error::ParseToml {
                path: repo_path.join("manifest.toml"),
                source,
            })?;
        if manifest.plugin_name != plugin_name {
            return Err(Error::Validation(format!(
                "installed manifest names `{}`, expected `{plugin_name}`",
                manifest.plugin_name
            )));
        }
        let environment_reference = manifest.image.reference.as_str().to_string();
        let environment_id = catalog
            .find_reference(&environment_reference)
            .ok_or_else(|| {
                Error::Validation(format!(
                    "installed environment `{environment_reference}` is not in the approved catalog"
                ))
            })?
            .to_string();
        let node_kinds = manifest
            .nodes
            .iter()
            .map(|node| node.kind.clone())
            .collect::<Vec<_>>();
        let proposal = Proposal {
            schema_version: 1,
            proposal_id,
            plugin_name: plugin_name.to_string(),
            node_kinds: node_kinds.clone(),
            base_node_kinds: Some(node_kinds),
            action: ProposalAction::UpdatePlugin,
            status: ProposalStatus::Draft,
            authored_by: "agent".to_string(),
            request_ids: request_ids.to_vec(),
            environment_id: Some(environment_id),
            environment_reference: Some(environment_reference),
            base_commit: Some(source.commit.clone()),
            base_remote: Some(source.remote.clone()),
            source_commit: Some(source.commit.clone()),
            remote: Some(source.remote.clone()),
            pushed_commit: None,
            pull_request_number: None,
            pull_request_url: None,
            merged_commit: None,
            latest_report: None,
            rationale: rationale.trim().to_string(),
            created_at: now,
            updated_at: now,
        };
        self.save(&proposal)?;
        Ok(PluginDevelopment::new(self, proposal))
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

    /// Open the development handle for one existing proposal.
    pub fn develop(&self, id: &str) -> Result<Option<PluginDevelopment<'_>>> {
        Ok(self
            .find(id)?
            .map(|proposal| PluginDevelopment::new(self, proposal)))
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
        let current = self.load(id)?;
        if current.action != ProposalAction::NewPlugin
            || current.status != ProposalStatus::Publishing
        {
            return Err(Error::InvalidTransition {
                from: format!("{:?}", current.status),
                to: "published".into(),
            });
        }
        self.update(id, |proposal| {
            proposal.status = ProposalStatus::Published;
            proposal.remote = Some(remote.to_string());
            proposal.pushed_commit = Some(commit.to_string());
        })
    }

    pub fn mark_publish_failed(&self, id: &str) -> Result<Proposal> {
        self.transition(id, ProposalStatus::PublishFailed)
    }

    /// Record the GitHub PR opened for an approved update proposal.
    pub fn mark_pull_request_open(
        &self,
        id: &str,
        remote: &str,
        commit: &str,
        number: u64,
        url: &str,
    ) -> Result<Proposal> {
        let current = self.load(id)?;
        if current.action != ProposalAction::UpdatePlugin
            || current.status != ProposalStatus::Publishing
        {
            return Err(Error::InvalidTransition {
                from: format!("{:?}", current.status),
                to: "pull_request_open".into(),
            });
        }
        self.update(id, |proposal| {
            proposal.status = ProposalStatus::PullRequestOpen;
            proposal.remote = Some(remote.to_string());
            proposal.pushed_commit = Some(commit.to_string());
            proposal.pull_request_number = Some(number);
            proposal.pull_request_url = Some(url.to_string());
        })
    }

    /// Record the merge commit produced for one update PR.
    pub fn mark_pull_request_merged(&self, id: &str, commit: &str) -> Result<Proposal> {
        let current = self.load(id)?;
        if current.action != ProposalAction::UpdatePlugin
            || current.status != ProposalStatus::PullRequestOpen
        {
            return Err(Error::InvalidTransition {
                from: format!("{:?}", current.status),
                to: "pull_request_merged".into(),
            });
        }
        self.update(id, |proposal| {
            proposal.status = ProposalStatus::PullRequestMerged;
            proposal.merged_commit = Some(commit.to_string());
        })
    }

    pub fn mark_install_pending(&self, id: &str) -> Result<Proposal> {
        self.transition(id, ProposalStatus::InstallPending)
    }

    pub fn mark_installed(&self, id: &str) -> Result<Proposal> {
        self.transition(id, ProposalStatus::Installed)
    }

    pub(crate) fn transition(&self, id: &str, to: ProposalStatus) -> Result<Proposal> {
        let proposal = self.load(id)?;
        self.ensure_transition(proposal.status, to)?;
        self.update(id, |proposal| proposal.status = to)
    }

    pub(crate) fn update<F>(&self, id: &str, mutate: F) -> Result<Proposal>
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

    pub(crate) fn ensure_transition(&self, from: ProposalStatus, to: ProposalStatus) -> Result<()> {
        let valid = match (from, to) {
            (ProposalStatus::Draft, ProposalStatus::Validating)
            | (ProposalStatus::NeedsFix, ProposalStatus::Validating)
            | (ProposalStatus::Validating, ProposalStatus::NeedsFix)
            | (ProposalStatus::Validating, ProposalStatus::PendingReview)
            | (ProposalStatus::PendingReview, ProposalStatus::Approved)
            | (ProposalStatus::PendingReview, ProposalStatus::Rejected)
            | (ProposalStatus::Approved, ProposalStatus::Publishing)
            | (ProposalStatus::Publishing, ProposalStatus::Published)
            | (ProposalStatus::Publishing, ProposalStatus::PullRequestOpen)
            | (ProposalStatus::Publishing, ProposalStatus::PublishFailed)
            | (ProposalStatus::PublishFailed, ProposalStatus::Approved)
            | (ProposalStatus::PullRequestOpen, ProposalStatus::PullRequestMerged)
            | (ProposalStatus::Published, ProposalStatus::InstallPending)
            | (ProposalStatus::PullRequestMerged, ProposalStatus::InstallPending)
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

    pub(crate) fn load(&self, id: &str) -> Result<Proposal> {
        self.find(id)?
            .ok_or_else(|| Error::InvalidRequest(format!("unknown proposal {id:?}")))
    }

    fn save(&self, proposal: &Proposal) -> Result<()> {
        atomic_toml(&self.manifest_path(&proposal.proposal_id), proposal)
    }

    fn manifest_path(&self, id: &str) -> PathBuf {
        self.proposal_path(id).join("proposal.toml")
    }

    pub(crate) fn proposal_path(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub(crate) fn author_name(&self) -> &str {
        &self.author_name
    }

    pub(crate) fn author_email(&self) -> &str {
        &self.author_email
    }

    fn validate_requests(
        &self,
        plugin_name: &str,
        request_ids: &[String],
        requests: &RequestStore,
        rationale: &str,
    ) -> Result<BTreeSet<String>> {
        crate::validate_plugin_name(plugin_name)?;
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
        if rationale.trim().is_empty() {
            return Err(Error::InvalidRequest("rationale is required".into()));
        }
        Ok(requested)
    }
}

fn validate_update_source(source: &InstalledPluginSource) -> Result<()> {
    if !source.remote.starts_with("https://github.com/")
        && !source.remote.starts_with("git@github.com:")
    {
        return Err(Error::Validation(
            "update proposals require a GitHub source".into(),
        ));
    }
    if source.commit.len() != 40 || !source.commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Validation(
            "update base commit must be a full 40-hex SHA".into(),
        ));
    }
    Ok(())
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
    use crate::{Environment, EnvironmentCatalog};

    const ENVIRONMENT_REFERENCE: &str = "docker.io/library/hello-world@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c";

    fn fixture(tmp: &Path) -> (RequestStore, ProposalStore, EnvironmentCatalog) {
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
                evidence_ids: Vec::new(),
                status: RequestStatus::Open,
            })
            .unwrap();
        let proposals = ProposalStore::open(tmp, "main", "RSI Test", "rsi@example.com");
        let mut catalog = EnvironmentCatalog::default();
        catalog.insert(
            "demo",
            Environment {
                reference: ENVIRONMENT_REFERENCE.into(),
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
        let mut development = proposals
            .create(
                "demo-plugin",
                &[request_id],
                "Needed for testing",
                &requests,
            )
            .unwrap();
        assert_eq!(development.proposal().environment_id, None);
        assert_eq!(development.proposal().environment_reference, None);
        let id = development.id().to_string();
        let reopened = proposals.develop(&id).unwrap().unwrap();
        assert_eq!(reopened.id(), development.id());
        development.bind_environment("demo", &catalog).unwrap();

        let workspace = development.workspace();
        workspace.write_text("README.md", "# demo\n").unwrap();
        assert!(development.submit().is_err());
    }
}
