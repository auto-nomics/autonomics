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
//! Pin discipline: `rev` must be a commit SHA. Tags and branches move;
//! images are digest-pinned, data bundles are digest-pinned, and plugin
//! code is rev-pinned — the same immutability rule one layer up.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use thiserror::Error;

use crate::loader::PLUGIN_ROOT_ENV;

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

fn sync_entry(entry: &PluginSource, root: &Path) -> Result<EntryOutcome> {
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
    let target = root.join(&entry.name);

    match (&entry.git, &entry.rev, &entry.path) {
        (Some(url), Some(rev), None) => sync_git(&entry.name, url, rev, &target),
        (Some(_), None, None) => Err(invalid("`git` sources require a pinned `rev`".into())),
        (Some(_), _, Some(_)) => Err(invalid("`git` and `path` are mutually exclusive".into())),
        (None, _, Some(source)) => sync_symlink(&entry.name, source, &target),
        (None, Some(_), None) => Err(invalid("`rev` requires a `git` source".into())),
        (None, None, None) => Err(invalid("one of `git` or `path` is required".into())),
    }
}

fn sync_git(name: &str, url: &str, rev: &str, target: &Path) -> Result<EntryOutcome> {
    let invalid = |message: String| SyncError::Invalid {
        name: name.to_string(),
        message,
    };
    if !rev.starts_with("sha256:") && !is_commit_sha(rev) {
        return Err(invalid(
            "`rev` must be a 40-hex commit SHA (tags and branches move)".into(),
        ));
    }

    // A target that exists but cannot resolve HEAD is an interrupted
    // clone (killed mid-checkout). Wipe it and re-clone rather than
    // failing forever on the broken state.
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

    // Already a checkout: is the pinned rev already what we have?
    let head = run_git_capture(name, target, &["rev-parse", "HEAD"])?;
    if head.trim() == rev {
        return Ok(EntryOutcome::Unchanged);
    }
    checkout_pinned(name, target, rev)?;
    Ok(EntryOutcome::Updated)
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
}
