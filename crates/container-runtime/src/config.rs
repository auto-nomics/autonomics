//! Process-wide container execution configuration.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::gc::{DEFAULT_WORKSPACE_GC_AGE_SECS, DEFAULT_WORKSPACE_GC_INTERVAL_SECS};

/// Env var that used to select the `k3s` or `podman` backend. The k3s
/// backend has been removed; a set value other than `podman` is an error so
/// stale deployments fail loudly instead of silently switching runtimes.
pub const REMOVED_BACKEND_ENV: &str = "AUTONOMICS_CONTAINER_BACKEND";

/// Validate a `AUTONOMICS_CONTAINER_BACKEND` value: only unset/empty/`podman`
/// is accepted now that the k3s backend is gone.
pub fn parse_removed_backend_env(value: &str) -> Result<(), String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "podman" => Ok(()),
        other => Err(format!(
            "container backend `{other}` is not supported anymore (the k3s backend was removed); \
             unset {REMOVED_BACKEND_ENV} or set it to `podman`"
        )),
    }
}

/// Fail when the environment still asks for a removed container backend.
pub fn ensure_backend_env_removed() -> Result<(), String> {
    let value = std::env::var_os(REMOVED_BACKEND_ENV)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    parse_removed_backend_env(&value)
}

/// Directory for DAG container file flow
pub fn podman_state_root() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Path::new(&home).join(".autonomics").join("state");
    }
    std::env::temp_dir().join("autonomics").join("podman")
}

pub fn default_panel_cache_root() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Path::new(&home).join(".autonomics").join("panels");
    }
    std::env::temp_dir().join("autonomics").join("panels")
}

// ─────────────────────────── garbage collection ────────────────────

/// When set to a truthy value, successful container runs keep their scratch
/// workspace on disk (debugging escape hatch). The background sweeper still
/// reclaims it after the GC age window.
pub const KEEP_WORKSPACE_ENV: &str = "AUTONOMICS_KEEP_WORKSPACE";
/// Minimum age before the sweeper removes a lock-free scratch directory
/// owned by a live process (seconds).
pub const WORKSPACE_GC_AGE_ENV: &str = "AUTONOMICS_WORKSPACE_GC_AGE_SECS";
/// Delay between background sweeps (seconds); `0` disables the periodic task.
pub const WORKSPACE_GC_INTERVAL_ENV: &str = "AUTONOMICS_WORKSPACE_GC_INTERVAL_SECS";
/// Minimum age before the work-dir sweep may reclaim a persistent,
/// content-addressed container work directory (seconds). Unset, `0`, or
/// invalid means work dirs are never reclaimed — they are the durable
/// store of container outputs.
pub const WORK_DIR_GC_AGE_ENV: &str = "AUTONOMICS_WORK_DIR_GC_AGE_SECS";

/// Whether successful runs must keep their scratch directory.
pub fn keep_workspace_enabled() -> bool {
    parse_env_flag(std::env::var_os(KEEP_WORKSPACE_ENV).as_deref())
}

/// Retention window for lock-free scratch owned by a live process.
pub fn workspace_gc_age() -> Duration {
    parse_env_secs(
        std::env::var_os(WORKSPACE_GC_AGE_ENV).as_deref(),
        DEFAULT_WORKSPACE_GC_AGE_SECS,
    )
}

/// Delay between background sweeps; `None` disables the periodic task.
pub fn workspace_gc_interval() -> Option<Duration> {
    let secs = parse_env_secs(
        std::env::var_os(WORKSPACE_GC_INTERVAL_ENV).as_deref(),
        DEFAULT_WORKSPACE_GC_INTERVAL_SECS,
    );
    (secs.as_secs() > 0).then_some(secs)
}

/// Retention window for persistent work dirs; `None` disables the work-dir
/// sweep entirely (the default — work dirs hold container outputs).
pub fn work_dir_gc_age() -> Option<Duration> {
    let raw = std::env::var_os(WORK_DIR_GC_AGE_ENV)?;
    let secs = parse_env_secs(Some(raw.as_os_str()), 0);
    (secs.as_secs() > 0).then_some(secs)
}

pub(crate) fn parse_env_flag(raw: Option<&OsStr>) -> bool {
    raw.is_some_and(|value| {
        matches!(
            value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub(crate) fn parse_env_secs(raw: Option<&OsStr>, default: u64) -> Duration {
    raw.and_then(|value| value.to_string_lossy().trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(default))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_backend_env_only_accepts_podman() {
        assert!(parse_removed_backend_env("").is_ok());
        assert!(parse_removed_backend_env(" PodMan ").is_ok());
        let error = parse_removed_backend_env("k3s").unwrap_err();
        assert!(error.contains("k3s backend was removed"));
        assert!(parse_removed_backend_env("docker").is_err());
    }

    #[test]
    fn panel_cache_default_is_shared_under_user_home() {
        let path = default_panel_cache_root();
        assert!(path.ends_with(Path::new(".autonomics/panels")));
        assert!(path.is_absolute());
    }

    #[test]
    fn gc_env_parsing_accepts_truthy_values_and_positive_numbers() {
        use std::ffi::OsStr;
        use std::time::Duration;

        assert!(!parse_env_flag(None));
        for off in ["", "0", "false", "no", "off", " keep "] {
            assert!(!parse_env_flag(Some(OsStr::new(off))), "`{off}`");
        }
        for on in ["1", "true", "YES", "On"] {
            assert!(parse_env_flag(Some(OsStr::new(on))), "`{on}`");
        }

        assert_eq!(
            parse_env_secs(Some(OsStr::new("3600")), 60),
            Duration::from_secs(3600)
        );
        assert_eq!(
            parse_env_secs(Some(OsStr::new("0")), 60),
            Duration::from_secs(60),
            "zero falls back to the default instead of meaning \"delete instantly\""
        );
        assert_eq!(
            parse_env_secs(Some(OsStr::new("not-a-number")), 60),
            Duration::from_secs(60)
        );
        assert_eq!(parse_env_secs(None, 60), Duration::from_secs(60));
    }
}
