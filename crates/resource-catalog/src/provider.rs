//! Self-describing resource providers.

use crate::entry::ResourceEntry;
use crate::registry::ResourceRegistry;

/// A unit of self-description: a bundle (e.g. `nodes-ldsc`) implements this to
/// declare the resources it owns, mirroring how node bundles implement
/// `NodePlugin::register` for the `NodeRegistry`.
///
/// Unlike `NodePlugin`, a provider exposes its entries via [`resources`] rather
/// than writing straight into the registry, so the catalog can validate and
/// reject duplicates with proper error propagation.
pub trait ResourceProvider: Send + Sync {
    /// Stable provider name, e.g. `"ldsc"`, `"genetics"`, `"infra"`.
    fn name(&self) -> &'static str;

    /// The resources this provider owns.
    fn resources(&self) -> Vec<ResourceEntry>;
}

/// Convenience: register a provider's entries into a registry.
pub fn register_provider(registry: &mut ResourceRegistry, provider: &dyn ResourceProvider) {
    for entry in provider.resources() {
        let _ = registry.register(entry);
    }
}
