//! Visualization DAG node bundle.

pub mod viz;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str { "viz" }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(viz::VizNodeFactory {}));
    }
}
