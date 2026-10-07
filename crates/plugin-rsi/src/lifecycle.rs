//! Host-owned orchestration for the unified plugin lifecycle.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::{
    EnvironmentCatalog, Error, PluginOperator, PluginPublisher, PluginPullRequestPublisher,
    PluginStatus, Result, ValidationReport, validate_workspace,
};

/// Result of one validation cycle.
#[derive(Debug, Clone)]
pub enum ValidationOutcome {
    /// The plugin passed all gates and entered review.
    Submitted(ValidationReport),
    /// The plugin passed all gates and remained in its local development state.
    Passed(ValidationReport),
    /// A gate failed and the plugin returned to its repair state.
    NeedsFix(ValidationReport),
}

/// Result of the fast local-activation path.
#[derive(Debug, Clone)]
pub enum LocalActivationOutcome {
    /// Validation passed and the immutable local snapshot is now runtime-active.
    Activated(ValidationReport, crate::LocalInstalledPluginSource),
    /// Validation failed and the development workspace remains repairable.
    NeedsFix(ValidationReport),
}

/// Thin orchestrator over one long-lived plugin workspace.
pub struct PluginLifecycle<'a, 'b> {
    operator: &'a mut PluginOperator<'b>,
    catalog: &'a EnvironmentCatalog,
    installed_node_kinds: &'a [String],
}

impl<'a, 'b> PluginLifecycle<'a, 'b> {
    pub fn new(
        operator: &'a mut PluginOperator<'b>,
        catalog: &'a EnvironmentCatalog,
        installed_node_kinds: &'a [String],
    ) -> Self {
        Self {
            operator,
            catalog,
            installed_node_kinds,
        }
    }

    pub fn plugin(&self) -> &container_plugin::manifest::PluginManifest {
        self.operator.manifest()
    }

    /// Validate the plugin, append evidence, snapshot passes, and enter review.
    pub fn validate_and_submit(&mut self) -> Result<ValidationOutcome> {
        self.operator.refresh()?;
        let report = self.validate_common()?;

        if !report.passed() {
            self.operator
                .mutate_manifest(|manifest| manifest.status = PluginStatus::NeedsFix)?;
            return Ok(ValidationOutcome::NeedsFix(report));
        }

        self.operator
            .snapshot(&format!("validation: {}", report.report_id))?;
        self.ensure_clean()?;
        self.operator
            .transition_snapshot(PluginStatus::PendingReview, "lifecycle: submit review")?;
        Ok(ValidationOutcome::Submitted(report))
    }

    /// Validate for local activation without entering the review workflow.
    ///
    /// A passing workspace returns to its normal editable development status.
    /// Runtime activation itself is recorded separately in installation
    /// metadata when the immutable snapshot is installed.
    pub fn validate_local(&mut self) -> Result<ValidationOutcome> {
        self.operator.refresh()?;
        let development_status = self.operator.status();
        let report = self.validate_common()?;

        if !report.passed() {
            self.operator.mutate_manifest(|manifest| {
                manifest.status = PluginStatus::NeedsFix;
                manifest.lifecycle.publication_pending = false;
            })?;
            return Ok(ValidationOutcome::NeedsFix(report));
        }

        self.operator
            .snapshot(&format!("validation: attempt {}", report.report_id))?;
        self.ensure_clean()?;
        let restore = if development_status == PluginStatus::Updating {
            PluginStatus::Updating
        } else {
            PluginStatus::Draft
        };
        self.operator.transition_snapshot(
            restore,
            "lifecycle: local validation passed; remain editable",
        )?;
        Ok(ValidationOutcome::Passed(report))
    }

    /// Apply human review and snapshot the decision.
    pub fn review(&mut self, approved: bool) -> Result<PluginStatus> {
        self.operator.refresh()?;
        let to = if approved {
            PluginStatus::Approved
        } else {
            PluginStatus::Rejected
        };
        self.operator
            .transition_snapshot(to, "lifecycle: review decision")?;
        Ok(to)
    }

    /// Publish a newly approved plugin to its default remote branch.
    pub fn publish_reviewed(&mut self, publisher: &dyn PluginPublisher) -> Result<PluginStatus> {
        self.operator.refresh()?;
        self.ensure_status(PluginStatus::Approved)?;
        self.ensure_clean()?;
        self.operator
            .transition_snapshot(PluginStatus::Publishing, "lifecycle: start publication")?;

        let repository = self.operator.repository();
        let head = repository.head()?;
        let outcome = match publisher.publish_plugin(self.operator.plugin_name(), &repository) {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = self
                    .operator
                    .mutate_manifest(|manifest| manifest.status = PluginStatus::PublishFailed);
                return Err(error);
            }
        };
        if outcome.commit != head {
            let _ = self
                .operator
                .mutate_manifest(|manifest| manifest.status = PluginStatus::PublishFailed);
            return Err(Error::Validation(
                "publisher returned a commit that is not plugin HEAD".into(),
            ));
        }
        self.operator.mutate_manifest(|manifest| {
            manifest.status = PluginStatus::Published;
            manifest.lifecycle.remote = Some(outcome.remote.clone());
            manifest.lifecycle.published_commit = Some(outcome.commit);
        })?;
        self.operator
            .snapshot("lifecycle: publish reviewed plugin")?;
        Ok(PluginStatus::Published)
    }

    /// Push an approved update and open its trusted pull request.
    pub fn open_update_pull_request(
        &mut self,
        publisher: &dyn PluginPullRequestPublisher,
    ) -> Result<PluginStatus> {
        self.operator.refresh()?;
        self.ensure_status(PluginStatus::Approved)?;
        self.ensure_clean()?;
        let repository = self.operator.repository();
        let head = repository.head()?;
        let base_commit = self
            .operator
            .manifest()
            .lifecycle
            .base_commit
            .clone()
            .ok_or_else(|| Error::Validation("update has no base commit".into()))?;
        let base_remote = self
            .operator
            .manifest()
            .lifecycle
            .base_remote
            .clone()
            .ok_or_else(|| Error::Validation("update has no base remote".into()))?;
        if head == base_commit {
            return Err(Error::Validation(
                "update contains no reviewed changes".into(),
            ));
        }
        if repository.remote_url("origin")?.as_deref() != Some(base_remote.as_str()) {
            return Err(Error::Validation(
                "update origin does not match its installed source".into(),
            ));
        }

        self.operator
            .transition_snapshot(PluginStatus::Publishing, "lifecycle: start update PR")?;
        let head = repository.head()?;
        let branch = update_branch(self.operator.plugin_name(), &base_commit, &head);
        let outcome =
            match publisher.open_pull_request(self.operator.plugin_name(), &branch, &repository) {
                Ok(outcome) => outcome,
                Err(error) => {
                    let _ = self
                        .operator
                        .mutate_manifest(|manifest| manifest.status = PluginStatus::PublishFailed);
                    return Err(error);
                }
            };
        if outcome.commit != head || outcome.remote != base_remote {
            let _ = self
                .operator
                .mutate_manifest(|manifest| manifest.status = PluginStatus::PublishFailed);
            return Err(Error::Validation(format!(
                "publisher returned update identity ({}, {}) that does not match plugin ({}, {})",
                outcome.commit, outcome.remote, base_remote, head
            )));
        }
        self.operator.mutate_manifest(|manifest| {
            manifest.status = PluginStatus::PullRequestOpen;
            manifest.lifecycle.remote = Some(outcome.remote.clone());
            manifest.lifecycle.pull_request_number = Some(outcome.number);
            manifest.lifecycle.pull_request_url = Some(outcome.url);
        })?;
        self.operator.snapshot("lifecycle: record update PR")?;
        Ok(PluginStatus::PullRequestOpen)
    }

    /// Merge one update PR and move the workspace to its immutable result.
    pub fn merge_update_pull_request(
        &mut self,
        publisher: &dyn PluginPullRequestPublisher,
    ) -> Result<PluginStatus> {
        self.operator.refresh()?;
        self.ensure_status(PluginStatus::PullRequestOpen)?;
        let remote = self
            .operator
            .manifest()
            .lifecycle
            .remote
            .clone()
            .ok_or_else(|| Error::Validation("open update PR has no remote".into()))?;
        let number = self
            .operator
            .manifest()
            .lifecycle
            .pull_request_number
            .ok_or_else(|| Error::Validation("open update PR has no number".into()))?;
        let outcome = publisher.merge_pull_request(&remote, number)?;
        if outcome.remote != remote {
            return Err(Error::Validation(
                "publisher returned the wrong update remote".into(),
            ));
        }
        self.operator
            .repository()
            .checkout_commit(&outcome.commit)?;
        self.operator.repository().switch_forced_branch("main")?;
        self.operator.mutate_manifest(|manifest| {
            manifest.status = PluginStatus::PullRequestMerged;
            manifest.lifecycle.merged_commit = Some(outcome.commit);
        })?;
        Ok(PluginStatus::PullRequestMerged)
    }

    /// Record installation and push the installed manifest status.
    pub fn install(&mut self, publisher: &dyn PluginPublisher) -> Result<PluginStatus> {
        if !matches!(
            self.operator.status(),
            PluginStatus::Published | PluginStatus::PullRequestMerged
        ) {
            return Err(Error::InvalidTransition {
                from: format!("{:?}", self.operator.status()),
                to: "install_pending".into(),
            });
        }
        self.operator
            .transition_snapshot(PluginStatus::InstallPending, "lifecycle: start install")?;
        self.operator
            .transition_snapshot(PluginStatus::Installed, "lifecycle: install plugin")?;
        let repository = self.operator.repository();
        if !repository.is_clean()? {
            return Err(Error::Validation(
                "installed plugin repository has uncommitted content".into(),
            ));
        }
        let repository = self.operator.repository();
        let outcome = publisher.publish_plugin(self.operator.plugin_name(), &repository)?;
        let remote = outcome.remote;
        if repository.head()? != outcome.commit {
            return Err(Error::Validation(
                "publisher returned an invalid installed commit".into(),
            ));
        }
        self.operator
            .install_registry_source(&remote, &outcome.commit)?;
        self.operator.mutate_manifest(|manifest| {
            manifest.lifecycle.publication_pending = false;
        })?;
        self.operator
            .snapshot("lifecycle: clear pending publication")?;
        Ok(PluginStatus::Installed)
    }

    fn validate_common(&mut self) -> Result<ValidationReport> {
        let owned_kinds = self.operator.owned_node_kinds();
        self.operator
            .transition_snapshot(PluginStatus::Validating, "lifecycle: start validation")?;
        let attempt = next_validation_attempt(&self.operator.reports_dir())?;
        let report = validate_workspace(
            self.operator.plugin_name(),
            &self.operator.workspace(),
            self.catalog,
            self.installed_node_kinds,
            &owned_kinds,
            attempt,
        );
        report.write(&self.operator.reports_dir())?;
        let relative = format!(".rsi/reports/attempt-{attempt}.json");
        self.operator.mutate_manifest(|manifest| {
            manifest.lifecycle.latest_report = Some(relative);
        })?;
        Ok(report)
    }

    fn ensure_status(&self, status: PluginStatus) -> Result<()> {
        if self.operator.status() == status {
            Ok(())
        } else {
            Err(Error::InvalidTransition {
                from: format!("{:?}", self.operator.status()),
                to: format!("{status:?}").to_lowercase(),
            })
        }
    }

    fn ensure_clean(&self) -> Result<()> {
        if self.operator.repository().is_clean()? {
            Ok(())
        } else {
            Err(Error::Validation(
                "plugin repository has uncommitted content".into(),
            ))
        }
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
        let name = entry?.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(value) = name
            .strip_prefix("attempt-")
            .and_then(|value| value.strip_suffix(".json"))
            .and_then(|value| value.parse::<u32>().ok())
        {
            attempt = attempt.max(value);
        }
    }
    Ok(attempt + 1)
}

fn update_branch(plugin_name: &str, base_commit: &str, head: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(plugin_name.as_bytes());
    hasher.update(base_commit.as_bytes());
    hasher.update(head.as_bytes());
    let digest = hasher.finalize();
    format!("plugin-rsi/update-{:0<12}", hex_prefix(&digest))
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
