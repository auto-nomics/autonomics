use clap::Args;
use container_plugin::{
    error::{Error, Result},
    manifest::PluginManifest,
};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Name of the new plugin
    #[arg(long, short)]
    pub plugin_name: String,
    /// Target directory for the new manifest family.
    #[arg(long, short)]
    pub dir_path: Option<PathBuf>,
}

pub fn init_plugin_project(plugin_name: &str, path: Option<PathBuf>) -> Result<()> {
    // The name becomes both a path segment and the image repo: reject
    // traversal, absolute paths, and casing before either is used.
    validate_plugin_name(plugin_name)?;

    let base = match path {
        Some(path) => path,
        None => std::env::current_dir().map_err(|source| Error::CurrentDir { source })?,
    };
    let project_path = base.join(plugin_name);

    // Fail closed instead of merging into an existing directory.
    if project_path.exists() {
        return Err(Error::AlreadyExists { path: project_path });
    }
    fs::create_dir_all(&project_path).map_err(|source| Error::CreateDir {
        path: project_path.clone(),
        source,
    })?;

    let manifest = PluginManifest::default();
    let manifest_text = toml::to_string_pretty(&manifest)?;
    let manifest_path = project_path.join("manifest.toml");
    fs::write(&manifest_path, manifest_text).map_err(|source| Error::WriteFile {
        path: manifest_path.clone(),
        source,
    })?;

    Ok(())
}

/// A plugin name is valid when it is lowercase kebab-case: starts with a
/// lowercase letter, ends with a lowercase letter or digit, and contains
/// only `[a-z0-9-]` in between.
fn validate_plugin_name(name: &str) -> Result<()> {
    let valid = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidName {
            name: name.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_lowercase_kebab_names() {
        assert!(validate_plugin_name("demo-tool").is_ok());
        assert!(validate_plugin_name("mtag").is_ok());
        assert!(validate_plugin_name("hdl-l").is_ok());
        assert!(validate_plugin_name("tool2").is_ok());
    }

    #[test]
    fn rejects_traversal_absolute_and_cased_names() {
        for bad in ["../evil", "/tmp/evil", "", "Demo-Tool", "demo_tool", "-demo", "demo-"] {
            assert!(validate_plugin_name(bad).is_err(), "`{bad}` must be rejected");
        }
    }
}
