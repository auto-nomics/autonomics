//! Process-wide container backend configuration.

use std::path::{Path, PathBuf};

pub const CONTAINER_BACKEND_ENV: &str = "AUTONOMICS_CONTAINER_BACKEND";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerBackend {
    K3s,
    Podman,
}

impl ContainerBackend {
    pub fn from_env() -> Result<Self, String> {
        let value = std::env::var_os(CONTAINER_BACKEND_ENV)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::parse(&value)
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "podman" => Ok(Self::Podman),
            "k3s" => Ok(Self::K3s),
            other => Err(format!(
                "unsupported container backend `{other}`; expected `k3s` or `podman`"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::K3s => "k3s",
            Self::Podman => "podman",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_backend_selects_podman_and_explicit_values_are_case_insensitive() {
        assert_eq!(
            ContainerBackend::parse("").unwrap(),
            ContainerBackend::Podman
        );
        assert_eq!(
            ContainerBackend::parse(" Podman ").unwrap(),
            ContainerBackend::Podman
        );
        assert_eq!(
            ContainerBackend::parse("K3S").unwrap(),
            ContainerBackend::K3s
        );
        assert!(ContainerBackend::parse("docker").is_err());
    }

    #[test]
    fn panel_cache_default_is_shared_under_user_home() {
        let path = default_panel_cache_root();
        assert!(path.ends_with(Path::new(".autonomics/panels")));
        assert!(path.is_absolute());
    }
}

pub fn podman_state_root() -> PathBuf {
    if let Some(root) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return Path::new(&root).join("autonomics").join("podman");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Path::new(&home)
            .join(".local")
            .join("state")
            .join("autonomics")
            .join("podman");
    }
    std::env::temp_dir().join("autonomics").join("podman")
}

pub fn default_panel_cache_root() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Path::new(&home).join(".autonomics").join("panels");
    }
    std::env::temp_dir().join("autonomics").join("panels")
}
