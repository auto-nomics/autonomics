//! Read-only plugin management queries: what is installed under the
//! plugins root, what each family declares, and what `plugins.toml`
//! declares as installation sources.
//!
//! Deliberately tolerant where the loader is fail-closed: the loader
//! guards *startup* (one invalid manifest aborts the daemon), while
//! these queries feed management surfaces that must show the broken
//! family — with its parse error — alongside the healthy ones, because
//! on-disk drift after startup is exactly what a management endpoint
//! exists to surface.

use std::collections::BTreeMap;
use std::path::Path;

use crate::loader::MANIFEST_FILE;
use crate::manifest::PluginManifest;
use crate::sync::{PLUGIN_CONFIG_FILE, PluginSource, PluginsConfig};

/// One scanned family directory under the plugins root.
#[derive(Debug)]
pub struct FamilyScan {
    /// Directory name under the root — the install identity `plugins.toml`
    /// declares (the manifest's `plugin_name` may differ; the loader keys
    /// nothing by directory beyond installation).
    pub dir: String,
    /// Parsed manifest, or the parse failure detail for this family.
    pub manifest: Result<PluginManifest, String>,
}

/// Scan every plugin family under `root`. A missing root is "nothing
/// installed" (same semantics as the registry build); each directory
/// holding a `manifest.toml` becomes one row, parse failures included.
/// Sorted by directory name for stable output.
pub fn inspect_root(root: &Path) -> Vec<FamilyScan> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut families: Vec<FamilyScan> = entries
        .flatten()
        .filter(|entry| entry.path().join(MANIFEST_FILE).is_file())
        .map(|entry| {
            let dir = entry.file_name().to_string_lossy().into_owned();
            let manifest = parse_manifest(&entry.path().join(MANIFEST_FILE));
            FamilyScan { dir, manifest }
        })
        .collect();
    families.sort_by(|a, b| a.dir.cmp(&b.dir));
    families
}

/// Parse one family manifest into the failure-tolerant `Result` form.
fn parse_manifest(path: &Path) -> Result<PluginManifest, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|source| format!("cannot read `{}`: {source}", path.display()))?;
    toml::from_str(&text).map_err(|error| format!("parse `{}`: {error}", path.display()))
}

/// Read the declared installation sources from `plugins.toml` beside the
/// plugins root. `Ok(None)` = no config file (nothing declared); a
/// present-but-unparseable config is an error the caller surfaces —
/// unlike a single broken family, a broken declaration file corrupts
/// every future sync, not one row.
pub fn declared_sources(
    config_path: &Path,
) -> Result<Option<BTreeMap<String, PluginSource>>, String> {
    let text = match std::fs::read_to_string(config_path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(format!("cannot read `{}`: {source}", config_path.display())),
    };
    let config: PluginsConfig = toml::from_str(&text)
        .map_err(|error| format!("parse `{}`: {error}", config_path.display()))?;
    Ok(Some(
        config
            .plugin
            .into_iter()
            .map(|source| (source.name.clone(), source))
            .collect(),
    ))
}

/// The default config location for a given plugins root: the sync layer
/// reads `<state_dir>/plugins.toml` and materializes into
/// `<state_dir>/plugins`, so the config sits beside the root.
pub fn default_config_path(root: &Path) -> std::path::PathBuf {
    root.parent()
        .unwrap_or_else(|| Path::new(""))
        .join(PLUGIN_CONFIG_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_family(root: &Path, name: &str, manifest_body: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST_FILE), manifest_body).unwrap();
    }

    const GOOD_LDSC: &str = r#"
schema_version = 1
plugin_name = "ldsc"

[image]
reference = "ghcr.io/auto-nomics/autonomics/ldsc@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c"

[[nodes]]
kind = "ldsc_h2"
desc = "d"
doc = "doc"
timeout_secs = 3600

[nodes.ports]
inputs = [{ type = "file" }]
outputs = [{ path = "ldsc_h2.log", format = "ldsc_log" }]

[nodes.command]
interpreter = "sh"
script = "true"
"#;

    #[test]
    fn missing_root_is_empty() {
        assert!(inspect_root(Path::new("/nonexistent-autonomics-plugins")).is_empty());
    }

    #[test]
    fn inspect_reports_broken_families_alongside_healthy_ones() {
        let root = tempfile::tempdir().unwrap();
        write_family(root.path(), "ldsc", GOOD_LDSC);
        write_family(root.path(), "broken", "not even toml {{{");
        // A directory without a manifest is not a plugin family.
        std::fs::create_dir_all(root.path().join("not-a-plugin")).unwrap();

        let families = inspect_root(root.path());
        assert_eq!(families.len(), 2, "healthy + broken, unrelated dir skipped");
        assert_eq!(families[0].dir, "broken");
        assert!(families[0].manifest.is_err());
        assert_eq!(families[1].dir, "ldsc");
        let manifest = families[1].manifest.as_ref().unwrap();
        assert_eq!(manifest.plugin_name, "ldsc");
        assert_eq!(
            manifest
                .nodes
                .iter()
                .map(|n| n.kind.clone())
                .collect::<Vec<_>>(),
            vec!["ldsc_h2".to_string()]
        );
    }

    #[test]
    fn declared_sources_missing_config_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            declared_sources(&dir.path().join(PLUGIN_CONFIG_FILE))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn declared_sources_parses_into_map() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(PLUGIN_CONFIG_FILE),
            r#"
[[plugin]]
name = "ldsc"
git = "https://example.com/ldsc-plugin"
rev = "0123456789abcdef0123456789abcdef01234567"

[[plugin]]
name = "local-dev"
path = "/home/dev/plugins/local-dev"
"#,
        )
        .unwrap();

        let declared = declared_sources(&dir.path().join(PLUGIN_CONFIG_FILE))
            .unwrap()
            .expect("config present");
        assert_eq!(declared.len(), 2);
        assert_eq!(
            declared["ldsc"].git.as_deref(),
            Some("https://example.com/ldsc-plugin")
        );
        assert_eq!(
            declared["local-dev"]
                .path
                .as_ref()
                .map(|p| p.display().to_string()),
            Some("/home/dev/plugins/local-dev".to_string())
        );
    }

    #[test]
    fn default_config_path_sits_beside_the_root() {
        let config = default_config_path(Path::new("/home/wjx/.autonomics/plugins"));
        assert_eq!(config, Path::new("/home/wjx/.autonomics/plugins.toml"));
    }
}
