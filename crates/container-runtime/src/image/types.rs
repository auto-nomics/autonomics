use std::collections::BTreeMap;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ContainerRuntimeError;
use crate::types::PullPolicy;

#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Deserialize, Serialize)]
pub struct ImageListOptions {
    /// Podman image filters, for example `reference=quay.io/org/tool` or
    /// `dangling=true`.
    #[serde(default)]
    pub filters: Vec<String>,
}

impl ImageListOptions {
    pub fn with_filter(mut self, filter: impl Into<String>) -> Self {
        self.filters.push(filter.into());
        self
    }

    pub(crate) fn validate(&self) -> Result<(), ContainerRuntimeError> {
        for filter in &self.filters {
            if filter.trim().is_empty() || filter.contains('\0') {
                return Err(ContainerRuntimeError::Invalid(format!(
                    "invalid image list filter `{filter}`"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Deserialize, Serialize)]
pub struct ImageRemoveOptions {
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub ignore_missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize)]
pub struct ImageRecord {
    pub id: String,
    pub repository: String,
    pub tag: String,
    pub repo_tags: Vec<String>,
    pub repo_digests: Vec<String>,
    pub digest: Option<String>,
    pub created_unix_seconds: i64,
    pub size_bytes: u64,
    pub shared_size_bytes: u64,
    pub virtual_size_bytes: u64,
    pub container_count: i64,
    pub dangling: bool,
    pub labels: BTreeMap<String, String>,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ImageInspect {
    pub id: String,
    pub digest: Option<String>,
    pub repo_tags: Vec<String>,
    pub repo_digests: Vec<String>,
    pub created: Option<String>,
    pub architecture: Option<String>,
    pub os: Option<String>,
    pub size_bytes: u64,
    pub virtual_size_bytes: Option<u64>,
    pub labels: BTreeMap<String, String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ImagePullResult {
    pub image: String,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ImageRemoveResult {
    pub image: String,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct EnsureImageResult {
    pub pulled: bool,
    pub pull: Option<ImagePullResult>,
    pub image: ImageInspect,
}

/// Runtime-neutral local image management.
#[async_trait]
pub trait ImageManager: Send + Sync {
    async fn image_exists(&self, image: &str) -> Result<bool, ContainerRuntimeError>;
    async fn image_pull(&self, image: &str) -> Result<ImagePullResult, ContainerRuntimeError>;
    async fn image_inspect(&self, image: &str) -> Result<ImageInspect, ContainerRuntimeError>;
    async fn image_list(
        &self,
        options: ImageListOptions,
    ) -> Result<Vec<ImageRecord>, ContainerRuntimeError>;
    async fn image_remove(
        &self,
        image: &str,
        options: ImageRemoveOptions,
    ) -> Result<ImageRemoveResult, ContainerRuntimeError>;
    async fn ensure_image(
        &self,
        image: &str,
        policy: PullPolicy,
    ) -> Result<EnsureImageResult, ContainerRuntimeError>;
}
