//! SQL and echo DAG node bundle.

pub mod echo_node;
pub mod sql_node;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str { "sql" }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(sql_node::SqlNodeFactory {}));
        registry.register(Box::new(echo_node::EchoNodeFactory {}));
    }
}
