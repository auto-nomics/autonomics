//! Registration of all built-in node factories into a [`NodeRegistry`].
//!
//! [`dag_core::NodeRegistry`] is the infrastructure; this module populates it
//! with every concrete factory implemented in `data-engine` and all enabled
//! bundle plugins. Called once at engine startup by [`crate::data_engine::DataEngine`].

use std::sync::Arc;

use datafusion::{
    catalog::CatalogProvider,
    execution::runtime_env::RuntimeEnv,
};
use datalake::Datalake;

use dag_core::registry::NodeRegistry;

/// Build a [`NodeRegistry`] populated with every built-in node factory.
///
/// Node bundles are registered via Cargo features (default: all enabled).
pub fn build_default_registry(
    runtime_env: Arc<RuntimeEnv>,
    iceberg_catalog: Option<Arc<dyn CatalogProvider>>,
    datalake: Arc<Datalake>,
    opendal: Option<Arc<fs::OpendalFileStorage>>,
) -> NodeRegistry {
    let mut registry = NodeRegistry::with_ingredients(
        runtime_env,
        iceberg_catalog,
        datalake,
        opendal,
    );

    // ── Phase 3: IO, causal, lcmm, mr, survey bundles ──────────────────
    #[cfg(feature = "bundle-io")]
    registry.register_plugin(&nodes_io::Plugin);
    #[cfg(feature = "bundle-causal")]
    registry.register_plugin(&nodes_causal::Plugin);
    #[cfg(feature = "bundle-lcmm")]
    registry.register_plugin(&nodes_lcmm::Plugin);
    #[cfg(feature = "bundle-mr")]
    registry.register_plugin(&nodes_mr::Plugin);
    #[cfg(feature = "bundle-survey")]
    registry.register_plugin(&nodes_survey::Plugin);

    // ── Phase 2: regression, survival, coloc, epi, viz, sql bundles ────
    #[cfg(feature = "bundle-regression")]
    registry.register_plugin(&nodes_regression::Plugin);
    #[cfg(feature = "bundle-survival")]
    registry.register_plugin(&nodes_survival::Plugin);
    #[cfg(feature = "bundle-coloc")]
    registry.register_plugin(&nodes_coloc::Plugin);
    #[cfg(feature = "bundle-epi")]
    registry.register_plugin(&nodes_epi::Plugin);
    #[cfg(feature = "bundle-viz")]
    registry.register_plugin(&nodes_viz::Plugin);
    #[cfg(feature = "bundle-sql")]
    registry.register_plugin(&nodes_sql::Plugin);

    // ── Phase 1: ml, hypothesize bundles ───────────────────────────────
    #[cfg(feature = "bundle-ml")]
    registry.register_plugin(&nodes_ml::Plugin);
    #[cfg(feature = "bundle-hypothesize")]
    registry.register_plugin(&nodes_hypothesize::Plugin);

    // ── Phase 4 (pending): LDSC + genetics nodes remain inline ────────
    // These will be extracted in the next phase.
    use crate::nodes::{
        bivariate_mixer::BivariateMixerNodeFactory,
        cpassoc::CpassocNodeFactory,
        hdl_l::HdlLNodeFactory,
        hdl_l_scan::HdlLScanNodeFactory,
        lava::{
            LavaBivarNodeFactory, LavaLocusNodeFactory, LavaMultiregNodeFactory, LavaPcorNodeFactory,
            LavaUnivNodeFactory,
        },
        lcv::LcvNodeFactory,
        ldsc_hsq::LdscHsqNodeFactory,
        ldsc_rg::LdscRgNodeFactory,
        ldsc_sldsc::LdscSldscNodeFactory,
        liability::LiabilityNodeFactory,
        magma::{
            MagmaAnnotateNodeFactory, MagmaGeneNodeFactory, MagmaMetaNodeFactory, MagmaSetNodeFactory,
        },
        mtag::MtagNodeFactory,
        susie_rss::SusieRssNodeFactory,
        univariate_mixer::UnivariateMixerNodeFactory,
    };

    registry.register(Box::new(LdscHsqNodeFactory {}));
    registry.register(Box::new(LdscRgNodeFactory {}));
    registry.register(Box::new(LcvNodeFactory {}));
    registry.register(Box::new(LdscSldscNodeFactory {}));
    registry.register(Box::new(LiabilityNodeFactory {}));
    registry.register(Box::new(LavaLocusNodeFactory {}));
    registry.register(Box::new(LavaUnivNodeFactory {}));
    registry.register(Box::new(LavaBivarNodeFactory {}));
    registry.register(Box::new(LavaPcorNodeFactory {}));
    registry.register(Box::new(LavaMultiregNodeFactory {}));
    registry.register(Box::new(HdlLNodeFactory {}));
    registry.register(Box::new(HdlLScanNodeFactory {}));
    registry.register(Box::new(UnivariateMixerNodeFactory {}));
    registry.register(Box::new(BivariateMixerNodeFactory {}));
    registry.register(Box::new(MtagNodeFactory {}));
    registry.register(Box::new(CpassocNodeFactory {}));
    registry.register(Box::new(SusieRssNodeFactory {}));
    registry.register(Box::new(MagmaAnnotateNodeFactory {}));
    registry.register(Box::new(MagmaGeneNodeFactory {}));
    registry.register(Box::new(MagmaSetNodeFactory {}));
    registry.register(Box::new(MagmaMetaNodeFactory {}));

    registry
}
