use crate::node_definition::NodeDefinition;
use container_runtime::image::ImageReference;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatus {
    Draft,
    /// An installed plugin is being changed in its long-lived workspace.
    Updating,
    Validating,
    NeedsFix,
    PendingReview,
    Approved,
    Publishing,
    Published,
    PullRequestOpen,
    PullRequestMerged,
    InstallPending,
    Installed,
    PublishFailed,
    Rejected,
}

/// Installation channel for a plugin whose development workspace and runtime
/// installation are managed independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginInstallationStatus {
    NotInstalled,
    LocalActive,
    GithubActive,
    Retired,
}

/// Daemon-owned installation facts. Unlike `status`, these fields describe
/// the immutable runtime source rather than the mutable development state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginInstallationMetadata {
    #[serde(default)]
    pub status: Option<PluginInstallationStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_commit: Option<String>,
}

impl PluginInstallationMetadata {
    pub fn is_runtime_active(&self) -> bool {
        matches!(
            self.status,
            Some(PluginInstallationStatus::LocalActive | PluginInstallationStatus::GithubActive)
        )
    }
}

/// Daemon-owned publication and update facts associated with one plugin.
///
/// Runtime consumers only need the node declarations and environment. These
/// fields let the RSI host recover lifecycle context from the same manifest
/// that carries `PluginStatus`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginLifecycleMetadata {
    /// Requests that motivated the current development or update.
    #[serde(default)]
    pub request_ids: Vec<String>,
    /// Host-provided reason retained with the lifecycle record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// Installed revision from which the current update started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    /// Installed remote from which the current update started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_remote: Option<String>,
    /// Remote created or updated by publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Commit reviewed and pushed for a new plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_commit: Option<String>,
    /// Open update PR number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_number: Option<u64>,
    /// Open update PR URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_url: Option<String>,
    /// Immutable commit produced when the update PR merged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_commit: Option<String>,
    /// Repository-relative latest validation report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_report: Option<String>,
}

fn default_plugin_status() -> PluginStatus {
    // Existing manifests predate daemon-owned lifecycle tracking. They are
    // loaded from published sources, so published is the conservative default.
    PluginStatus::Published
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub schema_version: u32,
    pub plugin_name: String,
    /// Daemon-owned lifecycle marker. Host code transitions it; agents can
    /// read it but must not treat it as an editable plugin field.
    #[serde(default = "default_plugin_status")]
    pub status: PluginStatus,
    /// Immutable runtime installation record, independent of development state.
    #[serde(default)]
    pub installation: PluginInstallationMetadata,
    /// Publication and update metadata owned by the RSI daemon.
    #[serde(default)]
    pub lifecycle: PluginLifecycleMetadata,
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
            status: default_plugin_status(),
            installation: Default::default(),
            lifecycle: Default::default(),
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
    fn legacy_manifests_default_to_published_status() {
        let manifest: PluginManifest = toml::from_str(MANIFEST_TOML).unwrap();
        assert!(matches!(manifest.status, PluginStatus::Published));
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
