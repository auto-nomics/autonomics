use data_catalog::model::HfRepoId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum ImageRepo {
    Docker,
    Ghcr,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ManifestDigest(String);

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRef {
    repo: ImageRepo,
    digest: ManifestDigest,
    tag: Option<String>,
    pub upstream: Option<String>,
    pub license: Option<String>,
}

impl Default for ImageRef {
    /// Default image is the 'hello-world' image from docker.io
    fn default() -> Self {
        Self {
            repo: ImageRepo::Docker,
            digest: ManifestDigest(
                "sha256:5e23090353324d887c48ad5e5c56d294eab81588df9605b07d1afe895f9cc8f8"
                    .to_string(),
            ),
            tag: None,
            upstream: None,
            license: None,
        }
    }
}

/// A catalog panel binding: the container mount contract plus the Hugging
/// Face dataset repository (`owner/name`) that satisfies it at runtime.
///
/// `bundle` is the runtime `DataBundle` ident — the runtime resolves it
/// through the local verified catalog cache and mounts the materialized
/// panel read-only at `mount`. HF repositories are never mounted directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelBinding {
    /// Local slot name used by the node (`DataBundleBinding::binding`).
    pub binding: String,
    /// Absolute container path the verified panel is mounted at.
    pub mount: String,
    /// Hugging Face dataset repository `owner/name`, validated at parse time.
    pub bundle: HfRepoId,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub schema_version: u32,
    pub plugin_name: String,
    pub image_ref: ImageRef,
    pub panels: Vec<PanelBinding>,
    pub nodes: Vec<String>,
}

impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            schema_version: 1,
            plugin_name: "new_plugin".to_string(),
            image_ref: Default::default(),
            panels: Default::default(),
            nodes: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST_TOML: &str = r#"
schema_version = 1
plugin_name = "mtag"
nodes = []

[image_ref]
repo = "Ghcr"
digest = "sha256:5e23090353324d887c48ad5e5c56d294eab81588df9605b07d1afe895f9cc8f8"

[[panels]]
binding = "ld_ref"
mount = "/panels/ld_ref"
bundle = "wjixiang/catalog-mtag-ld-ref-1000g-eur-w-ld"
"#;

    #[test]
    fn panel_bindings_parse_hf_dataset_repos() {
        let manifest: PluginManifest = toml::from_str(MANIFEST_TOML).unwrap();
        let panel = &manifest.panels[0];
        assert_eq!(panel.binding, "ld_ref");
        assert_eq!(panel.mount, "/panels/ld_ref");
        assert_eq!(panel.bundle.owner(), "wjixiang");
        assert_eq!(panel.bundle.name(), "catalog-mtag-ld-ref-1000g-eur-w-ld");
    }

    #[test]
    fn invalid_repo_ids_fail_manifest_parsing() {
        let broken = MANIFEST_TOML.replacen(
            "wjixiang/catalog-mtag-ld-ref-1000g-eur-w-ld",
            "not-a-repo-id",
            1,
        );
        let result = toml::from_str::<PluginManifest>(&broken);
        assert!(result.is_err(), "a bundle id without `owner/name` must be rejected at parse time");
    }
}
