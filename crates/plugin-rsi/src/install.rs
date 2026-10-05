use std::path::Path;

use crate::{Error, Result, request::atomic_toml};

/// The immutable git source declared for one installed plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPluginSource {
    /// GitHub repository declared by `plugins.toml`.
    pub remote: String,
    /// Full commit SHA currently installed.
    pub commit: String,
}

pub fn write_git_plugin_source(
    config_path: &Path,
    plugin_name: &str,
    git_url: &str,
    commit: &str,
) -> Result<()> {
    crate::validate_plugin_name(plugin_name)?;
    validate_github_remote(git_url)?;
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Validation(
            "plugin rev must be a full 40-hex commit SHA".into(),
        ));
    }

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
    let mut config: toml::Value = if text.trim().is_empty() {
        toml::Value::Table(Default::default())
    } else {
        toml::from_str::<toml::Value>(&text).map_err(|source| Error::ParseToml {
            path: config_path.to_path_buf(),
            source,
        })?
    };
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

    let mut updated = false;
    for entry in plugins.iter_mut() {
        let Some(table) = entry.as_table_mut() else {
            return Err(Error::Validation("plugin entry must be a table".into()));
        };
        if table.get("name").and_then(toml::Value::as_str) != Some(plugin_name) {
            continue;
        }
        let existing_git = table.get("git").and_then(toml::Value::as_str);
        if table.contains_key("path") || (existing_git.is_some() && existing_git != Some(git_url)) {
            return Err(Error::ConflictingPluginSource {
                name: plugin_name.to_string(),
            });
        }
        table.insert("git".into(), toml::Value::String(git_url.to_string()));
        table.insert("rev".into(), toml::Value::String(commit.to_string()));
        table.remove("path");
        updated = true;
        break;
    }
    if !updated {
        let mut table = toml::Table::new();
        table.insert("name".into(), toml::Value::String(plugin_name.to_string()));
        table.insert("git".into(), toml::Value::String(git_url.to_string()));
        table.insert("rev".into(), toml::Value::String(commit.to_string()));
        plugins.push(toml::Value::Table(table));
    }

    atomic_toml(config_path, &config)
}

/// Read one installed plugin's pinned git source from `plugins.toml`.
///
/// Updates are intentionally restricted to GitHub git sources. Local path
/// installations belong to human-driven development and have no immutable
/// baseline that this lifecycle can audit.
pub fn read_git_plugin_source(
    config_path: &Path,
    plugin_name: &str,
) -> Result<InstalledPluginSource> {
    crate::validate_plugin_name(plugin_name)?;
    let text = std::fs::read_to_string(config_path).map_err(|source| Error::ReadFile {
        path: config_path.to_path_buf(),
        source,
    })?;
    let config: toml::Value = toml::from_str(&text).map_err(|source| Error::ParseToml {
        path: config_path.to_path_buf(),
        source,
    })?;
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
        let remote = table
            .get("git")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| Error::Validation("installed plugin has no git source".into()))?;
        let commit = table
            .get("rev")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| Error::Validation("installed plugin has no pinned rev".into()))?;
        validate_github_remote(remote)?;
        if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::Validation(
                "plugin rev must be a full 40-hex commit SHA".into(),
            ));
        }
        if table.contains_key("path") {
            return Err(Error::ConflictingPluginSource {
                name: plugin_name.to_string(),
            });
        }
        matched = Some(InstalledPluginSource {
            remote: remote.to_string(),
            commit: commit.to_string(),
        });
    }
    matched.ok_or_else(|| Error::Validation(format!("plugin `{plugin_name}` is not installed")))
}

fn validate_github_remote(url: &str) -> Result<()> {
    let valid = url.starts_with("https://github.com/") || url.starts_with("git@github.com:");
    if valid {
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
        let sha = "0123456789abcdef0123456789abcdef01234567";
        write_git_plugin_source(
            &path,
            "demo-plugin",
            "git@github.com:org/demo-plugin.git",
            sha,
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("rev = \"0123456789abcdef0123456789abcdef01234567\""));
        let new_sha = "1234567890abcdef1234567890abcdef12345678";
        write_git_plugin_source(
            &path,
            "demo-plugin",
            "git@github.com:org/demo-plugin.git",
            new_sha,
        )
        .unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains(sha));
    }

    #[test]
    fn refuses_mutable_or_conflicting_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("plugins.toml");
        assert!(
            write_git_plugin_source(
                &path,
                "demo",
                "git@example.com:evil.git",
                "0123456789abcdef0123456789abcdef01234567"
            )
            .is_err()
        );

        std::fs::write(
            &path,
            "[[plugin]]\nname = \"demo-plugin\"\npath = \"/tmp/demo\"\n",
        )
        .unwrap();
        assert!(
            write_git_plugin_source(
                &path,
                "demo-plugin",
                "git@github.com:org/demo.git",
                "0123456789abcdef0123456789abcdef01234567"
            )
            .is_err()
        );
    }

    #[test]
    fn reads_pinned_installed_source() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("plugins.toml");
        let sha = "0123456789abcdef0123456789abcdef01234567";
        std::fs::write(
            &path,
            format!(
                "[[plugin]]\nname = \"demo-plugin\"\ngit = \"git@github.com:org/demo-plugin.git\"\nrev = \"{sha}\"\n"
            ),
        )
        .unwrap();
        let source = read_git_plugin_source(&path, "demo-plugin").unwrap();
        assert_eq!(
            source,
            InstalledPluginSource {
                remote: "git@github.com:org/demo-plugin.git".into(),
                commit: sha.into(),
            }
        );
    }

    #[test]
    fn update_source_reader_rejects_local_plugins() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("plugins.toml");
        std::fs::write(
            &path,
            "[[plugin]]\nname = \"demo-plugin\"\npath = \"/tmp/demo-plugin\"\n",
        )
        .unwrap();
        assert!(read_git_plugin_source(&path, "demo-plugin").is_err());
    }
}
