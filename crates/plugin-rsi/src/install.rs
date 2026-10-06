use std::path::{Path, PathBuf};

use crate::{Error, Result, request::atomic_toml};

/// An immutable GitHub installation source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitInstalledPluginSource {
    pub remote: String,
    pub commit: String,
}

/// A daemon-owned immutable local snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalInstalledPluginSource {
    pub path: PathBuf,
    pub commit: String,
    pub digest: String,
}

/// One plugin's immutable runtime source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstalledPluginSource {
    Git(GitInstalledPluginSource),
    Local(LocalInstalledPluginSource),
}

impl InstalledPluginSource {
    pub fn commit(&self) -> &str {
        match self {
            Self::Git(source) => &source.commit,
            Self::Local(source) => &source.commit,
        }
    }
}

pub fn write_git_plugin_source(
    config_path: &Path,
    plugin_name: &str,
    git_url: &str,
    commit: &str,
) -> Result<()> {
    crate::validate_plugin_name(plugin_name)?;
    validate_github_remote(git_url)?;
    validate_commit(commit)?;
    mutate_plugin_entry(config_path, plugin_name, |table| {
        reject_conflicting_source(table, plugin_name, false)?;
        table.insert("git".into(), toml::Value::String(git_url.to_string()));
        table.insert("rev".into(), toml::Value::String(commit.to_string()));
        for key in ["path", "local_commit", "local_digest"] {
            table.remove(key);
        }
        Ok(())
    })
}

pub fn write_local_plugin_source(
    config_path: &Path,
    plugin_name: &str,
    snapshot: &Path,
    commit: &str,
    digest: &str,
) -> Result<()> {
    crate::validate_plugin_name(plugin_name)?;
    validate_commit(commit)?;
    if !digest.starts_with("sha256:")
        || digest.len() != "sha256:".len() + 64
        || !digest["sha256:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(Error::Validation(
            "local plugin digest must be `sha256:` followed by 64 hex characters".into(),
        ));
    }
    if !snapshot.is_absolute() || !snapshot.is_dir() {
        return Err(Error::Validation(
            "local plugin snapshot must be an absolute directory".into(),
        ));
    }
    mutate_plugin_entry(config_path, plugin_name, |table| {
        reject_conflicting_source(table, plugin_name, true)?;
        table.insert(
            "path".into(),
            toml::Value::String(snapshot.to_string_lossy().into_owned()),
        );
        table.insert(
            "local_commit".into(),
            toml::Value::String(commit.to_string()),
        );
        table.insert(
            "local_digest".into(),
            toml::Value::String(digest.to_string()),
        );
        for key in ["git", "rev"] {
            table.remove(key);
        }
        Ok(())
    })
}

pub fn read_installed_plugin_source(
    config_path: &Path,
    plugin_name: &str,
) -> Result<InstalledPluginSource> {
    crate::validate_plugin_name(plugin_name)?;
    let table = find_plugin_entry(config_path, plugin_name)?;
    if let Some(remote) = table.get("git").and_then(toml::Value::as_str) {
        validate_github_remote(remote)?;
        let commit = required_str(&table, "rev")?;
        validate_commit(&commit)?;
        return Ok(InstalledPluginSource::Git(GitInstalledPluginSource {
            remote: remote.to_string(),
            commit,
        }));
    }

    let path = PathBuf::from(required_str(&table, "path")?);
    let commit = required_str(&table, "local_commit")?;
    let digest = required_str(&table, "local_digest")?;
    validate_commit(&commit)?;
    if !path.is_absolute() || !path.is_dir() {
        return Err(Error::Validation(
            "local plugin snapshot must be an absolute directory".into(),
        ));
    }
    Ok(InstalledPluginSource::Local(LocalInstalledPluginSource {
        path,
        commit,
        digest,
    }))
}

/// Backward-compatible reader for callers that require a GitHub source.
pub fn read_git_plugin_source(
    config_path: &Path,
    plugin_name: &str,
) -> Result<GitInstalledPluginSource> {
    match read_installed_plugin_source(config_path, plugin_name)? {
        InstalledPluginSource::Git(source) => Ok(source),
        InstalledPluginSource::Local(_) => Err(Error::ConflictingPluginSource {
            name: plugin_name.to_string(),
        }),
    }
}

fn mutate_plugin_entry(
    config_path: &Path,
    plugin_name: &str,
    mutate: impl FnOnce(&mut toml::Table) -> Result<()>,
) -> Result<()> {
    let mut config = read_config(config_path)?;
    let Some(root) = config.as_table_mut() else {
        return Err(Error::Validation(
            "plugins.toml root must be a table".into(),
        ));
    };
    let plugins = root
        .entry("plugin".to_string())
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    let Some(plugins) = plugins.as_array_mut() else {
        return Err(Error::Validation("`plugin` must be an array".into()));
    };

    for entry in plugins.iter_mut() {
        let Some(table) = entry.as_table_mut() else {
            return Err(Error::Validation("plugin entry must be a table".into()));
        };
        if table.get("name").and_then(toml::Value::as_str) != Some(plugin_name) {
            continue;
        }
        mutate(table)?;
        return atomic_toml(config_path, &config);
    }

    let mut table = toml::Table::new();
    table.insert("name".into(), toml::Value::String(plugin_name.to_string()));
    mutate(&mut table)?;
    plugins.push(toml::Value::Table(table));
    atomic_toml(config_path, &config)
}

fn find_plugin_entry(config_path: &Path, plugin_name: &str) -> Result<toml::Table> {
    let config = read_config(config_path)?;
    let entries = config
        .get("plugin")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| Error::Validation("`plugin` must be an array".into()))?;
    let mut matched = None;
    for entry in entries {
        let table = entry
            .as_table()
            .ok_or_else(|| Error::Validation("plugin entry must be a table".into()))?;
        if table.get("name").and_then(toml::Value::as_str) != Some(plugin_name) {
            continue;
        }
        if matched.is_some() {
            return Err(Error::Validation(format!(
                "plugin `{plugin_name}` is declared more than once"
            )));
        }
        matched = Some(table.clone());
    }
    matched.ok_or_else(|| Error::Validation(format!("plugin `{plugin_name}` is not installed")))
}

fn read_config(config_path: &Path) -> Result<toml::Value> {
    let text = match std::fs::read_to_string(config_path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(Error::ReadFile {
                path: config_path.to_path_buf(),
                source,
            });
        }
    };
    if text.trim().is_empty() {
        return Ok(toml::Value::Table(Default::default()));
    }
    toml::from_str(&text).map_err(|source| Error::ParseToml {
        path: config_path.to_path_buf(),
        source,
    })
}

fn reject_conflicting_source(
    table: &toml::Table,
    plugin_name: &str,
    writing_local: bool,
) -> Result<()> {
    let has_other = if writing_local {
        table.contains_key("git") || table.contains_key("rev")
    } else {
        table.contains_key("path")
            && !(table.contains_key("local_commit") && table.contains_key("local_digest"))
    };
    if has_other {
        Err(Error::ConflictingPluginSource {
            name: plugin_name.to_string(),
        })
    } else {
        Ok(())
    }
}

fn required_str(table: &toml::Table, key: &str) -> Result<String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Validation(format!("installed plugin has no `{key}` source field")))
}

fn validate_commit(commit: &str) -> Result<()> {
    if commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(Error::Validation(
            "plugin rev must be a full 40-hex commit SHA".into(),
        ))
    }
}

fn validate_github_remote(url: &str) -> Result<()> {
    if url.starts_with("https://github.com/") || url.starts_with("git@github.com:") {
        Ok(())
    } else {
        Err(Error::Validation(
            "RSI plugin source must be a GitHub repository".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomically_adds_and_updates_git_source() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("plugins.toml");
        write_git_plugin_source(
            &path,
            "demo-plugin",
            "git@github.com:org/demo-plugin.git",
            "0123456789abcdef0123456789abcdef01234567",
        )
        .unwrap();
        write_git_plugin_source(
            &path,
            "demo-plugin",
            "git@github.com:org/demo-plugin.git",
            "1234567890abcdef1234567890abcdef12345678",
        )
        .unwrap();
        assert!(matches!(
            read_installed_plugin_source(&path, "demo-plugin").unwrap(),
            InstalledPluginSource::Git(_)
        ));
    }

    #[test]
    fn writes_and_reads_local_snapshots() {
        let tmp = tempfile::tempdir().unwrap();
        let snapshot = tempfile::tempdir().unwrap();
        let path = tmp.path().join("plugins.toml");
        write_local_plugin_source(
            &path,
            "demo-plugin",
            snapshot.path(),
            "0123456789abcdef0123456789abcdef01234567",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap();
        assert!(matches!(
            read_installed_plugin_source(&path, "demo-plugin").unwrap(),
            InstalledPluginSource::Local(_)
        ));
        assert!(read_git_plugin_source(&path, "demo-plugin").is_err());
    }
}
