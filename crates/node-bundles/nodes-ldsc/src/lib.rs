//! LDSC and LCV DAG node bundle.

pub mod lcv;
pub mod ldsc_common;
pub mod ldsc_hsq;
pub mod ldsc_rg;
pub mod ldsc_sldsc;
pub mod liability;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "ldsc"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(ldsc_sldsc::LdscSldscNodeFactory {}));
        registry.register(Box::new(liability::LiabilityNodeFactory {}));
        registry.register(Box::new(lcv::LcvNodeFactory {}));
    }
}
