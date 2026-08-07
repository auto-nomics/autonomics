//! Plugin trait for extensible node bundles.
//!
//! Each node-bundle crate exports a type implementing [`NodePlugin`]. The
//! engine host (`data-engine`) selects which bundles to enable via Cargo
//! features and calls [`NodeRegistry::register_plugin`] for each enabled
//! bundle at startup.

use crate::registry::NodeRegistry;

/// A pluggable bundle of related DAG nodes.
///
/// # Example
///
/// ```ignore
/// use dag_core::{NodePlugin, NodeRegistry};
///
/// pub struct Plugin;
/// impl NodePlugin for Plugin {
///     fn name(&self) -> &'static str { "mr" }
///     fn register(&self, registry: &mut NodeRegistry) {
///         registry.register(Box::new(TwoSampleMrNodeFactory));
///         // ...
///     }
/// }
/// ```
pub trait NodePlugin: Send + Sync {
    /// Unique identifier for this bundle (e.g. `"mr"`, `"ldsc"`, `"ml"`).
    fn name(&self) -> &'static str;

    /// Register all node factories from this bundle into the registry.
    fn register(&self, registry: &mut NodeRegistry);

    /// Test-only: return a minimal valid spec for `kind`, used by the
    /// registry invariant test. Return `None` to skip build-testing `kind`.
    fn fixture_spec(&self, _kind: &str) -> Option<serde_json::Value> {
        None
    }
}
