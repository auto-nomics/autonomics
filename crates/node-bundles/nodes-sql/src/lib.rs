//! SQL and echo DAG node bundle.

pub mod echo_node;
pub mod hypergeometric_ora;
pub mod lr_communication;
pub mod permutation_test;
pub mod sql_node;
pub mod table_transforms;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "sql"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(sql_node::SqlNodeFactory {}));
        registry.register(Box::new(echo_node::EchoNodeFactory {}));
        registry.register(Box::new(table_transforms::TableTransposeNodeFactory));
        registry.register(Box::new(table_transforms::MeltUnpivotNodeFactory));
        registry.register(Box::new(lr_communication::LrCommunicationScoreNodeFactory));
        registry.register(Box::new(permutation_test::PermutationTestNodeFactory));
        registry.register(Box::new(
            hypergeometric_ora::HypergeometricOraOnDfNodeFactory,
        ));
    }
}
