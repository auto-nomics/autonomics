//! Survival analysis DAG node bundle (Fine–Gray, cuminc, KM, and the
//! chordoma-protocol orchestration nodes: nested CV, locked fusion,
//! paired-BS primary endpoint).

pub mod bs_delta;
pub mod cuminc;
pub mod cv;
pub mod fine_gray;
pub mod fusion;
pub mod nested_oof;
pub mod survival;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "survival"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(fine_gray::FineGrayNodeFactory {}));
        registry.register(Box::new(cuminc::CumincNodeFactory {}));
        registry.register(Box::new(survival::SurvivalNodeFactory {}));
        registry.register(Box::new(nested_oof::NestedOofNodeFactory {}));
        registry.register(Box::new(fusion::FusionFitNodeFactory {}));
        registry.register(Box::new(fusion::LockPredictNodeFactory {}));
        registry.register(Box::new(bs_delta::PairedBsDeltaNodeFactory {}));
    }
}
