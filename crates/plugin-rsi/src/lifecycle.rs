//! Host-owned orchestration across the plugin development lifecycle.

use std::path::Path;

use crate::{
    EnvironmentCatalog, Error, GitRepo, PluginDevelopment, PluginPublisher, Proposal,
    ProposalAction, ProposalStatus, PublishOutcome, Result, ValidationReport, validate_workspace,
};

/// Result of one host validation cycle.
#[derive(Debug, Clone)]
pub enum ValidationOutcome {
    /// The proposal passed all gates, was snapshotted, and entered review.
    Submitted(ValidationReport),
    /// A gate failed and the proposal returned to its repair state.
    NeedsFix(ValidationReport),
}

/// A thin orchestrator over one plugin development handle.
///
/// This type does not grant new authority. It connects the existing
/// host-owned validation, git, review, publication, and installation steps so
/// callers do not need to reassemble the state-machine sequence themselves.
pub struct PluginLifecycle<'a, 'b> {
    development: &'a mut PluginDevelopment<'b>,
    catalog: &'a EnvironmentCatalog,
    installed_node_kinds: &'a [String],
}

impl<'a, 'b> PluginLifecycle<'a, 'b> {
    /// Open the lifecycle for one development handle.
    pub fn new(
        development: &'a mut PluginDevelopment<'b>,
        catalog: &'a EnvironmentCatalog,
        installed_node_kinds: &'a [String],
    ) -> Self {
        Self {
            development,
            catalog,
            installed_node_kinds,
        }
    }

    /// Return the current metadata snapshot for the underlying proposal.
    pub fn proposal(&self) -> &Proposal {
        self.development.proposal()
    }

    /// Validate the proposal, persist append-only evidence, and submit passes.
    pub fn validate_and_submit(&mut self) -> Result<ValidationOutcome> {
        self.development.start_validation()?;
        let proposal = self.development.proposal().clone();
        let attempt = next_validation_attempt(&self.development.reports_dir())?;
        let report = validate_workspace(
            &proposal,
            &self.development.workspace(),
            self.catalog,
            self.installed_node_kinds,
            attempt,
        );
        let path = report.write(&self.development.reports_dir())?;
        let relative = path
            .strip_prefix(self.development.reports_dir().parent().ok_or_else(|| {
                Error::Validation("proposal report directory has no parent".into())
            })?)
            .map_err(|_| Error::Validation("report path escapes its proposal".into()))?
            .to_string_lossy()
            .replace('\\', "/");
        self.development.record_report(&relative, &report)?;
        if !report.passed() {
            return Ok(ValidationOutcome::NeedsFix(report));
        }

        self.development
            .snapshot(&format!("validation: attempt {attempt}"))?;
        self.development.submit()?;
        Ok(ValidationOutcome::Submitted(report))
    }

    /// Publish an approved proposal with the configured trusted publisher.
    pub fn publish_reviewed(&mut self, publisher: &dyn PluginPublisher) -> Result<Proposal> {
        self.development.refresh()?;
        if self.development.proposal().status != ProposalStatus::Approved {
            return Err(Error::InvalidTransition {
                from: format!("{:?}", self.development.proposal().status),
                to: "publishing".into(),
            });
        }
        if self.development.proposal().action != ProposalAction::NewPlugin {
            return Err(Error::Validation(
                "update proposals must be published through a pull request".into(),
            ));
        }
        let repository = GitRepo::open(
            self.development
                .reports_dir()
                .parent()
                .ok_or_else(|| Error::Validation("proposal directory is missing".into()))?
                .join("repo"),
        );
        let head = repository.head()?;
        if !repository.is_clean()? {
            return Err(Error::Validation(
                "approved plugin repository has uncommitted content".into(),
            ));
        }
        if self.development.proposal().source_commit.as_deref() != Some(head.as_str()) {
            return Err(Error::Validation(
                "approved source_commit does not match repository HEAD".into(),
            ));
        }

        self.development.start_publish()?;
        let plugin_name = self.development.proposal().plugin_name.clone();
        let outcome = match publisher.publish_plugin(&plugin_name, &repository) {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = self.development.mark_publish_failed();
                return Err(error);
            }
        };
        if outcome.commit != head {
            let _ = self.development.mark_publish_failed();
            return Err(Error::Validation(
                "publisher returned a commit that is not repository HEAD".into(),
            ));
        }
        let PublishOutcome { remote, commit, .. } = outcome;
        self.development.mark_published(&remote, &commit)
    }

    /// Push an approved update and open its trusted GitHub pull request.
    pub fn open_update_pull_request(
        &mut self,
        publisher: &dyn crate::PluginPullRequestPublisher,
    ) -> Result<Proposal> {
        self.development.refresh()?;
        let proposal = self.development.proposal().clone();
        if proposal.status != ProposalStatus::Approved {
            return Err(Error::InvalidTransition {
                from: format!("{:?}", proposal.status),
                to: "publishing".into(),
            });
        }
        if proposal.action != ProposalAction::UpdatePlugin {
            return Err(Error::Validation(
                "new plugins must publish to their default branch".into(),
            ));
        }
        let repository = GitRepo::open(
            self.development
                .reports_dir()
                .parent()
                .ok_or_else(|| Error::Validation("proposal directory is missing".into()))?
                .join("repo"),
        );
        let head = repository.head()?;
        if !repository.is_clean()? {
            return Err(Error::Validation(
                "approved plugin repository has uncommitted content".into(),
            ));
        }
        if proposal.source_commit.as_deref() != Some(head.as_str()) {
            return Err(Error::Validation(
                "approved source_commit does not match repository HEAD".into(),
            ));
        }
        let Some(base_commit) = proposal.base_commit.as_deref() else {
            return Err(Error::Validation(
                "update proposal has no base commit".into(),
            ));
        };
        let Some(base_remote) = proposal.base_remote.as_deref() else {
            return Err(Error::Validation(
                "update proposal has no base remote".into(),
            ));
        };
        if repository.remote_url("origin")?.as_deref() != Some(base_remote) {
            return Err(Error::Validation(
                "update workspace origin does not match the installed plugin source".into(),
            ));
        }
        if head == base_commit {
            return Err(Error::Validation(
                "update proposal contains no reviewed changes".into(),
            ));
        }

        self.development.start_publish()?;
        let plugin_name = proposal.plugin_name.clone();
        let branch = format!("plugin-rsi/{}", proposal.proposal_id.to_lowercase());
        let outcome = match publisher.open_pull_request(&plugin_name, &branch, &repository) {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = self.development.mark_publish_failed();
                return Err(error);
            }
        };
        if outcome.commit != head || outcome.remote != proposal.base_remote.as_deref().unwrap_or("")
        {
            let _ = self.development.mark_publish_failed();
            return Err(Error::Validation(
                "publisher returned an update identity that does not match the proposal".into(),
            ));
        }
        self.development.mark_pull_request_open(
            &outcome.remote,
            &outcome.commit,
            outcome.number,
            &outcome.url,
        )
    }

    /// Merge one open update PR and record its immutable merge commit.
    pub fn merge_update_pull_request(
        &mut self,
        publisher: &dyn crate::PluginPullRequestPublisher,
    ) -> Result<Proposal> {
        self.development.refresh()?;
        let proposal = self.development.proposal().clone();
        if proposal.status != ProposalStatus::PullRequestOpen {
            return Err(Error::InvalidTransition {
                from: format!("{:?}", proposal.status),
                to: "pull_request_merged".into(),
            });
        }
        let Some(remote) = proposal.remote.clone() else {
            return Err(Error::Validation("open update PR has no remote".into()));
        };
        let Some(number) = proposal.pull_request_number else {
            return Err(Error::Validation("open update PR has no number".into()));
        };
        let outcome = publisher.merge_pull_request(&remote, number)?;
        if outcome.remote != remote
            || outcome.commit == proposal.base_commit.as_deref().unwrap_or("")
        {
            return Err(Error::Validation(
                "publisher returned an invalid merge identity".into(),
            ));
        }
        self.development.mark_pull_request_merged(&outcome.commit)
    }

    /// Write a published plugin source into local plugin configuration.
    pub fn install(&mut self, config_path: &Path) -> Result<Proposal> {
        self.development.refresh()?;
        let proposal = self.development.proposal().clone();
        if !matches!(
            (proposal.action, proposal.status),
            (ProposalAction::NewPlugin, ProposalStatus::Published)
                | (
                    ProposalAction::UpdatePlugin,
                    ProposalStatus::PullRequestMerged
                )
        ) {
            return Err(Error::InvalidTransition {
                from: format!("{:?}", proposal.status),
                to: "install_pending".into(),
            });
        }
        let remote = proposal
            .remote
            .clone()
            .ok_or_else(|| Error::Validation("published proposal has no remote".into()))?;
        let commit = match proposal.action {
            ProposalAction::NewPlugin => proposal
                .pushed_commit
                .clone()
                .ok_or_else(|| Error::Validation("published proposal has no commit".into()))?,
            ProposalAction::UpdatePlugin => proposal
                .merged_commit
                .clone()
                .ok_or_else(|| Error::Validation("merged update proposal has no commit".into()))?,
        };

        self.development.mark_install_pending()?;
        crate::write_git_plugin_source(config_path, &proposal.plugin_name, &remote, &commit)?;
        self.development.mark_installed()
    }
}

fn next_validation_attempt(reports_dir: &Path) -> Result<u32> {
    let entries = match std::fs::read_dir(reports_dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(1),
        Err(source) => return Err(source.into()),
    };
    let mut attempt = 0;
    for entry in entries {
        let entry = entry?;
        let filename = entry.file_name();
        let Some(name) = filename.to_str() else {
            continue;
        };
        let Some(value) = name
            .strip_prefix("attempt-")
            .and_then(|value| value.strip_suffix(".json"))
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        attempt = attempt.max(value);
    }
    Ok(attempt + 1)
}
