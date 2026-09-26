//! Registry-agnostic image references.
//!
//! The canonical implementation lives in [`container_runtime::image`].
//! This module re-exports it so existing `registry_image(REPOSITORY, DIGEST)`
//! call sites and the public `nodes_io::image_registry` paths keep working.
//! The legacy bridge resolves against the fixed GHCR namespace; there is no
//! deployment-time registry override.

pub use container_runtime::image::{
    GHCR_NAMESPACE, GHCR_REGISTRY, ImageReference, ImageRepo, ManifestDigest, RegistryHost,
    RepositoryPath, registry_image,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexports_the_canonical_reference_builder() {
        let digest = "sha256:b7dd1d3a3493cc32af2dc286f2aace77036b1fd806bee4f14a4ff67509268be7";
        assert_eq!(
            registry_image("lava", digest).unwrap(),
            format!("ghcr.io/{GHCR_NAMESPACE}/lava@{digest}")
        );
        // The shim exposes the same validation, not a relaxed copy.
        assert!(registry_image("lava", "sha256:short").is_err());
    }
}
