use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogConfig {
    /// Backend ID defined in the same `vfs.toml`.
    pub backend: String,
    /// Catalog root prefix inside that backend.
    #[serde(default = "default_source")]
    pub source: String,
    /// Root index object name relative to `source`.
    #[serde(default = "default_index")]
    pub index: String,
    /// Immutable entry prefix relative to `source`.
    #[serde(default = "default_prefix")]
    pub prefix: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Expose the catalog root under `/catalog` for agent inspection.
    #[serde(default = "default_true")]
    pub agent_visible: bool,
}

fn default_source() -> String {
    "/".into()
}

fn default_index() -> String {
    "index.json".into()
}

fn default_prefix() -> String {
    "entries".into()
}

fn default_true() -> bool {
    true
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            backend: String::new(),
            source: default_source(),
            index: default_index(),
            prefix: default_prefix(),
            enabled: true,
            agent_visible: true,
        }
    }
}

impl CatalogConfig {
    pub fn from_vfs_toml(source: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default)]
            catalog: Option<CatalogConfig>,
        }
        let wrapper: Wrapper =
            toml::from_str(source).map_err(|error| format!("parse catalog config: {error}"))?;
        let config = wrapper.catalog.unwrap_or_default();
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.backend.trim().is_empty() {
            return Err("catalog backend cannot be empty".into());
        }
        if self.source != "/" {
            crate::model::validate_object_path(&self.source)?;
        }
        if self.source != "/" {
            crate::model::validate_relative_path(self.source.trim_matches('/'))?;
        }
        crate::model::validate_relative_path(self.index.trim_start_matches('/'))?;
        crate::model::validate_relative_path(self.prefix.trim_matches('/'))?;
        Ok(())
    }

    pub fn source_prefix(&self) -> String {
        normalize_prefix(&self.source)
    }

    pub fn object_key(&self, relative: &str) -> String {
        join_object_key(&self.source_prefix(), relative)
    }
}

pub fn normalize_prefix(value: &str) -> String {
    value.trim_matches('/').to_string()
}

pub fn join_object_key(prefix: &str, relative: &str) -> String {
    let relative = relative.trim_start_matches('/');
    if prefix.is_empty() {
        relative.to_string()
    } else {
        format!("{prefix}/{relative}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_optional_catalog_section() {
        assert!(
            CatalogConfig::from_vfs_toml("[[mount]]\npath=\"/\"\nbackend=\"x\"\nsource=\"/\"\n")
                .is_err()
        );

        let config = CatalogConfig::from_vfs_toml(
            r#"
[catalog]
backend = "warehouse"
source = "/autonomics/catalog"
index = "index.json"
prefix = "entries"
"#,
        )
        .unwrap();
        assert_eq!(
            config.object_key("entries/x/files"),
            "autonomics/catalog/entries/x/files"
        );
    }
}
