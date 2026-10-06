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
use crate::manifest::PluginManifest;
use dag_core::NodePlugin;

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
                let source = read_script_file(dir, relative)
                    .map_err(|message| invalid(format!("nodes[{index}] script_file: {message}")))?;
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

    // The exact manifest bytes join every node's plugin identity (WO-R09):
    // any edit to the family definition invalidates cached outputs of its
    // nodes. `text` is the file read verbatim, so this is byte-level.
    Ok(Plugin::new(manifest, text.as_bytes(), runtime, panel_cache))
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
        let runtime: Arc<dyn container_runtime::PodmanConnection> = Arc::new(
            container_runtime::PodmanRuntime::new(container_runtime::PodmanConfig {
                program: "podman".into(),
                workspace_root: state.path().join("workspace"),
                panel_cache_root: state.path().join("panels"),
            }),
        );
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
kind = "ldsc_h2"
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
            vec!["ldsc_h2"],
            "script_file contents are inlined before validation, so the \
             template-closure check runs over the real script"
        );
    }

    // ── plugin identity (WO-R09) ─────────────────────────────────────────
    //
    // Expected hashes are constructed independently in each test: sha256 is
    // computed straight over bytes read from the fixture files with
    // `sha2::Sha256`, never through the implementation's own hashing path.

    use sha2::{Digest, Sha256};

    fn independent_sha256(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!("sha256:{hex}")
    }

    /// Write the GOOD_LDSC fixture with `script` content and load it,
    /// returning the loaded plugin.
    fn load_ldsc(
        root: &std::path::Path,
        manifest_body: &str,
        script_body: &str,
    ) -> Plugin {
        let (runtime, cache, _state) = infra();
        let dir = root.join("ldsc");
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join(MANIFEST_FILE), manifest_body).unwrap();
        std::fs::write(dir.join("scripts/h2.sh"), script_body).unwrap();
        let mut plugins = load(root, runtime, cache).unwrap();
        assert_eq!(plugins.len(), 1);
        plugins.remove(0)
    }

    const H2_SCRIPT: &str =
        "set -eu\nldsc --h2 \"$AUTONOMICS_INPUT0\" > \"$AUTONOMICS_OUTPUT0\" 2>&1\n";

    /// Acceptance: the manifest hash is over the exact file bytes — one
    /// character (even a semantically inert comment) changes the identity —
    /// and loading the same directory twice is stable.
    #[test]
    fn plugin_identity_hashes_manifest_bytes_exactly() {
        let state = tempfile::tempdir().unwrap();
        let root = state.path().join("plugins");

        let first = load_ldsc(&root, GOOD_LDSC, H2_SCRIPT);
        let identity = first.plugin_identity("ldsc_h2").expect("kind is declared");

        // Golden constructed independently from the file on disk.
        let manifest_bytes = std::fs::read(root.join("ldsc").join(MANIFEST_FILE)).unwrap();
        assert_eq!(
            identity.manifest_sha256,
            independent_sha256(&manifest_bytes),
            "manifest hash must be sha256 over the exact manifest.toml bytes"
        );
        assert_eq!(
            identity.script_sha256.as_deref(),
            Some(independent_sha256(H2_SCRIPT.as_bytes()).as_str()),
            "script hash must be sha256 over the exact script_file content"
        );
        assert_eq!(
            identity.image_reference,
            "ghcr.io/auto-nomics/autonomics/ldsc@sha256:2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c"
        );
        assert!(identity.panels.is_empty(), "GOOD_LDSC declares no panels");

        // One character appended — a comment, invisible to the TOML parser —
        // must still change the manifest identity.
        let edited = format!("{GOOD_LDSC}\n# c");
        let second = load_ldsc(&root, &edited, H2_SCRIPT);
        let edited_identity = second.plugin_identity("ldsc_h2").unwrap();
        assert_ne!(
            identity.manifest_sha256, edited_identity.manifest_sha256,
            "a one-character manifest edit must change the identity"
        );
        // Everything the edit did not touch stays identical — the change is
        // attributable, not a wholesale reshuffle.
        assert_eq!(identity.script_sha256, edited_identity.script_sha256);
        assert_eq!(identity.image_reference, edited_identity.image_reference);

        // Stability: loading the same bytes again reproduces the identity.
        let third = load_ldsc(&root, &edited, H2_SCRIPT);
        assert_eq!(edited_identity, third.plugin_identity("ldsc_h2").unwrap());
    }

    /// Acceptance: a different image digest in `[image]` changes the
    /// identity's image reference (and, being a manifest edit, the manifest
    /// hash too).
    #[test]
    fn plugin_identity_tracks_the_image_digest() {
        let state = tempfile::tempdir().unwrap();
        let root = state.path().join("plugins");

        let first = load_ldsc(&root, GOOD_LDSC, H2_SCRIPT);
        let old = first.plugin_identity("ldsc_h2").unwrap();

        let edited = GOOD_LDSC.replace(
            "2dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c",
            "3dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c",
        );
        let second = load_ldsc(&root, &edited, H2_SCRIPT);
        let new = second.plugin_identity("ldsc_h2").unwrap();

        assert_ne!(old.image_reference, new.image_reference);
        assert!(new.image_reference.ends_with(
            "@sha256:3dad70a9583f93db1dcc9a560b7d5b309af4a5151dfaf615f80d059a0925d78c"
        ));
        assert_ne!(old.manifest_sha256, new.manifest_sha256);
        // The script did not change.
        assert_eq!(old.script_sha256, new.script_sha256);
    }

    /// Acceptance: editing only the referenced script file changes the
    /// script identity while the manifest bytes — and their hash — stay
    /// identical. This is exactly the "same fingerprint, different plugin
    /// implementation" hole WO-R09 closes: the script lives outside
    /// manifest.toml, so the manifest hash alone cannot see this edit.
    #[test]
    fn plugin_identity_tracks_script_file_content() {
        let state = tempfile::tempdir().unwrap();
        let root = state.path().join("plugins");

        let first = load_ldsc(&root, GOOD_LDSC, H2_SCRIPT);
        let old = first.plugin_identity("ldsc_h2").unwrap();

        let edited_script = H2_SCRIPT.replace("ldsc --h2", "ldsc --h2 --yes");
        let second = load_ldsc(&root, GOOD_LDSC, &edited_script);
        let new = second.plugin_identity("ldsc_h2").unwrap();

        assert_eq!(
            old.manifest_sha256, new.manifest_sha256,
            "manifest bytes are untouched by a script-file edit"
        );
        assert_ne!(
            old.script_sha256, new.script_sha256,
            "a script_file edit must change the identity"
        );
        assert_eq!(
            new.script_sha256.as_deref(),
            Some(independent_sha256(edited_script.as_bytes()).as_str())
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
        let both = format!("{}\nscript = \"inline\"\n", GOOD_LDSC);
        write_plugin(
            &plugins_root,
            "both",
            &both.replace("plugin_name = \"ldsc\"", "plugin_name = \"both\""),
        );

        let error = load(&plugins_root, runtime, cache).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("both `script` and `script_file`"),
            "{error}"
        );
    }

    #[test]
    fn missing_script_file_is_rejected() {
        let (runtime, cache, state) = infra();
        let plugins_root = state.path().join("plugins");
        // GOOD_LDSC references scripts/h2.sh but the file is never written.
        write_plugin(&plugins_root, "ldsc", GOOD_LDSC);

        let error = load(&plugins_root, runtime, cache).unwrap_err();
        assert!(
            error.to_string().contains("cannot read `scripts/h2.sh`"),
            "{error}"
        );
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
        assert!(error.to_string().contains("ldsc_h2"), "{error}");
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
