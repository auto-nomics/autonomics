//! Statistical genetics DAG node bundle.

pub mod bivariate_mixer;
pub mod cpassoc;
pub mod hdl_l;
pub mod hdl_l_scan;
pub mod lava;
pub mod magma;
pub mod mtag;
pub mod susie_rss;
pub mod univariate_mixer;

use dag_core::resource_catalog::{
    ResourceAddress, ResourceEntry, ResourceKind, ResourceProvider, CATALOG_NAME,
};
use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "genetics"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(lava::LavaLocusNodeFactory {}));
        registry.register(Box::new(lava::LavaUnivNodeFactory {}));
        registry.register(Box::new(lava::LavaBivarNodeFactory {}));
        registry.register(Box::new(lava::LavaPcorNodeFactory {}));
        registry.register(Box::new(lava::LavaMultiregNodeFactory {}));
        registry.register(Box::new(hdl_l::HdlLNodeFactory {}));
        registry.register(Box::new(hdl_l_scan::HdlLScanNodeFactory {}));
        registry.register(Box::new(mtag::MtagNodeFactory {}));
        registry.register(Box::new(cpassoc::CpassocNodeFactory {}));
        registry.register(Box::new(susie_rss::SusieRssNodeFactory {}));
        registry.register(Box::new(magma::MagmaAnnotateNodeFactory {}));
        registry.register(Box::new(magma::MagmaGeneNodeFactory {}));
        registry.register(Box::new(magma::MagmaSetNodeFactory {}));
        registry.register(Box::new(magma::MagmaMetaNodeFactory {}));
        registry.register(Box::new(univariate_mixer::UnivariateMixerNodeFactory {}));
        registry.register(Box::new(bivariate_mixer::BivariateMixerNodeFactory {}));
    }
}

/// Resource declarations for genetics bundle: LD-score panel (MTAG) and
/// per-chromosome LD-matrix tables (SuSiE-RSS).
pub struct Resources;
impl ResourceProvider for Resources {
    fn name(&self) -> &'static str {
        "genetics"
    }
    fn resources(&self) -> Vec<ResourceEntry> {
        vec![
            ResourceEntry::new(
                "ldscore.ukbb_eur",
                ResourceKind::IcebergTable,
                "UKBB EUR LD-score panel (used by MTAG)",
                ResourceAddress::iceberg_in(CATALOG_NAME, "ld_score", "ukbb_eur"),
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
