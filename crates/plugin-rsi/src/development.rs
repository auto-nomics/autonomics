use std::path::PathBuf;

use crate::{
    DevelopmentChanges, Error, GitRepo, Proposal, ProposalStatus, ProposalStore, Result, candidate,
    node::NodeDevelopment, validate::EnvironmentCatalog, workspace::ProposalWorkspace,
};
use container_plugin::{
    manifest::{ImageMetadata, PluginManifest, PluginStatus},
    node_definition::{self, NodeDefinition},
};
use container_runtime::ImageReference;

/// An operational handle to one plugin proposal.
///
/// [`ProposalStore`] owns creation, lookup, review, and publication records.
/// This type owns the narrower development workflow for one proposal: binding
/// an environment, maintaining manifest nodes, recording validation,
/// snapshotting git state, and moving it to pending review.
#[derive(Debug, Clone)]
pub struct PluginDevelopment<'a> {
    store: &'a ProposalStore,
    proposal: Proposal,
}

impl<'a> PluginDevelopment<'a> {
    pub(crate) fn new(store: &'a ProposalStore, proposal: Proposal) -> Self {
        Self { store, proposal }
    }

    /// Return the persisted metadata snapshot backing this handle.
    pub fn proposal(&self) -> &Proposal {
        &self.proposal
    }

    /// Return the proposal id.
    pub fn id(&self) -> &str {
        &self.proposal.proposal_id
    }

    /// Consume the handle and return its metadata snapshot.
    pub fn into_proposal(self) -> Proposal {
        self.proposal
    }

    /// Return the safe file workspace containing the plugin repository.
    pub fn workspace(&self) -> ProposalWorkspace {
        ProposalWorkspace::new(self.store.proposal_path(self.id()).join("repo"))
    }

    /// Return the append-only report directory for this proposal.
    pub fn reports_dir(&self) -> PathBuf {
        self.store.proposal_path(self.id()).join("reports")
    }

    /// List every node defined by this plugin's manifest.
    pub fn list_nodes(&self) -> Result<Vec<NodeDefinition>> {
        Ok(self.load_manifest()?.nodes)
    }

    /// Read one node by kind.
    pub fn read_node(&self, kind: &str) -> Result<Option<NodeDefinition>> {
        Ok(self
            .list_nodes()?
            .into_iter()
            .find(|node| node.kind == kind))
    }

    /// Create a node and return a development handle for it.
    pub fn create_node(&mut self, node: NodeDefinition) -> Result<NodeDevelopment<'_, 'a>> {
        self.refresh()?;
        self.ensure_nodes_editable()?;
        self.validate_node(&node)?;
        let mut manifest = self.load_manifest()?;
        if manifest
            .nodes
            .iter()
            .any(|existing| existing.kind == node.kind)
        {
            return Err(Error::Validation(format!(
                "node kind `{}` already exists",
                node.kind
            )));
        }
        let kind = node.kind.clone();
        manifest.nodes.push(node);
        let node_kinds = manifest
            .nodes
            .iter()
            .map(|node| node.kind.clone())
            .collect::<Vec<_>>();
        self.save_manifest(&manifest)?;
        self.mutate(|proposal| proposal.node_kinds = node_kinds)?;
        Ok(NodeDevelopment::new(self, kind))
    }

    /// Open a development handle for an existing node.
    pub fn node(&mut self, kind: &str) -> Result<Option<NodeDevelopment<'_, 'a>>> {
        self.refresh()?;
        if self.read_node(kind)?.is_some() {
            Ok(Some(NodeDevelopment::new(self, kind)))
        } else {
            Ok(None)
        }
    }

    /// Replace an existing node definition with the same kind.
    pub fn update_node(&mut self, node: NodeDefinition) -> Result<NodeDefinition> {
        self.refresh()?;
        self.ensure_nodes_editable()?;
        self.validate_node(&node)?;
        let mut manifest = self.load_manifest()?;
        let index = manifest
            .nodes
            .iter()
            .position(|existing| existing.kind == node.kind)
            .ok_or_else(|| Error::Validation(format!("node `{}` does not exist", node.kind)))?;
        manifest.nodes[index] = node.clone();
        let node_kinds = manifest
            .nodes
            .iter()
            .map(|node| node.kind.clone())
            .collect::<Vec<_>>();
        self.save_manifest(&manifest)?;
        self.mutate(|proposal| proposal.node_kinds = node_kinds)?;
        Ok(node)
    }

    /// Delete a node from the plugin manifest.
    pub fn delete_node(&mut self, kind: &str) -> Result<Proposal> {
        self.refresh()?;
        self.ensure_nodes_editable()?;
        let mut manifest = self.load_manifest()?;
        let before = manifest.nodes.len();
        manifest.nodes.retain(|node| node.kind != kind);
        if manifest.nodes.len() == before {
            return Err(Error::Validation(format!("node `{kind}` does not exist")));
        }
        let node_kinds = manifest
            .nodes
            .iter()
            .map(|node| node.kind.clone())
            .collect::<Vec<_>>();
        self.save_manifest(&manifest)?;
        self.mutate(|proposal| proposal.node_kinds = node_kinds)
    }

    /// Create a detached candidate repository for one specialized agent run.
    ///
    /// The returned workspace is the only repository an agent may touch. The
    /// daemon does not adopt any candidate until
    /// [`PluginDevelopment::adopt_development_candidate`] validates its diff.
    pub fn prepare_development_candidate(&mut self, run_id: &str) -> Result<ProposalWorkspace> {
        self.refresh()?;
        self.ensure_development_candidate_editable()?;
        self.environment_reference()?;
        let root = self.development_candidate_root(run_id)?;
        candidate::prepare_candidate_workspace(&root, &self.workspace())
    }

    /// Validate and adopt one specialized development candidate.
    ///
    /// Adoption is daemon-owned and atomic at the file-operation level: the
    /// candidate is checked first, then only safe relative text files are
    /// copied back into the proposal repository.
    pub fn adopt_development_candidate(&mut self, run_id: &str) -> Result<DevelopmentChanges> {
        self.refresh()?;
        self.ensure_development_candidate_editable()?;
        let environment_reference = self.environment_reference()?;
        let source = self.workspace();
        let candidate_root = self.development_candidate_root(run_id)?;
        let candidate_workspace = ProposalWorkspace::new(candidate_root.join("workspace"));
        if !candidate_workspace.path().is_dir() {
            return Err(Error::Validation(format!(
                "development candidate `{run_id}` does not exist"
            )));
        }
        let changes = candidate::workspace_changes(&source, &candidate_workspace)?;
        if changes.is_empty() {
            return Err(Error::Validation(format!(
                "development candidate `{run_id}` has no changes"
            )));
        }
        let manifest = candidate::validate_candidate(
            &candidate_workspace,
            &self.proposal.plugin_name,
            &environment_reference,
        )?;
        candidate::adopt_candidate(&source, &candidate_workspace, &changes)?;
        let node_kinds = manifest
            .nodes
            .iter()
            .map(|node| node.kind.clone())
            .collect::<Vec<_>>();
        self.mutate(|proposal| {
            proposal.node_kinds = node_kinds;
            proposal.latest_report = None;
            if proposal.status == ProposalStatus::Validating {
                proposal.status = ProposalStatus::NeedsFix;
            }
        })?;
        Ok(changes.into())
    }

    /// Bind a runtime environment already trusted by the daemon.
    ///
    /// Ordinary plugin proposals never own or build an image. New environments
    /// are managed as separate reusable assets and bound by id, never by an
    /// Agent-supplied digest.
    pub fn bind_environment(
        &mut self,
        environment_id: &str,
        catalog: &EnvironmentCatalog,
    ) -> Result<Proposal> {
        self.refresh()?;
        if !matches!(
            self.proposal.status,
            ProposalStatus::Draft | ProposalStatus::Validating | ProposalStatus::NeedsFix
        ) {
            return Err(Error::Validation(format!(
                "environment cannot be bound from status {:?}",
                self.proposal.status
            )));
        }
        let environment = catalog.get(environment_id).ok_or_else(|| {
            Error::Validation(format!(
                "environment {environment_id:?} is not approved for RSI"
            ))
        })?;
        let reference = ImageReference::parse(&environment.reference).map_err(|error| {
            Error::Validation(format!("invalid environment reference: {error}"))
        })?;
        let workspace = self.workspace();
        let manifest_path = workspace.path().join("manifest.toml");
        let mut manifest = if manifest_path.is_file() {
            let mut manifest = self.load_manifest()?;
            manifest.image.reference = reference.clone();
            manifest
        } else {
            let mut image = ImageMetadata::default();
            image.reference = reference.clone();
            PluginManifest {
                schema_version: 1,
                plugin_name: self.proposal.plugin_name.clone(),
                status: PluginStatus::Draft,
                image,
                panels: Vec::new(),
                nodes: Vec::new(),
            }
        };
        manifest.image.reference = reference;
        self.save_manifest(&manifest)?;
        let reference = environment.reference.to_string();
        self.mutate(|proposal| {
            proposal.environment_id = Some(environment_id.to_string());
            proposal.environment_reference = Some(reference);
        })
    }

    /// Move the proposal into validation before running gates.
    pub fn start_validation(&mut self) -> Result<Proposal> {
        self.refresh()?;
        let to = ProposalStatus::Validating;
        self.store.ensure_transition(self.proposal.status, to)?;
        self.mutate(|proposal| proposal.status = to)
    }

    /// Attach an infrastructure-generated validation report.
    pub fn record_report(
        &mut self,
        relative_report: &str,
        report: &crate::ValidationReport,
    ) -> Result<Proposal> {
        self.refresh()?;
        self.mutate(|proposal| {
            proposal.latest_report = Some(relative_report.to_string());
            if proposal.status == ProposalStatus::Validating && !report.passed() {
                proposal.status = ProposalStatus::NeedsFix;
            }
        })
    }

    /// Snapshot all workspace content as a daemon-authored git commit.
    pub fn snapshot(&mut self, message: &str) -> Result<Option<String>> {
        self.refresh()?;
        if !matches!(
            self.proposal.status,
            ProposalStatus::Draft | ProposalStatus::Validating | ProposalStatus::NeedsFix
        ) {
            return Err(Error::InvalidTransition {
                from: format!("{:?}", self.proposal.status),
                to: "snapshot".into(),
            });
        }
        let repo = GitRepo::open(self.store.proposal_path(self.id()).join("repo"));
        let commit =
            repo.snapshot_commit(message, self.store.author_name(), self.store.author_email())?;
        if let Some(commit) = &commit {
            let commit = commit.clone();
            self.mutate(|proposal| proposal.source_commit = Some(commit))?;
        }
        Ok(commit)
    }

    /// Submit the reviewed content for human review.
    pub fn submit(&mut self) -> Result<Proposal> {
        self.refresh()?;
        let to = ProposalStatus::PendingReview;
        self.store.ensure_transition(self.proposal.status, to)?;
        let repo = GitRepo::open(self.store.proposal_path(self.id()).join("repo"));
        if !repo.is_clean()? {
            return Err(Error::Validation(
                "working tree is dirty; snapshot the reviewed content first".into(),
            ));
        }
        let head = repo.head()?;
        if self.proposal.source_commit.as_deref() != Some(head.as_str()) {
            return Err(Error::Validation(
                "proposal source_commit does not match repository HEAD".into(),
            ));
        }
        if self.proposal.environment_reference.is_none() {
            return Err(Error::Validation(
                "proposal has no digest-pinned environment".into(),
            ));
        }
        let report = self.latest_report()?;
        if !report.passed() {
            return Err(Error::Validation(
                "latest validation report did not pass".into(),
            ));
        }
        self.mutate(|proposal| proposal.status = to)
    }

    /// Load the current validation report referenced by the proposal.
    pub fn latest_report(&self) -> Result<crate::ValidationReport> {
        let relative = self
            .proposal
            .latest_report
            .as_deref()
            .ok_or_else(|| Error::Validation("proposal has no validation report".into()))?;
        let path = self.store.proposal_path(self.id()).join(relative);
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

    /// Begin trusted publication of an approved proposal.
    pub fn start_publish(&mut self) -> Result<Proposal> {
        self.refresh()?;
        self.store.start_publish(self.id())
    }

    /// Record a successful publication push.
    pub fn mark_published(&mut self, remote: &str, commit: &str) -> Result<Proposal> {
        let proposal = self.store.mark_published(self.id(), remote, commit)?;
        self.proposal = proposal.clone();
        Ok(proposal)
    }

    /// Record a failed publication push and return the proposal to repair.
    pub fn mark_publish_failed(&mut self) -> Result<Proposal> {
        let proposal = self.store.mark_publish_failed(self.id())?;
        self.proposal = proposal.clone();
        Ok(proposal)
    }

    /// Record the PR opened for an approved update proposal.
    pub fn mark_pull_request_open(
        &mut self,
        remote: &str,
        commit: &str,
        number: u64,
        url: &str,
    ) -> Result<Proposal> {
        let proposal = self
            .store
            .mark_pull_request_open(self.id(), remote, commit, number, url)?;
        self.proposal = proposal.clone();
        Ok(proposal)
    }

    /// Record the merge commit of an update proposal.
    pub fn mark_pull_request_merged(&mut self, commit: &str) -> Result<Proposal> {
        let proposal = self.store.mark_pull_request_merged(self.id(), commit)?;
        self.proposal = proposal.clone();
        Ok(proposal)
    }

    /// Begin installation of a published proposal.
    pub fn mark_install_pending(&mut self) -> Result<Proposal> {
        let proposal = self.store.mark_install_pending(self.id())?;
        self.proposal = proposal.clone();
        Ok(proposal)
    }

    /// Record completion of the local plugin source installation.
    pub fn mark_installed(&mut self) -> Result<Proposal> {
        let proposal = self.store.mark_installed(self.id())?;
        self.proposal = proposal.clone();
        Ok(proposal)
    }

    pub(crate) fn refresh(&mut self) -> Result<()> {
        self.proposal = self.store.load(self.id())?;
        Ok(())
    }

    fn ensure_nodes_editable(&self) -> Result<()> {
        if matches!(
            self.proposal.status,
            ProposalStatus::Draft | ProposalStatus::Validating | ProposalStatus::NeedsFix
        ) {
            Ok(())
        } else {
            Err(Error::Validation(format!(
                "nodes cannot be edited from status {:?}",
                self.proposal.status
            )))
        }
    }

    fn ensure_development_candidate_editable(&self) -> Result<()> {
        if matches!(
            self.proposal.status,
            ProposalStatus::Draft | ProposalStatus::Validating | ProposalStatus::NeedsFix
        ) {
            Ok(())
        } else {
            Err(Error::Validation(format!(
                "development candidates cannot run from status {:?}",
                self.proposal.status
            )))
        }
    }

    fn environment_reference(&self) -> Result<String> {
        self.proposal.environment_reference.clone().ok_or_else(|| {
            Error::Validation(
                "proposal must bind an environment before specialized development".into(),
            )
        })
    }

    fn development_candidate_root(&self, run_id: &str) -> Result<std::path::PathBuf> {
        let valid = !run_id.is_empty()
            && run_id.len() <= 128
            && run_id.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '-'
                    || character == '_'
            });
        if !valid {
            return Err(Error::Validation(
                "development candidate id must be `[a-z0-9_-]` and at most 128 bytes".into(),
            ));
        }
        Ok(self
            .store
            .proposal_path(self.id())
            .join("development-candidates")
            .join(run_id))
    }

    fn validate_node(&self, node: &NodeDefinition) -> Result<()> {
        node_definition::validate(node)
            .map_err(|error| Error::Validation(format!("invalid node `{}`: {error}", node.kind)))?;
        if node.command.script.is_some() || node.command.script_file.is_none() {
            return Err(Error::Validation(
                "RSI nodes must declare a safe relative script_file".into(),
            ));
        }
        let script_file = node.command.script_file.as_deref().expect("checked above");
        match self.workspace().read_text(script_file) {
            Ok(_) => Ok(()),
            Err(Error::ReadFile { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn load_manifest(&self) -> Result<PluginManifest> {
        let path = self.workspace().path().join("manifest.toml");
        let text =
            std::fs::read_to_string(&path).map_err(|source| Error::ReadFile { path, source })?;
        let manifest: PluginManifest =
            toml::from_str(&text).map_err(|source| Error::ParseToml {
                path: self.workspace().path().join("manifest.toml"),
                source,
            })?;
        if manifest.plugin_name != self.proposal.plugin_name {
            return Err(Error::Validation(format!(
                "manifest plugin_name `{}` does not match proposal `{}`",
                manifest.plugin_name, self.proposal.plugin_name
            )));
        }
        Ok(manifest)
    }

    fn save_manifest(&self, manifest: &PluginManifest) -> Result<()> {
        let path = self.workspace().path().join("manifest.toml");
        let text = toml::to_string_pretty(manifest).map_err(|source| Error::SerializeToml {
            path: path.clone(),
            source,
        })?;
        self.workspace().write_text("manifest.toml", &text)
    }

    fn mutate<F>(&mut self, mutate: F) -> Result<Proposal>
    where
        F: FnOnce(&mut Proposal),
    {
        let proposal = self.store.update(self.id(), mutate)?;
        self.proposal = proposal.clone();
        Ok(proposal)
    }
}
