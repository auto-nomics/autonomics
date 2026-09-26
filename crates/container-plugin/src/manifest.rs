use clap::builder::Str;
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

#[derive(Debug, Serialize, Deserialize)]
pub struct PanelRef(String);

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub schema_version: u32,
    pub plugin_name: String,
    pub image_ref: ImageRef,
    pub panels: Vec<PanelRef>,
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
