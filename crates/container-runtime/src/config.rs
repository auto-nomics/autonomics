//! Process-wide container execution configuration.

use std::path::{Path, PathBuf};

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
}
