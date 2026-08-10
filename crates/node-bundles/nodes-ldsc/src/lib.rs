//! LDSC and LCV DAG node bundle.

pub mod lcv;
pub mod ldsc_common;
pub mod ldsc_hsq;
pub mod ldsc_rg;
pub mod ldsc_sldsc;
pub mod liability;

use dag_core::resource_catalog::{
    ResourceAddress, ResourceEntry, ResourceKind, ResourceProvider,
    CATALOG_NAME,
};
use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "ldsc"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(ldsc_hsq::LdscHsqNodeFactory {}));
        registry.register(Box::new(ldsc_rg::LdscRgNodeFactory {}));
        registry.register(Box::new(ldsc_sldsc::LdscSldscNodeFactory {}));
        registry.register(Box::new(liability::LiabilityNodeFactory {}));
        registry.register(Box::new(lcv::LcvNodeFactory {}));
    }
}

/// Resource declarations for LDSC bundle: LD-score panels used by h², rg,
/// S-LDSC, LCV nodes.
pub struct Resources;
impl ResourceProvider for Resources {
    fn name(&self) -> &'static str {
        "ldsc"
    }
    fn resources(&self) -> Vec<ResourceEntry> {
        vec![
            ResourceEntry::new(
                "ldscore.1000g_eur",
                ResourceKind::IcebergTable,
                "1000G EUR LD-score panel (ld_score + w_ld columns)",
                ResourceAddress::iceberg_in(CATALOG_NAME, "ld_score", "1000g_eur"),
            ),
            ResourceEntry::new(
                "ldscore.ukbb_eur",
                ResourceKind::IcebergTable,
                "UKBB EUR LD-score panel (single ld_score column)",
                ResourceAddress::iceberg_in(CATALOG_NAME, "ld_score", "ukbb_eur"),
            ),
            ResourceEntry::new(
                "ldscore.baselineLD_v2_2_eur",
                ResourceKind::IcebergTable,
                "baselineLD v2.2 EUR LD-score panel (97 annotations)",
                ResourceAddress::iceberg_in(CATALOG_NAME, "ld_score", "baselineLD_v2_2_eur"),
            ),
        ]
    }
}
