//! Latent class mixed model DAG node bundle (hlme).

pub mod hlme;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str { "lcmm" }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(hlme::HlmeNodeFactory {}));
        registry.register(Box::new(hlme::HlmePredictNodeFactory {}));
        registry.register(Box::new(hlme::HlmeCompareNodeFactory {}));
    }
}
