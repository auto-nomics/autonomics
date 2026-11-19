//! Plugin installation from `plugins.toml`.
//!
//! The config lives beside the plugin root (default
//! `~/.autonomics/plugins.toml`, i.e. next to `vfs.toml` in the state
//! directory) and lists installation sources. At startup the runtime host
//! calls [`sync`], which materializes every entry under the plugin root:
//!
//! - `git = "<url>"` + `rev = "<sha>"` — clone/checkout the pinned commit
//! - `path = "<dir>"` — a symlink, so local development iterations are
//!   picked up on the next startup without reinstalling
//!
//! Sync is idempotent and offline-safe once installed: a satisfied pin is
//! a no-op, and a `path` entry never touches the network. Git access uses
//! the system `git` CLI (the podman precedent: drive the real tool
//! instead of embedding a client). After syncing, the loader scans the
//! root as usual.
//!
//! [`check`] is sync's offline counterpart: it verifies every declared
//! entry against what is already installed (no network, no mutation) so
//! the startup diagnostic can decide between starting the daemon and
//! asking for `autonomics prepare`. Both share the entry validation, so
//! a config one rejects the other rejects too.
//!
//! Pin discipline: `rev` must be a commit SHA. Tags and branches move;
//! images are digest-pinned, data bundles are digest-pinned, and plugin
//! code is rev-pinned — the same immutability rule one layer up.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use thiserror::Error;

use crate::{
    loader::PLUGIN_ROOT_ENV,
    manifest::{PluginManifest, PluginStatus},
};

pub const PLUGIN_CONFIG_FILE: &str = "plugins.toml";

pub type Result<T> = std::result::Result<T, SyncError>;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("cannot read plugin config `{}`: {source}", path.display())]
    ReadConfig {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot parse plugin config `{}`: {detail}", path.display())]
    ParseConfig { path: PathBuf, detail: String },
    #[error("plugin `{name}`: {message}")]
    Invalid { name: String, message: String },
    #[error("plugin `{name}`: `{command}` failed: {stderr}")]
    Git {
        name: String,
        command: String,
        stderr: String,
    },
}

/// One `[[plugin]]` entry in `plugins.toml`. Exactly one source kind per
/// entry; `name` is the install directory under the plugin root.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSource {
    pub name: String,
    /// Git repository URL (https or file://). Requires `rev`.
    #[serde(default)]
    pub git: Option<String>,
    /// Pinned commit SHA for `git` sources. Tags and branches are
    /// rejected: they are mutable.
    #[serde(default)]
    pub rev: Option<String>,
    /// Local directory, installed as a symlink. Mutually exclusive with
    /// `git`.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Commit in the plugin development workspace for daemon-owned local sources.
    #[serde(default)]
    pub local_commit: Option<String>,
    /// Tree digest of a daemon-owned local snapshot.
    #[serde(default)]
    pub local_digest: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PluginsConfig {
    #[serde(default)]
    pub plugin: Vec<PluginSource>,
}

/// What sync did for one entry.
#[derive(Debug, PartialEq, Eq)]
pub enum EntryOutcome {
    Installed,
    Updated,
    Unchanged,
}

/// Verified state of one declared plugin source, from [`check`].
#[derive(Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Installed and matching the pin (or owned by an in-place
    /// development update).
    Ok,
    /// Nothing installed under the plugin root yet.
    Missing,
    /// Installed but not matching the pin: stale, interrupted, or the
    /// pinned rev is not available offline.
    Inconsistent,
}

/// One `[[plugin]]` entry as [`check`] saw it.
#[derive(Debug)]
pub struct CheckedEntry {
    pub name: String,
    pub outcome: CheckOutcome,
    /// Human-readable reason for a non-`Ok` outcome (empty for `Ok`).
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct CheckReport {
    pub entries: Vec<CheckedEntry>,
}

impl CheckReport {
    /// Whether every declared entry is installed at its pin.
    pub fn is_ok(&self) -> bool {
        self.entries
            .iter()
            .all(|entry| entry.outcome == CheckOutcome::Ok)
    }
}

#[derive(Debug, Default)]
pub struct SyncReport {
    pub outcomes: Vec<(String, EntryOutcome)>,
}

impl SyncReport {
    pub fn summary(&self) -> String {
        let installed = self
            .outcomes
            .iter()
            .filter(|(_, o)| *o == EntryOutcome::Installed)
            .count();
        let updated = self
            .outcomes
            .iter()
            .filter(|(_, o)| *o == EntryOutcome::Updated)
            .count();
        format!(
            "{} plugin(s): {} installed, {} updated, {} unchanged",
            self.outcomes.len(),
            installed,
            updated,
            self.outcomes.len() - installed - updated
        )
    }
}

/// Read `config_path`, materialize every entry under `root`, and pin the
/// process plugin root so the subsequent loader scan (which may run in
/// another crate that only knows the env default) sees exactly this root.
/// A missing config file is a no-op: no declared plugins, nothing to do.
pub fn sync(config_path: &Path, root: &Path) -> Result<SyncReport> {
    let text = match std::fs::read_to_string(config_path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SyncReport::default());
        }
        Err(source) => {
            return Err(SyncError::ReadConfig {
                path: config_path.to_path_buf(),
                source,
            });
        }
    };
    let config: PluginsConfig = toml::from_str(&text).map_err(|error| SyncError::ParseConfig {
        path: config_path.to_path_buf(),
        detail: error.to_string(),
    })?;

    std::fs::create_dir_all(root).map_err(|source| SyncError::Invalid {
        name: "<root>".into(),
        message: format!("cannot create `{}`: {source}", root.display()),
    })?;

    let mut report = SyncReport::default();
    for entry in &config.plugin {
        let outcome = sync_entry(entry, root)?;
        report.outcomes.push((entry.name.clone(), outcome));
    }

    // Pin the process-wide root: the loader in data-engine resolves its
    // root lazily through the same env variable.
    // SAFETY: called once at startup, before any loader scan.
    unsafe { std::env::set_var(PLUGIN_ROOT_ENV, root) };

    Ok(report)
}

/// Offline counterpart of [`sync`]: verify every declared entry against
/// what is already installed under `root` — no network, no mutation, no
/// process-wide root pin. A missing config file is a no-op (empty
/// report), mirroring `sync`. The daemon's startup gate runs this and
/// refuses to start on any non-`Ok` entry; `prepare` runs [`sync`] to
/// converge the state `check` reports on.
pub fn check(config_path: &Path, root: &Path) -> Result<CheckReport> {
    let text = match std::fs::read_to_string(config_path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CheckReport::default());
        }
        Err(source) => {
            return Err(SyncError::ReadConfig {
                path: config_path.to_path_buf(),
                source,
            });
        }
    };
    let config: PluginsConfig = toml::from_str(&text).map_err(|error| SyncError::ParseConfig {
        path: config_path.to_path_buf(),
        detail: error.to_string(),
    })?;

    let mut report = CheckReport::default();
    for entry in &config.plugin {
        report.entries.push(check_entry(entry, root)?);
    }
    Ok(report)
}

fn check_entry(entry: &PluginSource, root: &Path) -> Result<CheckedEntry> {
    let target = root.join(&entry.name);
    let (outcome, detail) = match validate_entry(entry)? {
        ValidSource::Git { rev, .. } => check_git(&entry.name, rev, &target)?,
        ValidSource::Path { source } => check_symlink(source, &target),
    };
    Ok(CheckedEntry {
        name: entry.name.clone(),
        outcome,
        detail,
    })
}

/// Classify one git-sourced checkout without touching the network.
/// Spawn failures (git missing) are environment errors (`Err`); broken
/// checkouts are plugin gaps (`Inconsistent`).
fn check_git(name: &str, rev: &str, target: &Path) -> Result<(CheckOutcome, String)> {
    // A leftover local-install symlink is what `sync` would replace.
    if target
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Ok((
            CheckOutcome::Inconsistent,
            "stale local-install symlink left behind".into(),
        ));
    }
    if !target.join(".git").exists() {
        return Ok((CheckOutcome::Missing, "not installed".into()));
    }
    let Some(head) = git_capture_opt(name, target, &["rev-parse", "HEAD"])? else {
        return Ok((
            CheckOutcome::Inconsistent,
            "interrupted clone (no resolvable HEAD)".into(),
        ));
    };
    if development_in_progress(target)? {
        return Ok((CheckOutcome::Ok, "development in progress".into()));
    }
    let head = head.trim();
    if head == rev {
        return Ok((CheckOutcome::Ok, String::new()));
    }
    if git_succeeds(name, target, &["cat-file", "-e", rev])? {
        Ok((
            CheckOutcome::Inconsistent,
            format!("HEAD {head} differs from pinned {rev} — checkout pending"),
        ))
    } else {
        Ok((
            CheckOutcome::Inconsistent,
            "pinned rev not in the local clone (fetch required)".into(),
        ))
    }
}

/// Classify one path-sourced symlink installation.
fn check_symlink(source: &Path, target: &Path) -> (CheckOutcome, String) {
    match std::fs::symlink_metadata(target) {
        Err(_) => (CheckOutcome::Missing, "not installed".into()),
        Ok(meta) if !meta.file_type().is_symlink() => (
            CheckOutcome::Inconsistent,
            format!(
                "`{}` exists but is not a symlink to `{}`",
                target.display(),
                source.display()
            ),
        ),
        Ok(_) => match std::fs::read_link(target) {
            Ok(existing) if existing == source => {
                if source.is_dir() {
                    (CheckOutcome::Ok, String::new())
                } else {
                    (
                        CheckOutcome::Inconsistent,
                        format!("source directory `{}` is gone", source.display()),
                    )
                }
            }
            Ok(other) => (
                CheckOutcome::Inconsistent,
                format!(
                    "symlink points at `{}`, expected `{}`",
                    other.display(),
                    source.display()
                ),
            ),
            Err(_) => (CheckOutcome::Inconsistent, "dangling symlink".into()),
        },
    }
}

/// A `[[plugin]]` entry reduced to its validated source kind — the
/// shared validation core of [`sync`] and [`check`]: both reject exactly
/// the same configs.
enum ValidSource<'a> {
    Git { url: &'a str, rev: &'a str },
    Path { source: &'a Path },
}

fn validate_entry(entry: &PluginSource) -> Result<ValidSource<'_>> {
    let invalid = |message: String| SyncError::Invalid {
        name: entry.name.clone(),
        message,
    };

    if entry.name.is_empty()
        || !entry
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
    {
        return Err(invalid(
            "name must be non-empty lowercase `[a-z0-9_-]`".into(),
        ));
    }

    match (&entry.git, &entry.rev, &entry.path) {
        (Some(url), Some(rev), None) => {
            if !rev.starts_with("sha256:") && !is_commit_sha(rev) {
                return Err(invalid(
                    "`rev` must be a 40-hex commit SHA (tags and branches move)".into(),
                ));
            }
            Ok(ValidSource::Git { url, rev })
        }
        (Some(_), None, None) => Err(invalid("`git` sources require a pinned `rev`".into())),
        (Some(_), _, Some(_)) => Err(invalid("`git` and `path` are mutually exclusive".into())),
        (None, _, Some(source)) => Ok(ValidSource::Path { source }),
        (None, Some(_), None) => Err(invalid("`rev` requires a `git` source".into())),
        (None, None, None) => Err(invalid("one of `git` or `path` is required".into())),
    }
}

fn sync_entry(entry: &PluginSource, root: &Path) -> Result<EntryOutcome> {
    let target = root.join(&entry.name);
    match validate_entry(entry)? {
        ValidSource::Git { url, rev } => sync_git(&entry.name, url, rev, &target),
        ValidSource::Path { source } => sync_symlink(&entry.name, source, &target),
    }
}

fn sync_git(name: &str, url: &str, rev: &str, target: &Path) -> Result<EntryOutcome> {
    let invalid = |message: String| SyncError::Invalid {
        name: name.to_string(),
        message,
    };

    // A target that exists but cannot resolve HEAD is an interrupted
    // clone (killed mid-checkout). Wipe it and re-clone rather than
    // failing forever on the broken state.
    // A local installation may leave a symlink behind when the registry
    // is later switched back to a Git source.
    if target
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        std::fs::remove_file(target).map_err(|source| {
            invalid(format!(
                "cannot replace stale symlink `{}`: {source}",
                target.display()
            ))
        })?;
    }
    if target.join(".git").exists()
        && run_git_capture(name, target, &["rev-parse", "HEAD"]).is_err()
    {
        std::fs::remove_dir_all(target).map_err(|source| {
            invalid(format!(
                "cannot remove interrupted clone `{}`: {source}",
                target.display()
            ))
        })?;
    }

    if !target.join(".git").exists() {
        run_git_retry(
            name,
            target.parent().unwrap(),
            &["clone", "--no-checkout", url, &target.to_string_lossy()],
        )?;
        let outcome = EntryOutcome::Installed;
        checkout_pinned(name, target, rev)?;
        return Ok(outcome);
    }

    if development_in_progress(target)? {
        return Ok(EntryOutcome::Unchanged);
    }

    // Already a checkout: is the pinned rev already what we have?
    let head = run_git_capture(name, target, &["rev-parse", "HEAD"])?;
    if head.trim() == rev {
        return Ok(EntryOutcome::Unchanged);
    }
    checkout_pinned(name, target, rev)?;
    Ok(EntryOutcome::Updated)
}

/// Whether daemon-owned development owns this checkout at startup.
///
/// A plugin can be installed and updated in the same directory. While its
/// manifest is outside a runtime-ready state, sync must not hard-reset the
/// in-progress workspace back to the older revision in `plugins.toml`.
fn development_in_progress(target: &Path) -> Result<bool> {
    let path = target.join("manifest.toml");
    if !path.is_file() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(&path).map_err(|source| SyncError::Invalid {
        name: target
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<plugin>".into()),
        message: format!("cannot read `{}`: {source}", path.display()),
    })?;
    let manifest: PluginManifest = toml::from_str(&text).map_err(|error| SyncError::Invalid {
        name: target
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<plugin>".into()),
        message: format!("cannot parse `{}`: {error}", path.display()),
    })?;
    Ok(!matches!(
        manifest.status,
        PluginStatus::Published | PluginStatus::Installed
    ))
}

fn is_commit_sha(rev: &str) -> bool {
    rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Fetch (offline-tolerant when the rev is already local) and hard-checkout
/// the pinned rev: the installed tree must be exactly the pinned commit.
fn checkout_pinned(name: &str, target: &Path, rev: &str) -> Result<()> {
    let have_it = run_git_capture(name, target, &["cat-file", "-e", rev]).is_ok();
    if !have_it {
        run_git_retry(name, target, &["fetch", "--all", "--tags"])?;
    }
    run_git(name, target, &["checkout", "--detach", rev])?;
    run_git(name, target, &["reset", "--hard", rev])?;
    Ok(())
}

fn sync_symlink(name: &str, source: &Path, target: &Path) -> Result<EntryOutcome> {
    let invalid = |message: String| SyncError::Invalid {
        name: name.to_string(),
        message,
    };
    if !source.is_dir() {
        return Err(invalid(format!(
            "`path` `{}` is not a directory",
            source.display()
        )));
    }

    // An existing symlink pointing at the source is already installed.
    if let Ok(existing) = std::fs::read_link(target) {
        if existing == source {
            return Ok(EntryOutcome::Unchanged);
        }
    }

    match std::fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::remove_file(target).map_err(|source_err| {
                invalid(format!(
                    "cannot replace stale symlink `{}`: {source_err}",
                    target.display()
                ))
            })?;
        }
        Ok(_) => {
            return Err(invalid(format!(
                "`{}` already exists and is not a symlink; refusing to overwrite",
                target.display()
            )));
        }
        Err(_) => {}
    }

    symlink_dir(source, target).map_err(|source_err| {
        invalid(format!(
            "cannot symlink `{}` -> `{}`: {source_err}",
            target.display(),
            source.display()
        ))
    })?;
    Ok(EntryOutcome::Installed)
}

/// Run a git command, retrying once on transient network failures. SSH
/// connections to git hosts are routinely dropped mid-transfer by NATs and
/// proxies; a single retry converts those into successes without masking
/// real errors (auth, missing repo), which fail identically twice.
fn run_git_retry(name: &str, dir: &Path, args: &[&str]) -> Result<()> {
    run_git(name, dir, args).or_else(|first| match args.first() {
        Some(&"clone") | Some(&"fetch") | Some(&"ls-remote") => run_git(name, dir, args)
            .inspect_err(|_second| {
                let _ = first;
            }),
        _ => Err(first),
    })
}

fn run_git(name: &str, dir: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|source| SyncError::Git {
            name: name.to_string(),
            command: args.first().copied().unwrap_or("git").to_string(),
            stderr: format!("cannot spawn git: {source}"),
        })?;
    if !output.status.success() {
        return Err(SyncError::Git {
            name: name.to_string(),
            command: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(())
}

fn run_git_capture(name: &str, dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|source| SyncError::Git {
            name: name.to_string(),
            command: args.first().copied().unwrap_or("git").to_string(),
            stderr: format!("cannot spawn git: {source}"),
        })?;
    if !output.status.success() {
        return Err(SyncError::Git {
            name: name.to_string(),
            command: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run a git command, distinguishing a spawn failure (`Err` — git
/// missing is an environment error) from a non-zero exit (`Ok(None)` —
/// a broken or unresolvable state). The check path treats the latter as
/// a plugin gap, not a failure of the check itself.
fn git_capture_opt(name: &str, dir: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|source| SyncError::Git {
            name: name.to_string(),
            command: args.first().copied().unwrap_or("git").to_string(),
            stderr: format!("cannot spawn git: {source}"),
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// Whether a git command exits zero (spawn failure is `Err`).
fn git_succeeds(name: &str, dir: &Path, args: &[&str]) -> Result<bool> {
    Ok(git_capture_opt(name, dir, args)?.is_some())
}

#[cfg(unix)]
fn symlink_dir(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(not(unix))]
fn symlink_dir(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "path plugins require symlink support (unix)",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join(PLUGIN_CONFIG_FILE);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn make_plugin_repo(base: &Path, name: &str) -> (PathBuf, String) {
        let repo = base.join(format!("{name}-repo"));
        std::fs::create_dir_all(repo.join("scripts")).unwrap();
        std::fs::write(
            repo.join("manifest.toml"),
            format!(
                r#"
schema_version = 1
plugin_name = "{name}"

[image]
reference = "ghcr.io/auto-nomics/autonomics/{name}@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c"

[[nodes]]
kind = "{name}_node"
desc = "d"
doc = "doc"
timeout_secs = 60

[nodes.ports]
inputs = [{{ type = "file" }}]
outputs = [{{ path = "out.log" }}]

[nodes.command]
interpreter = "sh"
script = "true"
"#
            ),
        )
        .unwrap();
        run_git(name, &repo, &["init", "--initial-branch=main"]).unwrap();
        run_git(name, &repo, &["add", "."]).unwrap();
        run_git(
            name,
            &repo,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "init",
            ],
        )
        .unwrap();
        let rev = run_git_capture(name, &repo, &["rev-parse", "HEAD"]).unwrap();
        (repo, rev.trim().to_string())
    }

    #[test]
    fn missing_config_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let report = sync(
            &dir.path().join("plugins.toml"),
            &dir.path().join("plugins"),
        )
        .unwrap();
        assert!(report.outcomes.is_empty());
    }

    #[test]
    fn local_path_sources_install_as_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("my-plugin");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("manifest.toml"), "# placeholder").unwrap();

        let config = write_config(
            dir.path(),
            &r#"
[[plugin]]
name = "my-plugin"
path = "SOURCE_PATH"
"#
            .replace("SOURCE_PATH", &source.to_string_lossy()),
        );
        let root = dir.path().join("plugins");
        let report = sync(&config, &root).unwrap();
        assert_eq!(report.outcomes[0].1, EntryOutcome::Installed);
        assert!(root.join("my-plugin").is_dir());
        // Re-sync is unchanged.
        let report = sync(&config, &root).unwrap();
        assert_eq!(report.outcomes[0].1, EntryOutcome::Unchanged);
        assert!(report.summary().contains("1 plugin(s)"));
    }

    #[test]
    fn git_sync_replaces_a_stale_local_install_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, rev) = make_plugin_repo(dir.path(), "gitplug");
        let local = dir.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("manifest.toml"), "# placeholder").unwrap();

        let root = dir.path().join("plugins");
        std::fs::create_dir_all(&root).unwrap();
        symlink_dir(&local, &root.join("gitplug")).unwrap();

        let config = write_config(
            dir.path(),
            &format!(
                r#"
[[plugin]]
name = "gitplug"
git = "{url}"
rev = "{rev}"
"#,
                url = repo.to_string_lossy(),
                rev = rev
            ),
        );
        let report = sync(&config, &root).unwrap();

        assert_eq!(report.outcomes[0].1, EntryOutcome::Installed);
        assert!(root.join("gitplug/manifest.toml").is_file());
        assert!(root.join("gitplug/.git").is_dir());
    }

    #[test]
    fn git_sources_install_at_the_pinned_rev() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, rev) = make_plugin_repo(dir.path(), "gitplug");
        let config = write_config(
            dir.path(),
            &format!(
                r#"
[[plugin]]
name = "gitplug"
git = "{url}"
rev = "{rev}"
"#,
                url = repo.to_string_lossy(),
                rev = rev
            ),
        );
        let root = dir.path().join("plugins");
        let report = sync(&config, &root).unwrap();
        assert_eq!(report.outcomes[0].1, EntryOutcome::Installed);
        assert!(root.join("gitplug/manifest.toml").is_file());
        // Re-sync at the same rev is a no-op (offline-safe path).
        let report = sync(&config, &root).unwrap();
        assert_eq!(report.outcomes[0].1, EntryOutcome::Unchanged);
    }

    #[test]
    fn sync_does_not_reset_a_plugin_update_in_progress() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, rev) = make_plugin_repo(dir.path(), "updating");
        let config = write_config(
            dir.path(),
            &format!(
                r#"
[[plugin]]
name = "updating"
git = "{url}"
rev = "{rev}"
"#,
                url = repo.to_string_lossy(),
                rev = rev
            ),
        );
        let root = dir.path().join("plugins");
        sync(&config, &root).unwrap();

        let workspace = root.join("updating");
        let manifest_path = workspace.join("manifest.toml");
        let manifest = std::fs::read_to_string(&manifest_path).unwrap();
        std::fs::write(
            &manifest_path,
            manifest.replacen(
                "schema_version = 1",
                "schema_version = 1\nstatus = \"updating\"",
                1,
            ),
        )
        .unwrap();
        std::fs::write(workspace.join("in-progress.txt"), "preserve me").unwrap();

        let report = sync(&config, &root).unwrap();
        assert_eq!(report.outcomes[0].1, EntryOutcome::Unchanged);
        assert_eq!(
            std::fs::read_to_string(workspace.join("in-progress.txt")).unwrap(),
            "preserve me"
        );
    }

    #[test]
    fn mutable_refs_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let config = write_config(
            dir.path(),
            r#"
[[plugin]]
name = "bad"
git = "https://example.com/x.git"
rev = "v1.0"
"#,
        );
        let error = sync(&config, &dir.path().join("plugins")).unwrap_err();
        assert!(error.to_string().contains("40-hex"), "{error}");
    }

    #[test]
    fn foreign_directories_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("plugins");
        std::fs::create_dir_all(root.join("occupied")).unwrap();
        std::fs::write(root.join("occupied/stub.txt"), "x").unwrap();
        let source = dir.path().join("other");
        std::fs::create_dir_all(&source).unwrap();

        let config = write_config(
            dir.path(),
            &format!(
                r#"
[[plugin]]
name = "occupied"
path = "{}"
"#,
                source.to_string_lossy()
            ),
        );
        let error = sync(&config, &root).unwrap_err();
        assert!(
            error.to_string().contains("refusing to overwrite"),
            "{error}"
        );
        // The foreign content is intact.
        assert!(root.join("occupied/stub.txt").is_file());
    }

    #[test]
    fn mutually_exclusive_sources_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let config = write_config(
            dir.path(),
            r#"
[[plugin]]
name = "both"
git = "https://example.com/x.git"
rev = "0123456789abcdef0123456789abcdef01234567"
path = "/tmp"
"#,
        );
        let error = sync(&config, &dir.path().join("plugins")).unwrap_err();
        assert!(error.to_string().contains("mutually exclusive"), "{error}");
    }

    // ── check: the offline counterpart ─────────────────────────────

    fn git_config(dir: &Path, name: &str, repo: &Path, rev: &str) -> PathBuf {
        write_config(
            dir,
            &format!(
                r#"
[[plugin]]
name = "{name}"
git = "{url}"
rev = "{rev}"
"#,
                url = repo.to_string_lossy(),
                rev = rev
            ),
        )
    }

    #[test]
    fn check_missing_config_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let report = check(
            &dir.path().join("plugins.toml"),
            &dir.path().join("plugins"),
        )
        .unwrap();
        assert!(report.entries.is_empty());
        assert!(report.is_ok());
    }

    #[test]
    fn check_reports_missing_before_install_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, rev) = make_plugin_repo(dir.path(), "gitplug");
        let source = dir.path().join("my-plugin");
        std::fs::create_dir_all(&source).unwrap();

        let config = write_config(
            dir.path(),
            &format!(
                r#"
[[plugin]]
name = "gitplug"
git = "{url}"
rev = "{rev}"

[[plugin]]
name = "my-plugin"
path = "{path}"
"#,
                url = repo.to_string_lossy(),
                rev = rev,
                path = source.to_string_lossy()
            ),
        );
        let root = dir.path().join("plugins");
        let report = check(&config, &root).unwrap();

        assert_eq!(report.entries.len(), 2);
        assert_eq!(report.entries[0].outcome, CheckOutcome::Missing);
        assert_eq!(report.entries[1].outcome, CheckOutcome::Missing);
        assert!(!report.is_ok());
        // The check is read-only: it never materializes the root.
        assert!(!root.exists());
    }

    #[test]
    fn check_ok_after_sync() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, rev) = make_plugin_repo(dir.path(), "gitplug");
        let source = dir.path().join("my-plugin");
        std::fs::create_dir_all(&source).unwrap();

        let config = write_config(
            dir.path(),
            &format!(
                r#"
[[plugin]]
name = "gitplug"
git = "{url}"
rev = "{rev}"

[[plugin]]
name = "my-plugin"
path = "{path}"
"#,
                url = repo.to_string_lossy(),
                rev = rev,
                path = source.to_string_lossy()
            ),
        );
        let root = dir.path().join("plugins");
        sync(&config, &root).unwrap();

        let report = check(&config, &root).unwrap();
        assert!(report.is_ok(), "{report:?}");
        assert!(report.entries.iter().all(|e| e.detail.is_empty()));
    }

    #[test]
    fn check_inconsistent_when_head_differs_but_rev_is_local() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, rev_a) = make_plugin_repo(dir.path(), "moving");
        // A second commit in the source repo: two revs to pin against.
        std::fs::write(repo.join("second.txt"), "2").unwrap();
        run_git("moving", &repo, &["add", "."]).unwrap();
        run_git(
            "moving",
            &repo,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "second",
            ],
        )
        .unwrap();
        let rev_b = run_git_capture("moving", &repo, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();

        let root = dir.path().join("plugins");
        // Install at the newer rev; the clone carries both revs.
        sync(&git_config(dir.path(), "moving", &repo, &rev_b), &root).unwrap();

        // Pin back to the older rev: local rev, but HEAD differs — an
        // offline checkout is pending, no fetch would be needed.
        let report = check(&git_config(dir.path(), "moving", &repo, &rev_a), &root).unwrap();
        assert_eq!(report.entries[0].outcome, CheckOutcome::Inconsistent);
        assert!(
            report.entries[0].detail.contains("checkout pending"),
            "{:?}",
            report.entries[0].detail
        );
    }

    #[test]
    fn check_inconsistent_when_rev_is_not_local() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, rev) = make_plugin_repo(dir.path(), "gitplug");
        let root = dir.path().join("plugins");
        sync(&git_config(dir.path(), "gitplug", &repo, &rev), &root).unwrap();

        // A syntactically valid SHA that never existed in the clone.
        let alien = "0123456789abcdef0123456789abcdef01234567".to_owned();
        let report = check(&git_config(dir.path(), "gitplug", &repo, &alien), &root).unwrap();
        assert_eq!(report.entries[0].outcome, CheckOutcome::Inconsistent);
        assert!(
            report.entries[0].detail.contains("fetch required"),
            "{:?}",
            report.entries[0].detail
        );
    }

    #[test]
    fn check_inconsistent_on_interrupted_clone() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, rev) = make_plugin_repo(dir.path(), "gitplug");
        let root = dir.path().join("plugins");
        // A `.git` stub that no git command can resolve: the state a
        // killed clone leaves behind.
        std::fs::create_dir_all(root.join("gitplug/.git")).unwrap();

        let report = check(&git_config(dir.path(), "gitplug", &repo, &rev), &root).unwrap();
        assert_eq!(report.entries[0].outcome, CheckOutcome::Inconsistent);
        assert!(
            report.entries[0].detail.contains("interrupted clone"),
            "{:?}",
            report.entries[0].detail
        );
    }

    #[test]
    fn check_inconsistent_on_stale_local_install_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, rev) = make_plugin_repo(dir.path(), "gitplug");
        let local = dir.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        let root = dir.path().join("plugins");
        std::fs::create_dir_all(&root).unwrap();
        symlink_dir(&local, &root.join("gitplug")).unwrap();

        let report = check(&git_config(dir.path(), "gitplug", &repo, &rev), &root).unwrap();
        assert_eq!(report.entries[0].outcome, CheckOutcome::Inconsistent);
        assert!(
            report.entries[0]
                .detail
                .contains("stale local-install symlink"),
            "{:?}",
            report.entries[0].detail
        );
    }

    #[test]
    fn check_symlink_gaps() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("my-plugin");
        std::fs::create_dir_all(&source).unwrap();
        let root = dir.path().join("plugins");

        // Foreign directory where the symlink belongs.
        std::fs::create_dir_all(root.join("my-plugin")).unwrap();
        let config = write_config(
            dir.path(),
            &format!(
                r#"
[[plugin]]
name = "my-plugin"
path = "{}"
"#,
                source.to_string_lossy()
            ),
        );
        let report = check(&config, &root).unwrap();
        assert_eq!(report.entries[0].outcome, CheckOutcome::Inconsistent);
        assert!(
            report.entries[0].detail.contains("not a symlink"),
            "{:?}",
            report.entries[0].detail
        );

        // Installed, then the source directory disappears.
        std::fs::remove_dir_all(root.join("my-plugin")).unwrap();
        sync(&config, &root).unwrap();
        std::fs::remove_dir_all(&source).unwrap();
        let report = check(&config, &root).unwrap();
        assert_eq!(report.entries[0].outcome, CheckOutcome::Inconsistent);
        assert!(
            report.entries[0].detail.contains("is gone"),
            "{:?}",
            report.entries[0].detail
        );
    }

    #[test]
    fn check_rejects_invalid_configs_like_sync() {
        let dir = tempfile::tempdir().unwrap();
        let config = write_config(
            dir.path(),
            r#"
[[plugin]]
name = "bad"
git = "https://example.com/x.git"
rev = "v1.0"
"#,
        );
        let error = check(&config, &dir.path().join("plugins")).unwrap_err();
        assert!(error.to_string().contains("40-hex"), "{error}");
    }
}
