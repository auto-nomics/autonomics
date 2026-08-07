//! Survival analysis DAG node bundle (Fine-Gray, cuminc, KM).

pub mod cuminc;
pub mod fine_gray;
pub mod survival;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str { "survival" }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(fine_gray::FineGrayNodeFactory {}));
        registry.register(Box::new(cuminc::CumincNodeFactory {}));
        registry.register(Box::new(survival::SurvivalNodeFactory {}));
    }
}
