//! Causal inference DAG node bundle (mediation, causal, cmest).

pub mod causal;
pub mod cmest;
pub mod mediation;
pub mod mediation_moderated;
pub mod mediation_serial;
pub mod mediation_weighted;
pub mod mediation_weighted_binary;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "causal"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(mediation::MediationNodeFactory {}));
        registry.register(Box::new(
            mediation_weighted::MediationWeightedNodeFactory {},
        ));
        registry.register(Box::new(
            mediation_weighted_binary::MediationWeightedBinaryNodeFactory {},
        ));
        registry.register(Box::new(mediation_serial::MediationSerialNodeFactory {}));
        registry.register(Box::new(
            mediation_moderated::MediationModeratedNodeFactory {},
        ));
        registry.register(Box::new(causal::CausalNodeFactory {}));
        registry.register(Box::new(cmest::CmestNodeFactory {}));
        registry.register(Box::new(cmest::CmestMultiNodeFactory {}));
        registry.register(Box::new(cmest::CmestBinaryYNodeFactory {}));
        registry.register(Box::new(cmest::CmestBinaryMNodeFactory {}));
        registry.register(Box::new(cmest::CmestWeightingNodeFactory {}));
        registry.register(Box::new(cmest::CmestGformulaNodeFactory {}));
    }
}
