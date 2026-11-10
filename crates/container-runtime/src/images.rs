//! Local image-store inventory and pre-pull for digest-pinned images.
//!
//! [`local_images`] reads the whole store in one `podman images` call
//! (one process instead of N per-image inspects); [`pull`] pre-fetches
//! one digest-pinned reference. Overlapping *pull* processes is safe —
//! the store serialises mutations through its own file locks — while
//! container *creation* keeps the `CREATE_GATE` precedent in
//! `podman.rs`; layer parallelism inside a single `podman pull` still
//! applies.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncBufReadExt;

use crate::connection::truncate_captured_bytes;
use crate::error::ContainerRuntimeError;
use crate::podman::{PodmanConfig, async_podman_command};

/// Budget for image pulls — the explicit ones here and the implicit
/// pull inside `podman create`. Tunable through
/// `AUTONOMICS_PODMAN_PULL_TIMEOUT_SECS`.
pub const DEFAULT_PULL_TIMEOUT_SECS: u64 = 3600;
const PULL_TIMEOUT_ENV: &str = "AUTONOMICS_PODMAN_PULL_TIMEOUT_SECS";
/// Budget for the single `podman images` inventory call.
const LIST_TIMEOUT_SECS: u64 = 60;

/// The pull budget in seconds: the `AUTONOMICS_PODMAN_PULL_TIMEOUT_SECS`
/// override when set to a positive number, else the default.
pub fn pull_timeout_secs() -> u64 {
    let value = std::env::var(PULL_TIMEOUT_ENV).ok();
    pull_timeout_from(value.as_deref())
}

fn pull_timeout_from(value: Option<&str>) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_PULL_TIMEOUT_SECS)
}

/// The images held by the local store, indexed for lookup by the
/// fully-qualified digest references the codebase declares.
///
/// Podman does not report a pulled image the same way across versions:
/// podman 3.x lists `RepoDigests` as bare `sha256:…` digests and keeps
/// the repository in `Names` only, while podman 4+ lists fully
/// qualified `repo@sha256:…` strings in `RepoDigests`. Comparing a
/// declared reference against a bare digest string never matches, so
/// every image would look permanently missing and `prepare` would
/// re-pull the whole catalogue on every run. The inventory therefore
/// normalises each row into canonical `repo@sha256:…` entries.
#[derive(Debug, Default, Clone)]
pub struct LocalImages {
    /// Canonical `repo@sha256:…` references present in the store.
    references: BTreeSet<String>,
    /// Digest → the repositories that carry it.
    by_digest: BTreeMap<String, BTreeSet<String>>,
    /// Digests whose repository is unknown (no usable name reported).
    orphan_digests: BTreeSet<String>,
}

impl LocalImages {
    /// Whether the store already holds the given digest-pinned
    /// reference (`repo@sha256:…`).
    pub fn contains(&self, reference: &str) -> bool {
        if self.references.contains(reference) {
            return true;
        }
        let Some((repo, digest)) = reference.rsplit_once('@') else {
            return false;
        };
        if !digest.starts_with("sha256:") {
            return false;
        }
        match self.by_digest.get(digest) {
            Some(repos) => repos.contains(repo),
            None => self.orphan_digests.contains(digest),
        }
    }

    /// The canonical references the store holds.
    pub fn references(&self) -> impl Iterator<Item = &str> {
        self.references.iter().map(String::as_str)
    }

    /// Whether the store reported no images at all.
    pub fn is_empty(&self) -> bool {
        self.references.is_empty() && self.orphan_digests.is_empty()
    }
}

/// All images present in local storage, read from one `podman images
/// --format json` call.
pub async fn local_images(program: &str) -> Result<LocalImages, ContainerRuntimeError> {
    local_images_command(async_podman_command(program), program).await
}

/// Inventory the same explicit private backend used for actual DAG containers.
pub async fn local_images_with_config(
    config: &PodmanConfig,
) -> Result<LocalImages, ContainerRuntimeError> {
    local_images_command(config.async_command(), &config.program).await
}

async fn local_images_command(
    mut command: tokio::process::Command,
    program: &str,
) -> Result<LocalImages, ContainerRuntimeError> {
    let listed = tokio::time::timeout(
        Duration::from_secs(LIST_TIMEOUT_SECS),
        command
            .args(["images", "--format", "json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await;
    let output = match listed {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return Err(ContainerRuntimeError::Invalid(format!(
                "cannot spawn `{program} images`: {error}"
            )));
        }
        Err(_) => {
            return Err(ContainerRuntimeError::Timeout {
                timeout_secs: LIST_TIMEOUT_SECS,
            });
        }
    };
    if !output.status.success() {
        return Err(ContainerRuntimeError::Invalid(format!(
            "`{program} images` failed with status {}: {}",
            output.status.code().unwrap_or(1),
            truncate_captured_bytes(&output.stderr)
        )));
    }
    Ok(parse_local_images(&output.stdout))
}

/// One row of `podman images --format json`. Podman 3.x and podman 4+
/// populate these fields differently, so all of them are read and
/// reconciled by [`parse_local_images`].
#[derive(Deserialize)]
struct ImageRow {
    #[serde(rename = "RepoDigests", default)]
    repo_digests: Vec<String>,
    #[serde(rename = "Names", default)]
    names: Vec<String>,
    #[serde(rename = "Digest", default)]
    digest: Option<String>,
}

impl ImageRow {
    /// The repositories this image is known under, derived from its
    /// names: `repo@sha256:…` and `repo:tag` both yield `repo`.
    fn repositories(&self) -> BTreeSet<String> {
        self.names
            .iter()
            .filter_map(|name| {
                let repo = match name.split_once('@') {
                    Some((repo, _)) => repo,
                    None => {
                        // Strip a tag, but never a registry port.
                        let slash = name.rfind('/').map(|index| index + 1).unwrap_or(0);
                        let colon = name[slash..].find(':')? + slash;
                        &name[..colon]
                    }
                };
                (!repo.is_empty()).then(|| repo.to_string())
            })
            .collect()
    }

    /// The bare `sha256:…` digests this image carries.
    fn digests(&self) -> BTreeSet<String> {
        let mut digests: BTreeSet<String> = self
            .repo_digests
            .iter()
            .filter(|entry| entry.starts_with("sha256:"))
            .cloned()
            .collect();
        if let Some(digest) = &self.digest {
            if digest.starts_with("sha256:") {
                digests.insert(digest.clone());
            }
        }
        digests
    }

    /// The fully-qualified references podman reports directly.
    fn qualified_references(&self) -> impl Iterator<Item = &str> {
        self.repo_digests
            .iter()
            .filter(|entry| entry.contains('@'))
            .map(String::as_str)
            .chain(
                self.names
                    .iter()
                    .filter(|name| name.contains('@'))
                    .map(String::as_str),
            )
    }
}

/// Parse the `podman images --format json` payload into the store's
/// canonical reference index. A payload that does not parse as the
/// expected array yields an empty inventory — the caller reports every
/// declared image as missing rather than failing the whole inventory.
fn parse_local_images(payload: &[u8]) -> LocalImages {
    let rows: Vec<ImageRow> = serde_json::from_slice(payload).unwrap_or_default();
    let mut images = LocalImages::default();
    for row in rows {
        let repositories = row.repositories();
        let digests = row.digests();

        for reference in row.qualified_references() {
            images.references.insert(reference.to_string());
            if let Some((repo, digest)) = reference.rsplit_once('@') {
                images
                    .by_digest
                    .entry(digest.to_string())
                    .or_default()
                    .insert(repo.to_string());
            }
        }

        for digest in digests {
            if repositories.is_empty() {
                // No usable repository: remember the digest on its own.
                images.orphan_digests.insert(digest);
                continue;
            }
            let repos = images.by_digest.entry(digest.clone()).or_default();
            for repository in &repositories {
                repos.insert(repository.clone());
                images.references.insert(format!("{repository}@{digest}"));
            }
        }
    }
    images
}

/// Pull one digest-pinned reference into local storage. Sequential by
/// contract (see the module doc). On timeout the child is killed
/// (`kill_on_drop`) so an abandoned pull cannot keep writing layers.
pub async fn pull(program: &str, reference: &str) -> Result<(), ContainerRuntimeError> {
    pull_command(async_podman_command(program), program, reference).await
}

/// Pre-pull into the exact backend that will create the daemon's containers.
pub async fn pull_with_config(
    config: &PodmanConfig,
    reference: &str,
) -> Result<(), ContainerRuntimeError> {
    let mut attempt = 1;
    loop {
        let staging = pull_staging_dir(config);
        let outcome = pull_command(
            config.async_command_with_staging(staging.as_deref()),
            &config.program,
            reference,
        )
        .await;
        remove_staging_dir(&staging);
        match outcome {
            Ok(()) => return Ok(()),
            Err(error) => {
                if !should_retry_pull(&error, attempt, PULL_ATTEMPTS) {
                    return Err(error);
                }
                attempt += 1;
                tokio::time::sleep(retry_backoff(attempt)).await;
            }
        }
    }
}

/// Rolling stderr tail kept for pull failure messages.
const PULL_TAIL_LINES: usize = 8;

/// Attempts per image pull before the reference is reported as failed.
///
/// A truncated layer — the registry or an HTTP proxy closing the body
/// before its `Content-Length` is satisfied — aborts the pull with
/// "unexpected EOF" while Podman stores the blob. Podman 3.x does not
/// retry it, and the truncation is transient: the same reference pulls
/// cleanly on a retry.
const PULL_ATTEMPTS: usize = 3;

/// Seconds to wait before the next pull attempt; the wait grows with the
/// attempt number so a struggling link is not hammered immediately.
const PULL_RETRY_BACKOFF_SECS: u64 = 5;

/// Whether a failed pull attempt should be repeated. A timeout is not
/// retried: the download was making (if slow) progress, and restarting
/// it would only throw the transferred layers away.
fn should_retry_pull(error: &ContainerRuntimeError, attempt: usize, attempts: usize) -> bool {
    attempt < attempts && !matches!(error, ContainerRuntimeError::Timeout { .. })
}

/// The wait before attempt number `attempt` (1-based).
fn retry_backoff(attempt: usize) -> Duration {
    Duration::from_secs(PULL_RETRY_BACKOFF_SECS * attempt as u64)
}

/// Counter that keeps each pull's staging directory name unique.
static PULL_STAGING_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// A private staging directory for one pull's blob writes, or `None` when
/// the runtime has no private runroot to stage in.
///
/// Podman stages every downloaded layer in a temporary blob cache under
/// TMPDIR. A failed pull leaves that scratch behind — gigabytes for a
/// large image — and the shared `libpod-tmp` cannot be swept without
/// touching a concurrent pull's files. Each pull therefore gets its own
/// directory, removed once the pull ends.
fn pull_staging_dir(config: &PodmanConfig) -> Option<std::path::PathBuf> {
    let root = config.tmpdir()?.join("staging");
    let _ = std::fs::create_dir_all(&root);
    let unique = PULL_STAGING_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory = root.join(format!("pull-{}-{unique}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    Some(directory)
}

/// Remove a pull's staging directory, best effort. A failure to remove it
/// only costs disk space; the next pull stages elsewhere.
fn remove_staging_dir(directory: &Option<std::path::PathBuf>) {
    if let Some(directory) = directory {
        let _ = std::fs::remove_dir_all(directory);
    }
}

/// [`pull_with_config`] with live progress: podman's stderr lines are
/// relayed to `on_line` as they arrive. The relay is best effort —
/// stderr closing early or staying silent never fails the pull.
pub async fn pull_with_config_progress(
    config: &PodmanConfig,
    reference: &str,
    on_line: impl FnMut(&str) + Send,
) -> Result<(), ContainerRuntimeError> {
    pull_with_config_progress_attempts(config, reference, on_line, PULL_ATTEMPTS, retry_backoff)
        .await
}

/// [`pull_with_config_progress`] with the attempt count and the wait
/// between attempts supplied by the caller.
async fn pull_with_config_progress_attempts(
    config: &PodmanConfig,
    reference: &str,
    mut on_line: impl FnMut(&str) + Send,
    attempts: usize,
    backoff: impl Fn(usize) -> Duration,
) -> Result<(), ContainerRuntimeError> {
    let mut attempt = 1;
    loop {
        let staging = pull_staging_dir(config);
        let outcome =
            pull_with_config_progress_once(config, reference, staging.as_deref(), &mut on_line)
                .await;
        remove_staging_dir(&staging);
        match outcome {
            Ok(()) => return Ok(()),
            Err(error) => {
                if !should_retry_pull(&error, attempt, attempts) {
                    return Err(error);
                }
                on_line(&format!(
                    "pull attempt {attempt} of {attempts} failed ({error}); retrying"
                ));
                tokio::time::sleep(backoff(attempt)).await;
                attempt += 1;
            }
        }
    }
}

/// One pull attempt with live stderr relayed to `on_line`.
async fn pull_with_config_progress_once<F: FnMut(&str) + Send>(
    config: &PodmanConfig,
    reference: &str,
    staging: Option<&std::path::Path>,
    on_line: &mut F,
) -> Result<(), ContainerRuntimeError> {
    let timeout_secs = pull_timeout_secs();
    let mut command = config.async_command_with_staging(staging);
    command
        .arg("pull")
        .arg(reference)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        ContainerRuntimeError::Invalid(format!("cannot spawn `{} pull`: {error}", config.program))
    })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ContainerRuntimeError::Invalid("pull child had no stderr pipe".into()))?;
    let mut tail: std::collections::VecDeque<String> = Default::default();
    let mut lines = tokio::io::BufReader::new(stderr).lines();
    while let Some(line) = lines.next_line().await.map_err(|error| {
        ContainerRuntimeError::Invalid(format!("pull stderr read failed: {error}"))
    })? {
        let text = line.trim_end();
        if text.is_empty() {
            continue;
        }
        on_line(text);
        tail.push_back(text.to_string());
        if tail.len() > PULL_TAIL_LINES {
            tail.pop_front();
        }
    }
    let status = match tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            return Err(ContainerRuntimeError::Invalid(format!(
                "cannot wait for `{} pull`: {error}",
                config.program
            )));
        }
        Err(_) => return Err(ContainerRuntimeError::Timeout { timeout_secs }),
    };
    if !status.success() {
        let tail_text = tail
            .iter()
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ContainerRuntimeError::Invalid(format!(
            "`{} pull {reference}` failed with status {}:\n{tail_text}",
            config.program,
            status.code().unwrap_or(1),
        )));
    }
    Ok(())
}

async fn pull_command(
    mut command: tokio::process::Command,
    program: &str,
    reference: &str,
) -> Result<(), ContainerRuntimeError> {
    let timeout_secs = pull_timeout_secs();
    let pulled = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        command
            .arg("pull")
            .arg(reference)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let output = match pulled {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return Err(ContainerRuntimeError::Invalid(format!(
                "cannot spawn `{program} pull`: {error}"
            )));
        }
        Err(_) => {
            return Err(ContainerRuntimeError::Timeout { timeout_secs });
        }
    };
    if !output.status.success() {
        return Err(ContainerRuntimeError::Invalid(format!(
            "`{program} pull {reference}` failed with status {}: {}",
            output.status.code().unwrap_or(1),
            truncate_captured_bytes(&output.stderr)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake executable records real argv; no Podman, network, image store or
    /// container is used. Both image operations must share the runtime's exact
    /// private root/runroot/tmpdir/driver, for both supported operator choices.
    #[cfg(unix)]
    #[tokio::test]
    async fn image_commands_use_the_same_explicit_private_backend_as_runtime() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("argv.jsonl");
        let program = directory.path().join("fake-podman");
        let reference = format!("docker.io/library/alpine@sha256:{}", "a".repeat(64));
        let body = format!(
            "#!/usr/bin/python3\nimport json,sys\nwith open({:?}, 'a') as f: f.write(json.dumps(sys.argv[1:])+'\\n')\nprint(json.dumps([{{'RepoDigests':[{reference:?}]}}]))\n",
            log.to_string_lossy()
        );
        std::fs::write(&program, body).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        for driver in [
            crate::podman::PrivateStorageDriver::Vfs,
            crate::podman::PrivateStorageDriver::Overlay,
        ] {
            let daemon = crate::ContainerExecutionInfra::for_state_dir_with_storage_driver(
                directory.path(),
                driver,
            )
            .unwrap();
            let mut config = daemon.config.clone();
            config.program = program.to_string_lossy().into_owned();
            let expected: Vec<String> = daemon
                .config
                .async_command()
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            assert!(expected.iter().any(|arg| arg == "--root"));
            assert!(expected.iter().any(|arg| arg == "--runroot"));
            assert!(expected.iter().any(|arg| arg == "--tmpdir"));
            assert!(
                local_images_with_config(&config)
                    .await
                    .unwrap()
                    .contains(&reference)
            );
            pull_with_config(&config, &reference).await.unwrap();
            let calls: Vec<Vec<String>> = std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let mut inventory = expected.clone();
            inventory.extend(["images", "--format", "json"].map(String::from));
            let mut pull = expected;
            pull.extend(["pull".to_owned(), reference.clone()]);
            assert_eq!(&calls[calls.len() - 2..], &[inventory, pull]);
        }
    }

    /// Progress pull relays podman's stderr lines as they arrive and keeps
    /// a rolling tail for the failure message. Same fake-executable
    /// pattern; no Podman, network or store involved.
    #[cfg(unix)]
    #[tokio::test]
    async fn progress_pull_relays_stderr_and_captures_failure_tail() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("fake-podman");
        let fail_marker = directory.path().join("fail-mode");
        let body = format!(
            r#"#!/usr/bin/python3
import os, sys
if os.path.exists({fail_marker:?}):
    sys.stderr.write("layer 1 done\n")
    sys.stderr.write("layer 2 failed: 502\n")
    sys.stderr.write("context line\n")
    sys.exit(125)
for index in range(200):
    sys.stderr.write(f"copying blob {{index}}\n")
sys.stdout.write("sha256:" + "0" * 64 + "\n")
"#
        );
        std::fs::write(&program, body).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let reference = format!("docker.io/library/alpine@sha256:{}", "a".repeat(64));
        let mut config = crate::ContainerExecutionInfra::for_state_dir(directory.path())
            .unwrap()
            .config;
        config.program = program.to_string_lossy().into_owned();

        let mut relayed = Vec::new();
        pull_with_config_progress(&config, &reference, |line| relayed.push(line.to_owned()))
            .await
            .unwrap();
        assert_eq!(relayed.len(), 200);
        assert_eq!(relayed.first().map(String::as_str), Some("copying blob 0"));
        assert_eq!(relayed.last().map(String::as_str), Some("copying blob 199"));

        std::fs::write(&fail_marker, b"").unwrap();
        let error = pull_with_config_progress(&config, &reference, |_| ())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("failed with status 125"), "{error}");
        assert!(error.contains("layer 1 done"), "{error}");
        assert!(error.contains("layer 2 failed: 502"), "{error}");
        assert!(!error.contains("copying blob"), "{error}");
    }

    /// A truncated layer fails the pull with "unexpected EOF". The
    /// truncation is transient, so the pull is retried and succeeds; the
    /// failed attempt's blob staging is removed instead of being left
    /// behind in the shared runroot.
    #[cfg(unix)]
    #[tokio::test]
    async fn transient_pull_failure_is_retried_and_staging_is_removed() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("fake-podman");
        let attempts_file = directory.path().join("attempts");
        let body = format!(
            r#"#!/usr/bin/python3
import os, sys
path = {attempts:?}
count = 0
if os.path.exists(path):
    count = int(open(path).read() or "0")
count += 1
open(path, "w").write(str(count))
if count < 3:
    sys.stderr.write("Error: writing blob: storing blob to file: happened during read: unexpected EOF\n")
    sys.exit(125)
sys.stderr.write("copying blob done\n")
"#,
            attempts = attempts_file.to_string_lossy()
        );
        std::fs::write(&program, body).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let reference = format!("docker.io/library/alpine@sha256:{}", "a".repeat(64));
        let mut config = crate::ContainerExecutionInfra::for_state_dir(directory.path())
            .unwrap()
            .config;
        config.program = program.to_string_lossy().into_owned();

        let mut relayed = Vec::new();
        pull_with_config_progress_attempts(
            &config,
            &reference,
            |line| relayed.push(line.to_owned()),
            3,
            |_| Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&attempts_file).unwrap().trim(), "3");
        assert!(
            relayed
                .iter()
                .any(|line| line.contains("retrying") && line.contains("unexpected EOF")),
            "{relayed:?}"
        );
        // Every attempt's staging directory is gone; nothing is left to
        // accumulate gigabytes per failed pull.
        let staging_root = config.tmpdir().unwrap().join("staging");
        let leftovers = std::fs::read_dir(&staging_root)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(leftovers, 0, "staging left behind in {staging_root:?}");
    }

    /// A pull that keeps failing is reported after the attempt budget, not
    /// retried forever.
    #[cfg(unix)]
    #[tokio::test]
    async fn pull_gives_up_after_the_attempt_budget() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("fake-podman");
        let attempts_file = directory.path().join("attempts");
        let body = format!(
            r#"#!/usr/bin/python3
import os, sys
path = {attempts:?}
count = 0
if os.path.exists(path):
    count = int(open(path).read() or "0")
count += 1
open(path, "w").write(str(count))
sys.stderr.write("Error: writing blob: happened during read: unexpected EOF\n")
sys.exit(125)
"#,
            attempts = attempts_file.to_string_lossy()
        );
        std::fs::write(&program, body).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let reference = format!("docker.io/library/alpine@sha256:{}", "a".repeat(64));
        let mut config = crate::ContainerExecutionInfra::for_state_dir(directory.path())
            .unwrap()
            .config;
        config.program = program.to_string_lossy().into_owned();

        let error = pull_with_config_progress_attempts(
            &config,
            &reference,
            |_| (),
            2,
            |_| Duration::from_secs(0),
        )
        .await
        .unwrap_err()
        .to_string();
        assert_eq!(std::fs::read_to_string(&attempts_file).unwrap().trim(), "2");
        assert!(error.contains("unexpected EOF"), "{error}");
    }

    #[test]
    fn pull_timeout_uses_default_for_unset_invalid_or_zero_values() {
        assert_eq!(pull_timeout_from(None), DEFAULT_PULL_TIMEOUT_SECS);
        assert_eq!(pull_timeout_from(Some("")), DEFAULT_PULL_TIMEOUT_SECS);
        assert_eq!(pull_timeout_from(Some("0")), DEFAULT_PULL_TIMEOUT_SECS);
        assert_eq!(
            pull_timeout_from(Some("not-a-number")),
            DEFAULT_PULL_TIMEOUT_SECS
        );
    }

    #[test]
    fn pull_timeout_accepts_explicit_override() {
        assert_eq!(pull_timeout_from(Some("7200")), 7200);
        assert_eq!(pull_timeout_from(Some(" 1800 ")), 1800);
    }

    #[test]
    fn default_pull_timeout_is_generous_enough_for_multi_gigabyte_images() {
        const _: () = assert!(DEFAULT_PULL_TIMEOUT_SECS == 3600);
    }

    #[test]
    fn repo_digests_parse_dedup_and_tolerate_missing_fields() {
        let payload = br#"[
            {"Id":"a","RepoDigests":["docker.io/library/alpine@sha256:aaa","docker.io/library/alpine@sha256:aaa"]},
            {"Id":"b","RepoDigests":["quay.io/biocontainers/samtools@sha256:bbb"]},
            {"Id":"c"}
        ]"#;
        let images = parse_local_images(payload);
        assert!(images.contains("docker.io/library/alpine@sha256:aaa"));
        assert!(images.contains("quay.io/biocontainers/samtools@sha256:bbb"));
        assert!(images.contains("docker.io/library/alpine@sha256:ccc") == false);
    }

    /// Podman 3.x reports `RepoDigests` as bare digests and keeps the
    /// repository in `Names` only. A digest-pulled image must still
    /// match its declared `repo@sha256:…` reference, otherwise every
    /// image looks permanently missing and `prepare` re-pulls the whole
    /// catalogue forever.
    #[test]
    fn podman_three_style_bare_repo_digests_still_match_declared_references() {
        let payload = br#"[
            {
                "Id":"9cb701c20b595f4442ba074b2ec924f396f4c474084608116901ce3098ebfe3d",
                "RepoTags": null,
                "RepoDigests": ["sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507"],
                "Names": ["docker.io/library/alpine@sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507"],
                "Digest": "sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507"
            }
        ]"#;
        let images = parse_local_images(payload);
        assert!(images.contains(
            "docker.io/library/alpine@sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507"
        ));
        // The same digest under a different repository is a different
        // image and must not satisfy the reference.
        assert!(
            !images.contains(
                "quay.io/biocontainers/alpine@sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507"
            )
        );
    }

    /// An image pulled by tag carries the repository in `Names` and the
    /// manifest digest in `Digest`; the declared digest-pinned
    /// reference must match it.
    #[test]
    fn tag_pulled_images_match_their_digest_pinned_reference() {
        let payload = br#"[
            {
                "Id":"abc",
                "RepoTags": ["quay.io/biocontainers/samtools:1.19"],
                "RepoDigests": ["sha256:bbb"],
                "Names": ["quay.io/biocontainers/samtools:1.19"],
                "Digest": "sha256:bbb"
            }
        ]"#;
        let images = parse_local_images(payload);
        assert!(images.contains("quay.io/biocontainers/samtools@sha256:bbb"));
        assert!(!images.contains("quay.io/biocontainers/bwa@sha256:bbb"));
    }

    #[test]
    fn repo_digests_empty_and_unexpected_payloads_yield_an_empty_set() {
        assert!(parse_local_images(b"[]").is_empty());
        assert!(parse_local_images(b"").is_empty());
        assert!(parse_local_images(b"not json").is_empty());
    }
}
