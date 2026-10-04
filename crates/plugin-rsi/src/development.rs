use std::path::PathBuf;

use crate::{
    ApprovedImage, Error, GitRepo, Proposal, ProposalStatus, ProposalStore, Result,
    validate::ImageCatalog, workspace::ProposalWorkspace,
};

/// An operational handle to one plugin proposal.
///
/// [`ProposalStore`] owns creation, lookup, review, and publication records.
/// This type owns the narrower development workflow for one proposal: binding
/// an image, exposing its workspace, recording validation, snapshotting git
/// state, and moving it to pending review.
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

    /// Bind an image already trusted by the runtime.
    ///
    /// Newly developed images will use a separate trusted build-result path;
    /// an Agent never supplies a raw digest through this method.
    pub fn bind_approved_image(
        &mut self,
        image_id: &str,
        catalog: &ImageCatalog,
    ) -> Result<Proposal> {
        self.refresh()?;
        if !matches!(
            self.proposal.status,
            ProposalStatus::Draft | ProposalStatus::Validating | ProposalStatus::NeedsFix
        ) {
            return Err(Error::Validation(format!(
                "image cannot be bound from status {:?}",
                self.proposal.status
            )));
        }
        let image = catalog.get(image_id).ok_or_else(|| {
            Error::Validation(format!("image {image_id:?} is not approved for RSI"))
        })?;
        let reference = image.reference.to_string();
        self.mutate(|proposal| {
            proposal.image_id = Some(image_id.to_string());
            proposal.image_reference = Some(reference);
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
        if self.proposal.image_reference.is_none() {
            return Err(Error::Validation(
                "proposal has no digest-pinned image".into(),
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

    fn refresh(&mut self) -> Result<()> {
        self.proposal = self.store.load(self.id())?;
        Ok(())
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
