use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogConfig {
    /// Hugging Face registry repository in `owner/name` form.
    #[serde(default)]
    pub repository: Option<String>,
    /// Prefix for per-package Hugging Face repositories in `owner/name` form.
    #[serde(default)]
    pub repository_prefix: Option<String>,
    /// Hugging Face revision; defaults to `main`.
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Expose the local catalog cache under `/catalog` for agent inspection.
    #[serde(default = "default_true")]
    pub agent_visible: bool,
}

fn default_true() -> bool {
    true
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            repository: None,
            repository_prefix: None,
            revision: None,
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

    /// Parse the `[catalog]` section out of a full `vfs.toml` document.
    ///
    /// `Ok(None)` when the document declares no catalog section (catalog
    /// absent for the deployment); a present-but-invalid section is an
    /// error, matching [`Self::from_vfs_toml`]. Unknown sections such as
    /// `[[backend]]` and `[[mount]]` are ignored.
    pub fn from_vfs_toml_optional(source: &str) -> Result<Option<Self>> {
        let document: toml::Value =
            toml::from_str(source).map_err(|error| format!("parse catalog config: {error}"))?;
        let Some(section) = document.get("catalog") else {
            return Ok(None);
        };
        let config: CatalogConfig = section
            .clone()
            .try_into()
            .map_err(|error| format!("parse catalog config: {error}"))?;
        config.validate()?;
        Ok(Some(config))
    }

    pub fn validate(&self) -> Result<()> {
        let Some(repository) = self
            .repository
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            if self.enabled {
                return Err("catalog repository is required".into());
            }
            return Ok(());
        };

        validate_repo_id(repository, "catalog repository")?;
        if let Some(prefix) = self
            .repository_prefix
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            validate_repo_id(prefix, "catalog repository prefix")?;
        }
        Ok(())
    }
}

fn validate_repo_id(value: &str, field: &str) -> Result<()> {
    let (owner, name) = value
        .split_once('/')
        .ok_or_else(|| format!("{field} must be `owner/name`, got `{value}`"))?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Err(format!("{field} must be `owner/name`, got `{value}`").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_repository_is_required() {
        assert!(
            CatalogConfig::from_vfs_toml("[[mount]]\npath=\"/\"\nbackend=\"x\"\nsource=\"/\"\n")
                .is_err()
        );

        CatalogConfig {
            repository: None,
            enabled: false,
            ..CatalogConfig::default()
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn parses_hugging_face_repository_catalog() {
        let config = CatalogConfig::from_vfs_toml(
            r#"
[catalog]
repository = "wjixiang/catalog-index"
revision = "main"
"#,
        )
        .unwrap();
        assert_eq!(config.repository.as_deref(), Some("wjixiang/catalog-index"));
        assert_eq!(config.revision.as_deref(), Some("main"));
    }

    #[test]
    fn parses_and_validates_package_repository_prefix() {
        let config = CatalogConfig::from_vfs_toml(
            r#"
[catalog]
repository = "wjixiang/catalog-index"
repository_prefix = "wjixiang/catalog"
"#,
        )
        .unwrap();
        assert_eq!(
            config.repository_prefix.as_deref(),
            Some("wjixiang/catalog")
        );

        let error = CatalogConfig::from_vfs_toml(
            r#"
[catalog]
repository = "invalid/repo/id"
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("catalog repository must be"));
    }

    #[test]
    fn optional_parse_absent_section_is_none() {
        let config = CatalogConfig::from_vfs_toml_optional(
            r#"
[[backend]]
id = "default"

[[mount]]
path = "/"
backend = "default"
"#,
        )
        .unwrap();
        assert!(config.is_none());
    }

    #[test]
    fn optional_parse_finds_catalog_among_other_sections() {
        let config = CatalogConfig::from_vfs_toml_optional(
            r#"
[[backend]]
id = "default"

[catalog]
repository = "wjixiang/catalog-index"

[[mount]]
path = "/"
backend = "default"
"#,
        )
        .unwrap();
        assert_eq!(
            config
                .expect("catalog section present")
                .repository
                .as_deref(),
            Some("wjixiang/catalog-index")
        );
    }

    #[test]
    fn optional_parse_invalid_section_is_an_error() {
        let error = CatalogConfig::from_vfs_toml_optional(
            r#"
[catalog]
repository = "invalid/repo/id"
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("catalog repository must be"));
    }
}
