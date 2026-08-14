//! Statistical genetics DAG node bundle.

pub mod bivariate_mixer;
pub mod cpassoc;
pub mod genomic_sem;
pub mod hdl_l;
pub mod hdl_l_scan;
pub mod lava;
pub mod magma;
pub mod mtag;
pub mod susie_rss;
pub mod univariate_mixer;

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
        registry.register(Box::new(genomic_sem::GsemMungeNodeFactory {}));
        registry.register(Box::new(genomic_sem::GsemLdscNodeFactory {}));
        registry.register(Box::new(genomic_sem::GsemUsermodelNodeFactory {}));
        registry.register(Box::new(genomic_sem::GsemCommonfactorNodeFactory {}));
        registry.register(Box::new(genomic_sem::GsemRgmodelNodeFactory {}));
    }
}
