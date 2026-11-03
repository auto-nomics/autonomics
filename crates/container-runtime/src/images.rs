//! Local image-store inventory and pre-pull for digest-pinned images.
//!
//! [`local_digests`] reads the whole store in one `podman images` call
//! (one process instead of N per-image inspects); [`pull`] pre-fetches
//! one digest-pinned reference. Callers pull **sequentially** — the
//! `CREATE_GATE` precedent in `podman.rs` argues against concurrent
//! storage mutation; layer parallelism inside a single `podman pull`
//! still applies.

use std::collections::BTreeSet;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;

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
