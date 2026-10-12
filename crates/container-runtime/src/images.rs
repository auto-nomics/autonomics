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
use crate::podman::async_podman_command;

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
    let listed = tokio::time::timeout(
        Duration::from_secs(LIST_TIMEOUT_SECS),
        async_podman_command(program)
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
    let timeout_secs = pull_timeout_secs();
    let pulled = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        async_podman_command(program)
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
