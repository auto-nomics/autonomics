//! Environment (image) development manifest and lifecycle state.
//!
//! Mirrors the plugin manifest's shape conventions but lives in this crate:
//! `container-plugin` owns node/plugin protocol types and must not learn
//! about environment development. `manifest.toml` inside an environment
//! workspace is daemon-written; agents mutate it only through the
//! `environment_manifest_update` tool.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Daemon-owned lifecycle marker stored in `manifest.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentStatus {
    /// Created, not yet validated.
    Draft,
    /// Currently running validation gates.
    Validating,
    /// A validation gate failed; the workspace awaits repairs.
    NeedsFix,
    /// An in-place update of an active environment.
    Updating,
    /// Submitted for review by the background distiller.
    PendingReview,
    /// Approved by review and ready for publication.
    Approved,
    /// Currently pushing to the configured registry.
    Publishing,
    /// The push failed; the workspace returns to repairable states.
    PublishFailed,
    /// Successfully pushed to the registry.
    Published,
    /// Rejected by review; retained as negative feedback.
    Rejected,
    /// Publication finished; the catalog re-pin is being finalized.
    InstallPending,
    /// The registry reference is active in the environment catalog.
    Installed,
}

/// Digest-pinned base image provenance for one environment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentBase {
    /// Full `FROM` reference `host/path@sha256:…`; validated at load time.
    pub reference: String,
    /// Display-only tag for the base image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Upstream source description (e.g. `Bioconductor 3.20`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// SPDX license identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

/// One smoke test executed against a freshly built image.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentSmokeTest {
    pub name: String,
    /// Executable and arguments. No shell string is accepted.
    pub argv: Vec<String>,
    /// Optional substring assertion on the test's stdout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_stdout_contains: Option<String>,
}

/// Daemon-owned lifecycle facts, mirroring plugin lifecycle metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentLifecycleMetadata {
    /// Requests that motivated the current development or update.
    #[serde(default)]
    pub request_ids: Vec<String>,
    /// Host-provided reason retained with the lifecycle record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// Repository-relative latest validation report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_report: Option<String>,
    /// Whether the current local activation should enter background publication.
    #[serde(default)]
    pub publication_pending: bool,
    /// Catalog reference replaced by the current activation (rollback target).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_reference: Option<String>,
    /// Last locally built image tag (`localhost/…:rsi-<attempt>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_reference: Option<String>,
    /// Digest-pinned registry reference after a successful push.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_reference: Option<String>,
    /// Environment workspace used as the fork source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_environment: Option<String>,
}

/// One environment development workspace's manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentManifest {
    /// Must equal the workspace directory name.
    pub environment_id: String,
    /// Daemon-owned lifecycle marker.
    pub status: EnvironmentStatus,
    /// Executables the image guarantees, e.g. `["Rscript", "sh"]`.
    #[serde(default)]
    pub interpreters: Vec<String>,
    /// Base image provenance; the `FROM` of the build.
    pub base: EnvironmentBase,
    /// Repository-relative path of the Containerfile.
    #[serde(default = "default_containerfile")]
    pub containerfile: String,
    /// Smoke tests; at least one is required for a passing build.
    #[serde(default)]
    pub tests: Vec<EnvironmentSmokeTest>,
    #[serde(default)]
    pub lifecycle: EnvironmentLifecycleMetadata,
}

fn default_containerfile() -> String {
    "Containerfile".into()
}

pub(crate) fn environment_is_editable(status: EnvironmentStatus) -> bool {
    matches!(
        status,
        EnvironmentStatus::Draft
            | EnvironmentStatus::Updating
            | EnvironmentStatus::NeedsFix
            | EnvironmentStatus::PublishFailed
    )
}

/// Explicit transition whitelist for the environment lifecycle.
pub(crate) fn ensure_environment_transition(
    from: EnvironmentStatus,
    to: EnvironmentStatus,
) -> Result<()> {
    let valid = match (from, to) {
        (EnvironmentStatus::Draft, EnvironmentStatus::Validating)
        | (EnvironmentStatus::Draft, EnvironmentStatus::Updating)
        | (EnvironmentStatus::Draft, EnvironmentStatus::PendingReview)
        | (EnvironmentStatus::Installed, EnvironmentStatus::Updating)
        | (EnvironmentStatus::Updating, EnvironmentStatus::Validating)
        | (EnvironmentStatus::Updating, EnvironmentStatus::PendingReview)
        | (EnvironmentStatus::NeedsFix, EnvironmentStatus::Validating)
        | (EnvironmentStatus::Validating, EnvironmentStatus::NeedsFix)
        | (EnvironmentStatus::Validating, EnvironmentStatus::Draft)
        | (EnvironmentStatus::Validating, EnvironmentStatus::Updating)
        | (EnvironmentStatus::Validating, EnvironmentStatus::PendingReview)
        | (EnvironmentStatus::PendingReview, EnvironmentStatus::Updating)
        | (EnvironmentStatus::PendingReview, EnvironmentStatus::Approved)
        | (EnvironmentStatus::PendingReview, EnvironmentStatus::Rejected)
        | (EnvironmentStatus::Approved, EnvironmentStatus::Publishing)
        | (EnvironmentStatus::Publishing, EnvironmentStatus::Published)
        | (EnvironmentStatus::Publishing, EnvironmentStatus::PublishFailed)
        | (EnvironmentStatus::PublishFailed, EnvironmentStatus::Updating)
        | (EnvironmentStatus::Published, EnvironmentStatus::Updating)
        | (EnvironmentStatus::Published, EnvironmentStatus::InstallPending)
        | (EnvironmentStatus::InstallPending, EnvironmentStatus::Installed) => true,
        (from, to) if from == to => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidTransition {
            from: format!("{from:?}").to_lowercase(),
            to: format!("{to:?}").to_lowercase(),
        })
    }
}

/// Validate one interpreter token: a single shell-free executable name.
///
/// Interpreters are interpolated into daemon-built probe commands, so they
/// must contain no whitespace or shell metacharacters; casing is preserved
/// (`Rscript`, `python3`, `STAR`).
pub(crate) fn validate_interpreter_token(interpreter: &str) -> Result<()> {
    let valid = !interpreter.is_empty()
        && interpreter.len() <= 64
        && interpreter
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+'));
    if valid {
        Ok(())
    } else {
        Err(Error::Validation(format!(
            "interpreter `{interpreter}` must be a single executable name \
             (letters, digits, `.`, `_`, `+`)"
        )))
    }
}

/// Validate a whole interpreter list: tokens plus no duplicates after
/// lowercasing (the catalog registry applies the same rule on approval).
pub(crate) fn validate_interpreters(interpreters: &[String]) -> Result<()> {
    if interpreters.is_empty() {
        return Err(Error::Validation(
            "environment must declare at least one interpreter".into(),
        ));
    }
    let mut normalized = std::collections::BTreeMap::new();
    for interpreter in interpreters {
        validate_interpreter_token(interpreter)?;
        normalized.insert(interpreter.trim().to_ascii_lowercase(), ());
    }
    if normalized.len() != interpreters.len() {
        return Err(Error::Validation(
            "environment contains duplicate interpreters".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_follow_the_explicit_lifecycle_whitelist() {
        assert!(
            ensure_environment_transition(EnvironmentStatus::Draft, EnvironmentStatus::Validating)
                .is_ok()
        );
        assert!(
            ensure_environment_transition(
                EnvironmentStatus::Approved,
                EnvironmentStatus::Publishing
            )
            .is_ok()
        );
        assert!(
            ensure_environment_transition(
                EnvironmentStatus::Publishing,
                EnvironmentStatus::PublishFailed
            )
            .is_ok()
        );
        assert!(
            ensure_environment_transition(EnvironmentStatus::Published, EnvironmentStatus::Draft)
                .is_err()
        );
        assert!(
            ensure_environment_transition(EnvironmentStatus::Rejected, EnvironmentStatus::Approved)
                .is_err()
        );
    }

    #[test]
    fn interpreters_are_unique_shell_free_tokens() {
        assert!(validate_interpreters(&["Rscript".into(), "sh".into()]).is_ok());
        assert!(validate_interpreters(&["STAR".into(), "python3".into()]).is_ok());
        assert!(validate_interpreters(&[]).is_err());
        assert!(
            validate_interpreters(&["sh".into(), "SH".into()]).is_err(),
            "duplicates are case-insensitive"
        );
        assert!(validate_interpreter_token("x && rm").is_err());
        assert!(validate_interpreter_token("sh;rm").is_err());
        assert!(validate_interpreter_token("").is_err());
    }
}
