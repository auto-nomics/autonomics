use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogConfig {
    /// Backend ID defined in the same `vfs.toml`. Ignored when `repository`
    /// is set.
    #[serde(default)]
    pub backend: Option<String>,
    /// Hugging Face dataset repository in `owner/name` form. When set, the
    /// catalog is hosted on the Hub instead of object storage.
    #[serde(default)]
    pub repository: Option<String>,
    /// Prefix for per-package dataset repositories in `owner/name` form.
    /// The index lives at `repository`; each package lives at
    /// `{repository_prefix}-{sanitized-id}`.
    #[serde(default)]
    pub repository_prefix: Option<String>,
    /// Hugging Face revision (branch or tag); defaults to `main`.
    #[serde(default)]
    pub revision: Option<String>,
    /// Catalog root prefix inside that backend.
    #[serde(default = "default_source")]
    pub source: String,
    /// Root index object name relative to `source`.
    #[serde(default = "default_index")]
    pub index: String,
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

fn default_true() -> bool {
    true
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            backend: None,
            repository: None,
            repository_prefix: None,
            revision: None,
            source: default_source(),
            index: default_index(),
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
        let backend = self
            .backend
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        match (&self.repository, backend) {
            (Some(repository), _) => {
                let (owner, name) = repository.split_once('/').ok_or_else(|| {
                    format!("catalog repository must be `owner/name`, got `{repository}`")
                })?;
                if owner.is_empty() || name.is_empty() || name.contains('/') {
                    return Err(format!(
                        "catalog repository must be `owner/name`, got `{repository}`"
                    )
                    .into());
                }
            }
            (None, Some(_)) => {}
            (None, None) => return Err("catalog backend or repository is required".into()),
        }
        if self.revision.is_some() && self.repository.is_none() {
            return Err("catalog revision requires repository".into());
        }
        if backend.is_some() {
            if self.source != "/" {
                crate::model::validate_object_path(&self.source)?;
                crate::model::validate_relative_path(self.source.trim_matches('/'))?;
            }
            crate::model::validate_relative_path(self.index.trim_start_matches('/'))?;
        }
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
"#,
        )
        .unwrap();
        assert_eq!(
            config.object_key("entries/x/files"),
            "autonomics/catalog/entries/x/files"
        );
    }

    #[test]
    fn parses_hugging_face_repository_catalog() {
        let config = CatalogConfig::from_vfs_toml(
            r#"
[catalog]
repository = "wjixiang/autonomics-catalog-test"
revision = "main"
"#,
        )
        .unwrap();
        assert_eq!(
            config.repository.as_deref(),
            Some("wjixiang/autonomics-catalog-test")
        );
        assert_eq!(config.revision.as_deref(), Some("main"));
        assert!(config.backend.is_none());
    }

    #[test]
    fn rejects_revision_without_repository() {
        let error = CatalogConfig::from_vfs_toml(
            r#"
[catalog]
backend = "warehouse"
revision = "main"
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("revision requires repository"));
    }
}
