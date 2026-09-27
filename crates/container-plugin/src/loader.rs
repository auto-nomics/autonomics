//! Plugin discovery and loading: turn a directory of plugin folders into
//! registered [`Plugin`]s.
//!
//! A plugin is a directory whose root contains `manifest.toml`, optionally
//! alongside `scripts/`, fixtures, and build provenance. The loader scans
//! a trusted root (admin-controlled; agents never reach host paths),
//! parses and validates every manifest fail-closed, inlines
//! `script_file` contents, and constructs one [`Plugin`] per family.
//!
//! Deliberately offline: network distribution (git sources, sync) is an
//! M5 concern layered *on top* of this module.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::factory::Plugin;
use dag_core::NodePlugin;
use crate::manifest::PluginManifest;

pub const MANIFEST_FILE: &str = "manifest.toml";
pub const PLUGIN_ROOT_ENV: &str = "AUTONOMICS_PLUGIN_ROOT";

/// Where plugins live when the environment does not override: under the
/// autonomics state home (`~/.autonomics/plugins`), next to the agent DB
/// and vfs.toml. The engine wiring treats a missing root as "no plugins
/// installed" and an existing root with invalid manifests as a startup
/// failure.
pub fn default_plugins_root() -> PathBuf {
    if let Some(root) = std::env::var_os(PLUGIN_ROOT_ENV) {
        return PathBuf::from(root);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".autonomics/plugins")
}

pub type Result<T> = std::result::Result<T, Error>;

/// Failures while turning a plugins directory into [`Plugin`]s. Every
/// variant names the plugin (and entry, when relevant) so the startup
/// error points at the exact file to fix.
#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot read plugins root `{}`: {source}", root.display())]
    ReadRoot {
        root: PathBuf,
        source: std::io::Error,
    },

    #[error("cannot read `{path}`: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("cannot parse `{path}`: {detail}")]
    Parse { path: PathBuf, detail: String },

    #[error("plugin `{plugin}`: {message}")]
    Invalid { plugin: String, message: String },

    #[error("kind `{kind}` is declared by both `{first}` and `{second}`")]
    DuplicateKind {
        kind: String,
        first: String,
        second: String,
    },
}

/// Load every plugin found under `root`. Each direct subdirectory that
/// contains a `manifest.toml` becomes one plugin; anything else is
/// skipped silently (the root may hold unrelated state). Fails closed:
/// one invalid plugin aborts the whole load, naming the offender.
pub fn load(
    root: &Path,
    runtime: Arc<dyn container_runtime::PodmanConnection>,
    panel_cache: Arc<container_runtime::PanelCache>,
) -> Result<Vec<Plugin>> {
    let entries = std::fs::read_dir(root).map_err(|source| Error::ReadRoot {
        root: root.to_path_buf(),
        source,
    })?;

    let mut plugins = Vec::new();
    let mut kind_owner: BTreeMap<String, String> = BTreeMap::new();

    for entry in entries.flatten() {
        let dir = entry.path();
        let manifest_path = dir.join(MANIFEST_FILE);
        if !dir.is_dir() || !manifest_path.is_file() {
            continue;
        }
        let plugin = load_one(&dir, &manifest_path, runtime.clone(), panel_cache.clone())?;
        // Cross-family kind uniqueness within the loaded set. Kinds that
        // collide with registry factories outside this loader surface at
        // registration time (registry last-write-wins today); the M5
        // migration protocol removes the nodes-io line in the same commit
        // that adds the manifest.
        for kind in plugin.registered_kinds() {
            if let Some(first) = kind_owner.get(kind) {
                return Err(Error::DuplicateKind {
                    kind: kind.to_string(),
                    first: first.clone(),
                    second: plugin.name().to_string(),
                });
            }
            kind_owner.insert(kind.to_string(), plugin.name().to_string());
        }
        plugins.push(plugin);
    }

    Ok(plugins)
}

/// Load one plugin directory: read, parse, inline script files, validate
/// (family-level plus per-node), construct.
fn load_one(
    dir: &Path,
    manifest_path: &Path,
    runtime: Arc<dyn container_runtime::PodmanConnection>,
    panel_cache: Arc<container_runtime::PanelCache>,
) -> Result<Plugin> {
    let text = std::fs::read_to_string(manifest_path).map_err(|source| Error::ReadFile {
        path: manifest_path.to_path_buf(),
        source,
    })?;
    let mut manifest: PluginManifest = toml::from_str(&text).map_err(|error| Error::Parse {
        path: manifest_path.to_path_buf(),
        detail: error.to_string(),
    })?;

    let plugin = manifest.plugin_name.clone();
    let invalid = |message: String| Error::Invalid {
        plugin: plugin.clone(),
        message,
    };

    // Family-level checks.
    if manifest.plugin_name.trim().is_empty() {
        return Err(invalid("plugin_name cannot be empty".into()));
    }
    let mut mounts = BTreeSet::new();
    let mut bindings = BTreeSet::new();
    for panel in &manifest.panels {
        if !mounts.insert(panel.mount.clone()) {
            return Err(invalid(format!("duplicate panel mount `{}`", panel.mount)));
        }
        if !bindings.insert(panel.binding.clone()) {
            return Err(invalid(format!(
                "duplicate panel binding `{}`",
                panel.binding
            )));
        }
    }

    // Inline script_file references before any validation sees them: from
    // here on, `script` is the single source of script truth.
    for (index, node) in manifest.nodes.iter_mut().enumerate() {
        match (&node.command.script, &node.command.script_file) {
            (Some(_), Some(_)) => {
                return Err(invalid(format!(
                    "nodes[{index}] declares both `script` and `script_file`"
                )));
            }
            (None, Some(relative)) => {
                let source = read_script_file(dir, relative).map_err(|message| {
                    invalid(format!("nodes[{index}] script_file: {message}"))
                })?;
                node.command.script = Some(source);
                node.command.script_file = None;
            }
            _ => {}
        }
    }

    // Per-node checks (kind syntax, outputs, template closure, requires).
    for node in &manifest.nodes {
        crate::node_definition::validate(node)
            .map_err(|message| invalid(format!("node `{}`: {message}", node.kind)))?;
    }

    Ok(Plugin::new(manifest, runtime, panel_cache))
}

/// Read a script file referenced relative to the plugin root, rejecting
/// traversal or absolute paths: the plugin directory is the trust
/// boundary for what may be inlined into a container command.
fn read_script_file(plugin_root: &Path, relative: &str) -> std::result::Result<String, String> {
    if relative.is_empty() || relative.contains('\0') {
        return Err("path cannot be empty".into());
    }
    let candidate = Path::new(relative);
    if candidate.is_absolute()
        || !candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "`{relative}` must be a safe relative path inside the plugin directory"
        ));
    }
    std::fs::read_to_string(plugin_root.join(candidate))
        .map_err(|source| format!("cannot read `{relative}`: {source}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use container_runtime::PanelCache;

    fn infra() -> (
        Arc<dyn container_runtime::PodmanConnection>,
        Arc<PanelCache>,
        tempfile::TempDir,
    ) {
        let state = tempfile::tempdir().unwrap();
        let runtime: Arc<dyn container_runtime::PodmanConnection> =
            Arc::new(container_runtime::PodmanRuntime::new(
                container_runtime::PodmanConfig {
                    program: "podman".into(),
                    workspace_root: state.path().join("workspace"),
                    panel_cache_root: state.path().join("panels"),
                },
            ));
        let cache = Arc::new(PanelCache::new(state.path().join("panels")));
        (runtime, cache, state)
    }

    fn write_plugin(root: &Path, name: &str, manifest_body: &str) {
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
kind = "ldsc_h2_container"
desc = "d"
doc = "doc"
timeout_secs = 3600

[nodes.ports]
inputs = [{ type = "file" }]
outputs = [{ path = "ldsc_h2.log", format = "ldsc_log" }]

[nodes.command]
interpreter = "sh"
script_file = "scripts/h2.sh"
"#;

    #[test]
    fn loads_a_directory_of_plugins_and_inlines_script_files() {
        let (runtime, cache, state) = infra();
        let plugins_root = state.path().join("plugins");
        let ldsc_dir = plugins_root.join("ldsc");
        std::fs::create_dir_all(ldsc_dir.join("scripts")).unwrap();
        std::fs::write(ldsc_dir.join(MANIFEST_FILE), GOOD_LDSC).unwrap();
        std::fs::write(
            ldsc_dir.join("scripts/h2.sh"),
            "set -eu\nldsc --h2 \"$AUTONOMICS_INPUT0\" > \"$AUTONOMICS_OUTPUT0\" 2>&1\n",
        )
        .unwrap();
        // An unrelated directory without a manifest is skipped silently.
        std::fs::create_dir_all(plugins_root.join("not-a-plugin")).unwrap();

        let plugins = load(&plugins_root, runtime, cache).unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name(), "ldsc");
        assert_eq!(
            plugins[0].registered_kinds(),
            vec!["ldsc_h2_container"],
            "script_file contents are inlined before validation, so the \
             template-closure check runs over the real script"
        );
    }

    #[test]
    fn script_file_traversal_is_rejected() {
        let (runtime, cache, state) = infra();
        let plugins_root = state.path().join("plugins");
        write_plugin(
            &plugins_root,
            "evil",
            &GOOD_LDSC
                .replace("plugin_name = \"ldsc\"", "plugin_name = \"evil\"")
                .replace("scripts/h2.sh", "../escape.sh"),
        );

        let error = load(&plugins_root, runtime, cache).unwrap_err();
        assert!(error.to_string().contains("safe relative path"), "{error}");
    }

    #[test]
    fn script_and_script_file_are_mutually_exclusive() {
        let (runtime, cache, state) = infra();
        let plugins_root = state.path().join("plugins");
        let both = format!(
            "{}\nscript = \"inline\"\n",
            GOOD_LDSC.replace("script_file = \"scripts/h2.sh\"", "script_file = \"scripts/h2.sh\"")
        );
        write_plugin(
            &plugins_root,
            "both",
            &both.replace("plugin_name = \"ldsc\"", "plugin_name = \"both\""),
        );

        let error = load(&plugins_root, runtime, cache).unwrap_err();
        assert!(error.to_string().contains("both `script` and `script_file`"), "{error}");
    }

    #[test]
    fn missing_script_file_is_rejected() {
        let (runtime, cache, state) = infra();
        let plugins_root = state.path().join("plugins");
        // GOOD_LDSC references scripts/h2.sh but the file is never written.
        write_plugin(&plugins_root, "ldsc", GOOD_LDSC);

        let error = load(&plugins_root, runtime, cache).unwrap_err();
        assert!(error.to_string().contains("cannot read `scripts/h2.sh`"), "{error}");
    }

    #[test]
    fn duplicate_kind_across_families_is_rejected() {
        let (runtime, cache, state) = infra();
        let plugins_root = state.path().join("plugins");
        write_plugin(&plugins_root, "ldsc", GOOD_LDSC);
        write_plugin(
            &plugins_root,
            "ldsc-fork",
            &GOOD_LDSC.replace("plugin_name = \"ldsc\"", "plugin_name = \"ldsc-fork\""),
        );
        // Both manifests reference scripts/h2.sh; only with the file in
        // place do both survive to the duplicate-kind check.
        for name in ["ldsc", "ldsc-fork"] {
            let scripts = plugins_root.join(name).join("scripts");
            std::fs::create_dir_all(&scripts).unwrap();
            std::fs::write(
                scripts.join("h2.sh"),
                "set -eu\nldsc --h2 \"$AUTONOMICS_INPUT0\" > \"$AUTONOMICS_OUTPUT0\" 2>&1\n",
            )
            .unwrap();
        }

        let error = load(&plugins_root, runtime, cache).unwrap_err();
        assert!(error.to_string().contains("ldsc_h2_container"), "{error}");
        assert!(error.to_string().contains("ldsc-fork"), "{error}");
    }

    #[test]
    fn undeclared_template_ref_fails_at_load_not_inside_the_container() {
        let (runtime, cache, state) = infra();
        let plugins_root = state.path().join("plugins");
        let dir = plugins_root.join("ldsc");
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join(MANIFEST_FILE), GOOD_LDSC).unwrap();
        // The referenced script uses an undeclared param: the closure check
        // must fire over the *inlined* script.
        std::fs::write(
            dir.join("scripts/h2.sh"),
            "set -eu\nldsc --flag {{ undeclared }} > \"$AUTONOMICS_OUTPUT0\" 2>&1\n",
        )
        .unwrap();

        let error = load(&plugins_root, runtime, cache).unwrap_err();
        assert!(error.to_string().contains("undeclared param"), "{error}");
    }
}
