//! Trusted publication of environment images to the configured registry.
//!
//! Mirrors the GitHub publisher's role for plugins: agents never see this
//! interface; only the daemon pushes reviewed images. The pushed digest comes
//! from the registry (`--digestfile`), and the catalog is re-pinned to the
//! digest-pinned remote reference afterwards.

use std::sync::Arc;

use async_trait::async_trait;
use container_runtime::{
    DEFAULT_PUSH_TIMEOUT_SECS, ImageBuildConnection, ImagePushRequest, RegistryHost, RepositoryPath,
};

use crate::{Error, Result};

/// Push configuration; `enabled: false` refuses pushes without touching the
/// local activation path.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImageRegistryConfig {
    pub enabled: bool,
    /// Registry host, e.g. `ghcr.io`.
    pub registry: String,
    /// Repository namespace below the host, e.g. `auto-nomics/environments`.
    pub namespace: String,
}

impl Default for ImageRegistryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            registry: "ghcr.io".into(),
            namespace: "auto-nomics/environments".into(),
        }
    }
}

/// Immutable result of publishing one environment image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedImageReference {
    /// Digest-pinned remote reference (`host/ns/id@sha256:…`).
    pub reference: String,
    /// Registry-side manifest digest.
    pub digest: String,
}

/// Trusted publisher abstraction used by the environment lifecycle.
#[async_trait]
pub trait ImagePublisher: Send + Sync {
    /// Push one locally built environment image and report its immutable
    /// registry-side reference.
    async fn push_environment(
        &self,
        environment_id: &str,
        local_reference: &str,
    ) -> Result<PublishedImageReference>;
}

/// Trusted publisher type shared by the facade and the background distiller.
pub type SharedImagePublisher = Arc<dyn ImagePublisher>;

/// Production publisher: `podman push --digestfile` through the shared
/// [`ImageBuildConnection`].
#[derive(Clone)]
pub struct PodmanImagePublisher {
    config: ImageRegistryConfig,
    builder: Arc<dyn ImageBuildConnection>,
}

impl PodmanImagePublisher {
    pub fn new(config: ImageRegistryConfig, builder: Arc<dyn ImageBuildConnection>) -> Self {
        Self { config, builder }
    }

    /// Repository path (`registry/namespace/id`) for one environment.
    pub fn remote_repository(&self, environment_id: &str) -> Result<String> {
        crate::validate_plugin_name(environment_id)?;
        RegistryHost::new(&self.config.registry).map_err(Error::ImageRegistry)?;
        RepositoryPath::new(&format!("{}/{}", self.config.namespace, environment_id))
            .map_err(Error::ImageRegistry)?;
        Ok(format!(
            "{}/{}/{}",
            self.config.registry, self.config.namespace, environment_id
        ))
    }

    /// Tagged push target: the short local digest keeps history addressable
    /// in the registry without mutable tags like `latest`.
    fn push_target(&self, environment_id: &str, local_reference: &str) -> Result<String> {
        let short_digest = local_reference
            .rsplit_once('@')
            .and_then(|(_, digest)| digest.strip_prefix("sha256:"))
            .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(|hex| hex[..12].to_string())
            .ok_or_else(|| {
                Error::ImageRegistry(format!(
                    "local reference `{local_reference}` is not digest-pinned"
                ))
            })?;
        Ok(format!(
            "{}:{short_digest}",
            self.remote_repository(environment_id)?
        ))
    }
}

#[async_trait]
impl ImagePublisher for PodmanImagePublisher {
    async fn push_environment(
        &self,
        environment_id: &str,
        local_reference: &str,
    ) -> Result<PublishedImageReference> {
        if !self.config.enabled {
            return Err(Error::ImageRegistry("image publishing is disabled".into()));
        }
        let remote_tagged = self.push_target(environment_id, local_reference)?;
        let pushed = self
            .builder
            .push(ImagePushRequest {
                local_reference: local_reference.to_string(),
                remote_reference: remote_tagged,
                timeout_secs: DEFAULT_PUSH_TIMEOUT_SECS,
            })
            .await
            .map_err(|error| Error::ImageRegistry(error.to_string()))?;
        let reference = format!(
            "{}@{}",
            self.remote_repository(environment_id)?,
            pushed.digest
        );
        container_runtime::ImageReference::parse(&reference)
            .map(|_| ())
            .map_err(Error::ImageRegistry)?;
        Ok(PublishedImageReference {
            reference,
            digest: pushed.digest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_repositories_are_validated_addresses() {
        let publisher = PodmanImagePublisher {
            config: ImageRegistryConfig::default(),
            builder: panic_free_builder(),
        };
        assert_eq!(
            publisher.remote_repository("bioconductor-extra").unwrap(),
            "ghcr.io/auto-nomics/environments/bioconductor-extra"
        );
        assert!(publisher.remote_repository("Bad_Name").is_err());

        let mut invalid = ImageRegistryConfig::default();
        invalid.registry = "https://ghcr.io".into();
        let publisher = PodmanImagePublisher {
            config: invalid,
            builder: panic_free_builder(),
        };
        assert!(publisher.remote_repository("demo-env").is_err());
    }

    #[test]
    fn push_targets_use_the_short_local_digest() {
        let publisher = PodmanImagePublisher {
            config: ImageRegistryConfig::default(),
            builder: panic_free_builder(),
        };
        let digest = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(
            publisher
                .push_target("demo-env", &format!("localhost/ns/demo-env@{digest}"))
                .unwrap(),
            format!(
                "ghcr.io/auto-nomics/environments/demo-env:{}",
                "ab".repeat(6)
            )
        );
        assert!(
            publisher
                .push_target("demo-env", "localhost/ns/demo-env:rsi-1")
                .is_err()
        );
    }

    fn panic_free_builder() -> Arc<dyn ImageBuildConnection> {
        Arc::new(NoopBuilder)
    }

    struct NoopBuilder;

    #[async_trait]
    impl ImageBuildConnection for NoopBuilder {
        async fn build(
            &self,
            _request: container_runtime::ImageBuildRequest,
        ) -> std::result::Result<
            container_runtime::ImageBuildResult,
            container_runtime::ContainerRuntimeError,
        > {
            Err(container_runtime::ContainerRuntimeError::Invalid(
                "not used by these tests".into(),
            ))
        }

        async fn push(
            &self,
            _request: container_runtime::ImagePushRequest,
        ) -> std::result::Result<
            container_runtime::ImagePushResult,
            container_runtime::ContainerRuntimeError,
        > {
            Err(container_runtime::ContainerRuntimeError::Invalid(
                "not used by these tests".into(),
            ))
        }
    }
}
