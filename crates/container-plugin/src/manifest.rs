use container_runtime::image::ImageReference;
use crate::node_definition::NodeDefinition;
use data_catalog::model::HfRepoId;
use serde::{Deserialize, Serialize};

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

/// Audit metadata for the image this manifest runs. The executable
/// reference (`host/path@sha256:…`) is mandatory and lives in the
/// [`ImageReference`] inner field; everything else here is non-runtime
/// information about provenance and licensing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMetadata {
    /// Full image reference `host/path@sha256:…`. Parsed and validated at
    /// load time; the parsed host/path/digest fields stay immutable.
    pub reference: ImageReference,
    /// Display-only tag for the published image; never participates in
    /// reference resolution (the digest does).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Upstream source description (e.g. `MTAG 1.0.8 @ 9e17f3c`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// SPDX license identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

impl Default for ImageMetadata {
    /// Default image is the hello-world image on docker.io's official namespace.
    fn default() -> Self {
        let reference = ImageReference::parse(
            "docker.io/library/hello-world@sha256:5e23090353324d887c48ad5e5c56d294eab81588df9605b07d1afe895f9cc8f8",
        )
        .expect("hard-coded default image must satisfy validation");
        Self {
            reference,
            tag: None,
            upstream: None,
            license: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub schema_version: u32,
    pub plugin_name: String,
    pub image: ImageMetadata,
    /// Panel-free families (mrpresso, mvmr) legitimately omit this.
    #[serde(default)]
    pub panels: Vec<PanelBinding>,
    #[serde(default)]
    pub nodes: Vec<NodeDefinition>,
}

impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            schema_version: 1,
            plugin_name: "new_plugin".to_string(),
            image: Default::default(),
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

[image]
reference = "ghcr.io/auto-nomics/autonomics/mtag@sha256:28ac0a0a0ee741340390b7588bf8ba36e6b316d62adc0cab7fd4494f5dd6a90c"

[[panels]]
binding = "ld_ref"
mount = "/panels/ld_ref"
bundle = "wjixiang/catalog-mtag-ld-ref-1000g-eur-w-ld"
"#;

    #[test]
    fn image_references_parse_full_repository_paths() {
        let manifest: PluginManifest = toml::from_str(MANIFEST_TOML).unwrap();
        let reference = &manifest.image.reference;
        assert_eq!(reference.registry().as_str(), "ghcr.io");
        assert_eq!(reference.path().as_str(), "auto-nomics/autonomics/mtag");
        assert_eq!(
            reference.digest().as_str(),
            "sha256:28ac0a0a0ee741340390b7588bf8ba36e6b316d62adc0cab7fd4494f5dd6a90c"
        );
    }

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
        assert!(
            result.is_err(),
            "a bundle id without `owner/name` must be rejected at parse time"
        );
    }

    #[test]
    fn invalid_image_references_fail_manifest_parsing() {
        // Bad digest
        let broken = MANIFEST_TOML.replacen(
            "28ac0a0a0ee741340390b7588bf8ba36e6b316d62adc0cab7fd4494f5dd6a90c",
            "not-a-digest",
            1,
        );
        assert!(
            toml::from_str::<PluginManifest>(&broken).is_err(),
            "a malformed digest must be rejected at parse time"
        );

        // Missing `@digest`
        let broken = MANIFEST_TOML.replacen(
            "mtag@sha256:28ac0a0a0ee741340390b7588bf8ba36e6b316d62adc0cab7fd4494f5dd6a90c",
            "mtag",
            1,
        );
        assert!(
            toml::from_str::<PluginManifest>(&broken).is_err(),
            "an image reference without `@digest` must be rejected at parse time"
        );
    }
}

