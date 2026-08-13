//! Mendelian randomisation DAG node bundle.

pub mod mrlap;
pub mod mrpresso;
pub mod mvmr;
pub mod two_sample_mr;

use dag_core::resource_catalog::{
    CATALOG_NAME, ResourceAddress, ResourceEntry, ResourceKind, ResourceProvider,
};
use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "mr"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(two_sample_mr::TwoSampleMrNodeFactory {}));
        registry.register(Box::new(mrlap::MrlapNodeFactory {}));
        registry.register(Box::new(mrpresso::MrpressoNodeFactory {}));
        registry.register(Box::new(mvmr::MvmrNodeFactory {}));
    }
}

/// Resource declarations for MR bundle: LD-score panel (for MRLAP) and
/// per-chromosome LD-matrix tables (for TwoSampleMR clumping).
pub struct Resources;
impl ResourceProvider for Resources {
    fn name(&self) -> &'static str {
        "mr"
    }
    fn resources(&self) -> Vec<ResourceEntry> {
        vec![
            ResourceEntry::new(
                "ldscore.1000g_eur",
                ResourceKind::IcebergTable,
                "1000G EUR LD-score panel (used by MRLAP)",
                ResourceAddress::iceberg_in(CATALOG_NAME, "ld_score", "1000g_eur"),
            ),
            ResourceEntry::new(
                "ldmatrix.eur_chr",
                ResourceKind::IcebergTable,
                "1000G EUR pairwise LD-matrix base table (per-chromosome: eur_chr{N})",
                ResourceAddress::iceberg_in(CATALOG_NAME, "ld_matrix", "eur_chr"),
            ),
        ]
    }
}
