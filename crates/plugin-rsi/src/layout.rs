//! State-directory layout for plugin development and installed plugins.
//!
//! Version 2 namespaces all plugin state beneath `plugins/`.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{Error, Result, names::validate_plugin_name, request::atomic_toml};

const V2_MARKER: &str = "plugins/.autonomics-layout.toml";
const LEGACY_WORKSPACE_DIR: &str = "plugins";
const LEGACY_RUNTIME_DIR: &str = "plugin-runtime";
const LEGACY_SNAPSHOT_DIR: &str = "plugin-installs";
const LEGACY_REQUEST_DIR: &str = "plugin-rsi/requests";

/// Which on-disk generation a state directory uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginLayoutVersion {
    /// Pre-2 flat directories.
    Legacy,
    /// Namespaced layout beneath `plugins/`.
    V2,
}

impl PluginLayoutVersion {
    pub fn as_u32(self) -> u32 {
        match self {
            Self::Legacy => 1,
            Self::V2 => 2,
        }
    }
}

/// Resolved state paths for one plugin state directory.
#[derive(Debug, Clone)]
pub struct PluginStateLayout {
    state_dir: PathBuf,
    version: PluginLayoutVersion,
}

#[derive(Debug, Serialize)]
struct LayoutMarker {
    version: u32,
}

impl PluginStateLayout {
    /// Explicitly construct the pre-2 layout.
    pub fn legacy(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: state_dir.into(),
            version: PluginLayoutVersion::Legacy,
        }
    }

    /// Explicitly construct the version-2 layout.
    pub fn v2(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: state_dir.into(),
            version: PluginLayoutVersion::V2,
        }
    }

    /// Detect an existing state layout without moving data.
    pub fn open(state_dir: impl Into<PathBuf>) -> Self {
        let state_dir = state_dir.into();
        if state_dir.join(V2_MARKER).is_file()
            || ![
                LEGACY_WORKSPACE_DIR,
                LEGACY_RUNTIME_DIR,
                LEGACY_SNAPSHOT_DIR,
                LEGACY_REQUEST_DIR,
            ]
            .iter()
            .any(|legacy_dir| state_dir.join(legacy_dir).exists())
        {
            Self::v2(state_dir)
        } else {
            Self::legacy(state_dir)
        }
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn version(&self) -> PluginLayoutVersion {
        self.version
    }

    pub fn workspace_root(&self) -> PathBuf {
        match self.version {
            PluginLayoutVersion::Legacy => self.state_dir.join(LEGACY_WORKSPACE_DIR),
            PluginLayoutVersion::V2 => self.state_dir.join("plugins").join("dev"),
        }
    }

    pub fn snapshot_root(&self) -> PathBuf {
        match self.version {
            PluginLayoutVersion::Legacy => self.state_dir.join(LEGACY_SNAPSHOT_DIR),
            PluginLayoutVersion::V2 => self.state_dir.join("plugins").join("snapshots"),
        }
    }

    pub fn runtime_root(&self) -> PathBuf {
        match self.version {
            PluginLayoutVersion::Legacy => self.state_dir.join(LEGACY_RUNTIME_DIR),
            PluginLayoutVersion::V2 => self.state_dir.join("plugins").join("runtime"),
        }
    }

    pub fn registry_path(&self) -> PathBuf {
        self.state_dir
            .join(container_plugin::sync::PLUGIN_CONFIG_FILE)
    }

    pub fn requests_root(&self) -> PathBuf {
        match self.version {
            PluginLayoutVersion::Legacy => self.state_dir.join(LEGACY_REQUEST_DIR),
            PluginLayoutVersion::V2 => self
                .state_dir
                .join("evolution")
                .join("plugin")
                .join("requests"),
        }
    }

    pub fn workspace_path(&self, plugin_name: &str) -> Result<PathBuf> {
        validate_plugin_name(plugin_name)?;
        Ok(self.workspace_root().join(plugin_name))
    }

    pub fn marker_path(&self) -> PathBuf {
        self.state_dir.join(V2_MARKER)
    }

    /// Create v2 storage directories.
    pub fn ensure_v2_directories(&self) -> Result<()> {
        if self.version != PluginLayoutVersion::V2 {
            return Err(Error::Validation(
                "cannot create v2 directories for a legacy plugin layout".into(),
            ));
        }
        std::fs::create_dir_all(self.workspace_root())?;
        std::fs::create_dir_all(self.snapshot_root())?;
        std::fs::create_dir_all(self.runtime_root())?;
        if !self.marker_path().is_file() {
            atomic_toml(
                &self.marker_path(),
                &LayoutMarker {
                    version: self.version.as_u32(),
                },
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_uses_v2_and_creates_namespaced_roots() {
        let state = tempfile::tempdir().unwrap();
        let layout = PluginStateLayout::open(state.path());
        assert_eq!(layout.version(), PluginLayoutVersion::V2);

        layout.ensure_v2_directories().unwrap();
        assert!(layout.workspace_root().is_dir());
        assert!(layout.snapshot_root().is_dir());
        assert!(layout.runtime_root().is_dir());
        assert_eq!(layout.workspace_root(), state.path().join("plugins/dev"));
        assert_eq!(layout.runtime_root(), state.path().join("plugins/runtime"));
        assert!(layout.marker_path().is_file());
    }

    #[test]
    fn legacy_state_is_detected_and_left_untouched() {
        let state = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(state.path().join("plugins")).unwrap();
        std::fs::create_dir_all(state.path().join("plugin-runtime")).unwrap();
        std::fs::create_dir_all(state.path().join("plugin-installs")).unwrap();
        std::fs::create_dir_all(state.path().join("plugin-rsi/requests")).unwrap();

        let layout = PluginStateLayout::open(state.path());
        assert_eq!(layout.version(), PluginLayoutVersion::Legacy);
        assert_eq!(layout.workspace_root(), state.path().join("plugins"));
        assert_eq!(layout.runtime_root(), state.path().join("plugin-runtime"));
        assert!(!layout.marker_path().exists());
    }

    #[test]
    fn v2_marker_is_authoritative() {
        let state = tempfile::tempdir().unwrap();
        let layout = PluginStateLayout::v2(state.path());
        layout.ensure_v2_directories().unwrap();

        // A v2 marker wins even if stale legacy directories are present.
        std::fs::create_dir_all(state.path().join("plugin-runtime")).unwrap();
        let reopened = PluginStateLayout::open(state.path());
        assert_eq!(reopened.version(), PluginLayoutVersion::V2);
        assert_eq!(
            reopened.runtime_root(),
            state.path().join("plugins/runtime")
        );
    }
}
