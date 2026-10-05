use std::{
    path::Path,
    process::{Command, Output},
};

use serde::{Deserialize, Serialize};

use crate::{Error, Result, gitrepo::GitRepo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GhVisibility {
    Private,
    Public,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhPublisherConfig {
    pub enabled: bool,
    pub owner: String,
    pub visibility: GhVisibility,
    pub repository_suffix: String,
    pub default_branch: String,
    pub allow_repo_create: bool,
}

impl Default for GhPublisherConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            owner: "auto-nomics".into(),
            visibility: GhVisibility::Private,
            repository_suffix: "-plugin".into(),
            default_branch: "main".into(),
            allow_repo_create: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhStatus {
    pub enabled: bool,
    pub authenticated: bool,
    pub account: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishOutcome {
    pub remote: String,
    pub commit: String,
    pub repository_created: bool,
}

/// Immutable result of opening one plugin update PR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestOutcome {
    /// Remote containing the pushed update branch.
    pub remote: String,
    /// Reviewed commit pushed to the update branch.
    pub commit: String,
    /// Branch created for this plugin update.
    pub branch: String,
    /// GitHub PR number.
    pub number: u64,
    /// Human-reviewable GitHub PR URL.
    pub url: String,
}

/// Immutable result of merging one plugin update PR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeOutcome {
    /// Remote whose default branch received the merge.
    pub remote: String,
    /// Immutable commit produced by the merge.
    pub commit: String,
}

/// Trusted publisher abstraction used by the plugin lifecycle.
///
/// Production uses [`GhPublisher`]; hosts and tests may inject another
/// implementation without granting agents raw GitHub access.
pub trait PluginPublisher {
    /// Publish one reviewed plugin repository and report its immutable source.
    fn publish_plugin(&self, plugin_name: &str, repo: &GitRepo) -> Result<PublishOutcome>;
}

/// Trusted PR abstraction used by plugin updates.
///
/// As with publication, agents never see this interface; only the daemon can
/// push review branches and call the GitHub CLI.
pub trait PluginPullRequestPublisher {
    /// Push one reviewed update branch and open a PR against its base.
    fn open_pull_request(
        &self,
        plugin_name: &str,
        branch: &str,
        repo: &GitRepo,
    ) -> Result<PullRequestOutcome>;

    /// Merge one previously opened PR and return its immutable merge commit.
    fn merge_pull_request(&self, remote: &str, number: u64) -> Result<MergeOutcome>;
}

impl PluginPublisher for GhPublisher {
    fn publish_plugin(&self, plugin_name: &str, repo: &GitRepo) -> Result<PublishOutcome> {
        self.publish(plugin_name, repo)
    }
}

impl PluginPullRequestPublisher for GhPublisher {
    fn open_pull_request(
        &self,
        plugin_name: &str,
        branch: &str,
        repo: &GitRepo,
    ) -> Result<PullRequestOutcome> {
        self.open_update_pull_request(plugin_name, branch, repo)
    }

    fn merge_pull_request(&self, remote: &str, number: u64) -> Result<MergeOutcome> {
        self.merge_update_pull_request(remote, number)
    }
}

#[derive(Debug, Clone)]
pub struct GhPublisher {
    config: GhPublisherConfig,
}

impl GhPublisher {
    pub fn new(config: GhPublisherConfig) -> Self {
        Self { config }
    }

    pub fn status(&self) -> GhStatus {
        if !self.config.enabled {
            return GhStatus {
                enabled: false,
                authenticated: false,
                account: None,
                detail: Some("GitHub publishing is disabled".into()),
            };
        }
        match self.run_gh_capture(&["auth", "status"]) {
            Ok(output) => GhStatus {
                enabled: true,
                authenticated: true,
                account: account_from_status(&output),
                detail: None,
            },
            Err(error) => GhStatus {
                enabled: true,
                authenticated: false,
                account: None,
                detail: Some(error.to_string()),
            },
        }
    }

    pub fn repository_name(&self, plugin_name: &str) -> Result<String> {
        crate::validate_plugin_name(plugin_name)?;
        let suffix = &self.config.repository_suffix;
        if suffix.is_empty()
            || suffix.contains('/')
            || !suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(Error::GitHub("invalid repository suffix".into()));
        }
        Ok(format!("{plugin_name}{suffix}"))
    }

    pub fn remote_url(&self, plugin_name: &str) -> Result<String> {
        let repository = self.repository_name(plugin_name)?;
        Ok(format!(
            "git@github.com:{}/{}.git",
            self.config.owner, repository
        ))
    }

    pub fn repository_exists(&self, plugin_name: &str) -> Result<bool> {
        let repository = self.repository_name(plugin_name)?;
        let slug = format!("{}/{}", self.config.owner, repository);
        self.run_gh(&["repo", "view", &slug])
            .map(|_| true)
            .or_else(|error| {
                let message = error.to_string();
                if message.contains("could not resolve")
                    || message.contains("Not Found")
                    || message.contains("name not known")
                {
                    Ok(false)
                } else {
                    Err(error)
                }
            })
    }

    pub fn create_repository(&self, plugin_name: &str) -> Result<()> {
        if !self.config.allow_repo_create {
            return Err(Error::GitHub("repository creation is disabled".into()));
        }
        let repository = self.repository_name(plugin_name)?;
        let slug = format!("{}/{}", self.config.owner, repository);
        let visibility = match self.config.visibility {
            GhVisibility::Private => "--private",
            GhVisibility::Public => "--public",
        };
        self.run_gh(&["repo", "create", &slug, visibility])
    }

    pub fn publish(&self, plugin_name: &str, repo: &GitRepo) -> Result<PublishOutcome> {
        if !self.status().authenticated {
            return Err(Error::GitHub("gh is not authenticated".into()));
        }
        crate::validate_plugin_name(plugin_name)?;
        if !repo.is_clean()? {
            return Err(Error::GitHub(
                "repository has uncommitted content; only reviewed commits can publish".into(),
            ));
        }
        let commit = repo.head()?;
        let remote_url = self.remote_url(plugin_name)?;
        match repo.remote_url("origin")? {
            Some(existing) if existing == remote_url => {}
            Some(existing) => {
                return Err(Error::GitHub(format!(
                    "origin points to {existing}, expected {remote_url}"
                )));
            }
            None => repo.add_remote("origin", &remote_url)?,
        }

        let exists = self.repository_exists(plugin_name)?;
        if !exists {
            self.create_repository(plugin_name)?;
        }
        repo.push("origin", &self.config.default_branch)?;
        let pushed = repo.head()?;
        if pushed != commit {
            return Err(Error::GitHub(
                "local HEAD moved while publishing; release is not immutable".into(),
            ));
        }
        Ok(PublishOutcome {
            remote: remote_url,
            commit: pushed,
            repository_created: !exists,
        })
    }

    /// Push an update branch and open a GitHub PR.
    pub fn open_update_pull_request(
        &self,
        plugin_name: &str,
        branch: &str,
        repo: &GitRepo,
    ) -> Result<PullRequestOutcome> {
        if !self.status().authenticated {
            return Err(Error::GitHub("gh is not authenticated".into()));
        }
        crate::validate_plugin_name(plugin_name)?;
        validate_branch_name(branch)?;
        if !repo.is_clean()? {
            return Err(Error::GitHub(
                "repository has uncommitted content; only reviewed commits can publish".into(),
            ));
        }
        let commit = repo.head()?;
        let remote = repo
            .remote_url("origin")?
            .ok_or_else(|| Error::GitHub("update workspace has no origin remote".into()))?;
        let slug = github_slug(&remote)?;
        repo.switch_new_branch(branch)?;
        repo.push("origin", branch)?;
        let created = self.run_gh_capture(&[
            "pr",
            "create",
            "--repo",
            &slug,
            "--head",
            branch,
            "--base",
            &self.config.default_branch,
            "--title",
            &format!("plugin-rsi: optimize {plugin_name}"),
            "--body",
            "Automated plugin update reviewed by the RSI lifecycle.",
        ])?;
        let url = created
            .trim()
            .lines()
            .last()
            .unwrap_or_default()
            .to_string();
        let number = parse_pr_number(&url)?;
        if repo.head()? != commit {
            return Err(Error::GitHub(
                "local HEAD moved while opening the PR; update is not immutable".into(),
            ));
        }
        Ok(PullRequestOutcome {
            remote,
            commit,
            branch: branch.to_string(),
            number,
            url,
        })
    }

    /// Merge one update PR and return its merge commit.
    pub fn merge_update_pull_request(&self, remote: &str, number: u64) -> Result<MergeOutcome> {
        if !self.status().authenticated {
            return Err(Error::GitHub("gh is not authenticated".into()));
        }
        let slug = github_slug(remote)?;
        let number = number.to_string();
        self.run_gh(&[
            "pr",
            "merge",
            &number,
            "--repo",
            &slug,
            "--squash",
            "--delete-branch",
        ])?;
        let view = self.run_gh_capture(&[
            "pr",
            "view",
            &number,
            "--repo",
            &slug,
            "--json",
            "mergeCommit",
        ])?;
        let parsed: MergeView = serde_json::from_str(&view)
            .map_err(|source| Error::GitHub(format!("cannot parse gh pr view output: {source}")))?;
        let commit = parsed.merge_commit.oid;
        if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::GitHub("gh returned an invalid merge commit".into()));
        }
        Ok(MergeOutcome {
            remote: remote.to_string(),
            commit,
        })
    }

    fn run_gh(&self, args: &[&str]) -> Result<()> {
        let output = self.spawn(args)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(command_error(args, &output.stderr))
        }
    }

    fn run_gh_capture(&self, args: &[&str]) -> Result<String> {
        let output = self.spawn(args)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(command_error(args, &output.stderr))
        }
    }

    fn spawn(&self, args: &[&str]) -> Result<Output> {
        Command::new("gh")
            .args(args)
            .output()
            .map_err(|source| Error::GitHub(format!("cannot run gh: {source}")))
    }
}

#[derive(Debug, Deserialize)]
struct MergeView {
    merge_commit: MergeCommit,
}

#[derive(Debug, Deserialize)]
struct MergeCommit {
    oid: String,
}

fn validate_branch_name(branch: &str) -> Result<()> {
    let valid = branch == "plugin-rsi-latest"
        || branch.strip_prefix("plugin-rsi/").is_some_and(|suffix| {
            !suffix.is_empty()
                && !suffix.contains('/')
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(Error::GitHub(
            "PR branch must be `plugin-rsi/<alphanumeric-or-dash>`".into(),
        ))
    }
}

fn github_slug(remote: &str) -> Result<String> {
    let without_git = remote.trim_end_matches(".git");
    let path = remote
        .strip_prefix("git@github.com:")
        .or_else(|| without_git.strip_prefix("https://github.com/"))
        .map(str::to_string);
    let Some(path) = path else {
        return Err(Error::GitHub(format!("remote `{remote}` is not on GitHub")));
    };
    if path.split('/').count() != 2 {
        return Err(Error::GitHub(format!("invalid GitHub remote `{remote}`")));
    }
    Ok(path)
}

fn parse_pr_number(url: &str) -> Result<u64> {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| Error::GitHub("gh did not return a PR URL".into()))
}

fn account_from_status(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.split_once("account "))?
        .1
        .split_whitespace()
        .next()
        .map(str::to_string)
}

fn command_error(args: &[&str], stderr: &[u8]) -> Error {
    Error::GitHub(format!(
        "gh {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(stderr).trim()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_stable_repository_identity() {
        let publisher = GhPublisher::new(GhPublisherConfig::default());
        assert_eq!(
            publisher.repository_name("clusterprofiler-bitr").unwrap(),
            "clusterprofiler-bitr-plugin"
        );
        assert_eq!(
            publisher.remote_url("clusterprofiler-bitr").unwrap(),
            "git@github.com:auto-nomics/clusterprofiler-bitr-plugin.git"
        );
    }
}
