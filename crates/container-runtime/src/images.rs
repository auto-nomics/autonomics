//! Local image-store inventory and pre-pull for digest-pinned images.
//!
//! [`local_digests`] reads the whole store in one `podman images` call
//! (one process instead of N per-image inspects); [`pull`] pre-fetches
//! one digest-pinned reference. Overlapping *pull* processes is safe —
//! the store serialises mutations through its own file locks — while
//! container *creation* keeps the `CREATE_GATE` precedent in
//! `podman.rs`; layer parallelism inside a single `podman pull` still
//! applies.

use std::collections::BTreeSet;
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

/// All repository digests present in local storage, as `repo@sha256:…`
/// strings — the union of `RepoDigests` across every image, from one
/// `podman images --format json` call. Digest-pinned references
/// (`ImageReference::as_str()`) match this set by exact string: every
/// reference the codebase declares is fully qualified, which is the
/// form podman reports.
pub async fn local_digests(program: &str) -> Result<BTreeSet<String>, ContainerRuntimeError> {
    local_digests_command(async_podman_command(program), program).await
}

/// Inventory the same explicit private backend used for actual DAG containers.
pub async fn local_digests_with_config(
    config: &PodmanConfig,
) -> Result<BTreeSet<String>, ContainerRuntimeError> {
    local_digests_command(config.async_command(), &config.program).await
}

async fn local_digests_command(
    mut command: tokio::process::Command,
    program: &str,
) -> Result<BTreeSet<String>, ContainerRuntimeError> {
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
    Ok(parse_repo_digests(&output.stdout))
}

/// One row of `podman images --format json`; only `RepoDigests` is
/// needed here.
#[derive(Deserialize)]
struct ImageRow {
    #[serde(rename = "RepoDigests", default)]
    repo_digests: Vec<String>,
}

/// Parse the `podman images --format json` payload into the deduplicated
/// set of `repo@sha256:…` strings. A payload that does not parse as the
/// expected array yields an empty set — the caller reports every
/// declared image as missing rather than failing the whole inventory.
fn parse_repo_digests(payload: &[u8]) -> BTreeSet<String> {
    let rows: Vec<ImageRow> = serde_json::from_slice(payload).unwrap_or_default();
    rows.into_iter().flat_map(|row| row.repo_digests).collect()
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
    pull_command(config.async_command(), &config.program, reference).await
}

/// Rolling stderr tail kept for pull failure messages.
const PULL_TAIL_LINES: usize = 8;

/// [`pull_with_config`] with live progress: podman's stderr lines are
/// relayed to `on_line` as they arrive. The relay is best effort —
/// stderr closing early or staying silent never fails the pull.
pub async fn pull_with_config_progress(
    config: &PodmanConfig,
    reference: &str,
    mut on_line: impl FnMut(&str),
) -> Result<(), ContainerRuntimeError> {
    let timeout_secs = pull_timeout_secs();
    let mut command = config.async_command();
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
                local_digests_with_config(&config)
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
        let digests = parse_repo_digests(payload);
        assert_eq!(
            digests,
            BTreeSet::from([
                "docker.io/library/alpine@sha256:aaa".to_owned(),
                "quay.io/biocontainers/samtools@sha256:bbb".to_owned(),
            ])
        );
    }

    #[test]
    fn repo_digests_empty_and_unexpected_payloads_yield_an_empty_set() {
        assert!(parse_repo_digests(b"[]").is_empty());
        assert!(parse_repo_digests(b"").is_empty());
        assert!(parse_repo_digests(b"not json").is_empty());
    }
}
