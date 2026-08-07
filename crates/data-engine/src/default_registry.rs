//! Registration of all built-in node factories into a [`NodeRegistry`].
//!
//! [`dag_core::NodeRegistry`] is the infrastructure; this module populates it
//! with every concrete factory implemented in `data-engine`. Called once at
//! engine startup by [`crate::data_engine::DataEngine`].

use std::sync::Arc;

use datafusion::{
    catalog::CatalogProvider,
    execution::runtime_env::RuntimeEnv,
};
use datalake::Datalake;

use dag_core::registry::{NodeCtx, NodeRegistry};

use crate::nodes::{
    bivariate_mixer::BivariateMixerNodeFactory,
    bkmr::BkmrNodeFactory,
    causal::CausalNodeFactory,
    chi_square::ChiSquareNodeFactory,
    cmest::{
        CmestBinaryMNodeFactory, CmestBinaryYNodeFactory, CmestGformulaNodeFactory,
        CmestMultiNodeFactory, CmestNodeFactory, CmestWeightingNodeFactory,
    },
    coloc::ColocAbfNodeFactory,
    cox_regression::CoxRegressionNodeFactory,
    cpassoc::CpassocNodeFactory,
    cuminc::CumincNodeFactory,
    echo_node::EchoNodeFactory,
    epi_lasso::EpiLassoNodeFactory,
    epi_rcs::EpiRcsNodeFactory,
    epi_roc::EpiRocNodeFactory,
    epi_wqs::EpiWqsNodeFactory,
    evalue::EvalueNodeFactory,
    fine_gray::FineGrayNodeFactory,
    hdl_l::HdlLNodeFactory,
    hdl_l_scan::HdlLScanNodeFactory,
    hlme::{HlmeCompareNodeFactory, HlmeNodeFactory, HlmePredictNodeFactory},
    lava::{
        LavaBivarNodeFactory, LavaLocusNodeFactory, LavaMultiregNodeFactory, LavaPcorNodeFactory,
        LavaUnivNodeFactory,
    },
    lcv::LcvNodeFactory,
    ldsc_hsq::LdscHsqNodeFactory,
    ldsc_rg::LdscRgNodeFactory,
    ldsc_sldsc::LdscSldscNodeFactory,
    liability::LiabilityNodeFactory,
    linear_regression::LinearRegressionNodeFactory,
    logistic_regression::LogisticRegressionNodeFactory,
    magma::{
        MagmaAnnotateNodeFactory, MagmaGeneNodeFactory, MagmaMetaNodeFactory, MagmaSetNodeFactory,
    },
    mediation::MediationNodeFactory,
    mrlap::MrlapNodeFactory,
    mrpresso::MrpressoNodeFactory,
    mtag::MtagNodeFactory,
    mvmr::MvmrNodeFactory,
    sink_file::FileSinkNodeFactory,
    sink_iceberg::IcebergSinkNodeFactory,
    source_file::FileSourceNodeFactory,
    source_iceberg::IcebergSourceNodeFactory,
    source_opengwas_tophits::OpengwasTophitsNodeFactory,
    source_opentargets::{OpentargetsAssociationsNodeFactory, OpentargetsSearchNodeFactory},
    sql_node::SqlNodeFactory,
    survey_calibrate::{CalibrateFactory, PostStratifyFactory, RakeFactory, TrimWeightsFactory},
    survey_describe::{
        SvyMeanFactory, SvyQuantileFactory, SvyRatioFactory, SvyTableFactory, SvyTotalFactory,
        SvyVarFactory,
    },
    survey_model::{
        SvyCoxphFactory, SvyGlmFactory, SvyIvregFactory, SvyLoglinFactory, SvyMleFactory,
        SvyNlsFactory, SvyOlrFactory, SvySurvregFactory,
    },
    survey_survival::{SvyKmFactory, SvyLogrankFactory},
    survey_test::{SvyChisqFactory, SvyCiPropFactory, SvyRankTestFactory, SvyTtestFactory},
    survey_utility::{RegTermTestFactory, SvyByFactory, SvyContrastFactory, SvyStandardizeFactory},
    survival::SurvivalNodeFactory,
    susie_rss::SusieRssNodeFactory,
    two_sample_mr::TwoSampleMrNodeFactory,
    univariate_mixer::UnivariateMixerNodeFactory,
    viz::VizNodeFactory,
};

/// Build a [`NodeRegistry`] populated with every built-in node factory.
///
/// This is the engine's "default plugin set" — the single place that knows
/// about all concrete factories. As node bundles are extracted into separate
/// crates (Phase 1+), each bundle's `Plugin::register` will replace the
/// corresponding `register` calls here.
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

    registry.register(Box::new(SqlNodeFactory {}));
    registry.register(Box::new(FileSourceNodeFactory {}));
    registry.register(Box::new(IcebergSourceNodeFactory {}));
    registry.register(Box::new(FileSinkNodeFactory {}));
    registry.register(Box::new(IcebergSinkNodeFactory {}));
    registry.register(Box::new(LdscHsqNodeFactory {}));
    registry.register(Box::new(LdscRgNodeFactory {}));
    registry.register(Box::new(LcvNodeFactory {}));
    registry.register(Box::new(LdscSldscNodeFactory {}));
    registry.register(Box::new(LiabilityNodeFactory {}));
    registry.register(Box::new(LinearRegressionNodeFactory {}));
    registry.register(Box::new(LogisticRegressionNodeFactory {}));
    registry.register(Box::new(MediationNodeFactory {}));
    registry.register(Box::new(ChiSquareNodeFactory {}));
    registry.register(Box::new(CmestNodeFactory {}));
    registry.register(Box::new(CmestMultiNodeFactory {}));
    registry.register(Box::new(CmestBinaryYNodeFactory {}));
    registry.register(Box::new(CmestBinaryMNodeFactory {}));
    registry.register(Box::new(CmestWeightingNodeFactory {}));
    registry.register(Box::new(CmestGformulaNodeFactory {}));
    registry.register(Box::new(CoxRegressionNodeFactory {}));
    registry.register(Box::new(FineGrayNodeFactory {}));
    registry.register(Box::new(CumincNodeFactory {}));
    registry.register(Box::new(SurvivalNodeFactory {}));
    registry.register(Box::new(EpiRcsNodeFactory {}));
    registry.register(Box::new(EpiRocNodeFactory {}));
    registry.register(Box::new(EpiLassoNodeFactory {}));
    registry.register(Box::new(EpiWqsNodeFactory {}));
    registry.register(Box::new(EchoNodeFactory {}));
    registry.register(Box::new(ColocAbfNodeFactory {}));
    registry.register(Box::new(BkmrNodeFactory {}));
    registry.register(Box::new(HlmeNodeFactory {}));
    registry.register(Box::new(HlmePredictNodeFactory {}));
    registry.register(Box::new(HlmeCompareNodeFactory {}));
    registry.register(Box::new(EvalueNodeFactory {}));
    registry.register(Box::new(TwoSampleMrNodeFactory {}));
    registry.register(Box::new(MrlapNodeFactory {}));
    registry.register(Box::new(MrpressoNodeFactory {}));
    registry.register(Box::new(LavaLocusNodeFactory {}));
    registry.register(Box::new(LavaUnivNodeFactory {}));
    registry.register(Box::new(LavaBivarNodeFactory {}));
    registry.register(Box::new(LavaPcorNodeFactory {}));
    registry.register(Box::new(LavaMultiregNodeFactory {}));
    registry.register(Box::new(HdlLNodeFactory {}));
    registry.register(Box::new(HdlLScanNodeFactory {}));
    registry.register(Box::new(UnivariateMixerNodeFactory {}));
    registry.register(Box::new(BivariateMixerNodeFactory {}));
    registry.register(Box::new(CausalNodeFactory {}));
    registry.register(Box::new(MtagNodeFactory {}));
    registry.register(Box::new(MvmrNodeFactory {}));
    registry.register(Box::new(CpassocNodeFactory {}));
    registry.register(Box::new(VizNodeFactory {}));
    registry.register(Box::new(OpentargetsAssociationsNodeFactory {}));
    registry.register(Box::new(OpentargetsSearchNodeFactory {}));
    registry.register(Box::new(OpengwasTophitsNodeFactory {}));
    registry.register(Box::new(SusieRssNodeFactory {}));
    registry.register(Box::new(MagmaAnnotateNodeFactory {}));
    registry.register(Box::new(MagmaGeneNodeFactory {}));
    registry.register(Box::new(MagmaSetNodeFactory {}));
    registry.register(Box::new(MagmaMetaNodeFactory {}));
    #[cfg(feature = "bundle-hypothesize")]
    registry.register_plugin(&nodes_hypothesize::Plugin);
    // ── survey-package nodes ───────────────────────────────────────────
    registry.register(Box::new(SvyMeanFactory {}));
    registry.register(Box::new(SvyTotalFactory {}));
    registry.register(Box::new(SvyVarFactory {}));
    registry.register(Box::new(SvyRatioFactory {}));
    registry.register(Box::new(SvyTableFactory {}));
    registry.register(Box::new(SvyQuantileFactory {}));
    registry.register(Box::new(SvyGlmFactory {}));
    registry.register(Box::new(SvyCoxphFactory {}));
    registry.register(Box::new(SvySurvregFactory {}));
    registry.register(Box::new(SvyOlrFactory {}));
    registry.register(Box::new(SvyLoglinFactory {}));
    registry.register(Box::new(SvyMleFactory {}));
    registry.register(Box::new(SvyNlsFactory {}));
    registry.register(Box::new(SvyIvregFactory {}));
    registry.register(Box::new(PostStratifyFactory {}));
    registry.register(Box::new(RakeFactory {}));
    registry.register(Box::new(CalibrateFactory {}));
    registry.register(Box::new(TrimWeightsFactory {}));
    registry.register(Box::new(SvyTtestFactory {}));
    registry.register(Box::new(SvyRankTestFactory {}));
    registry.register(Box::new(SvyChisqFactory {}));
    registry.register(Box::new(SvyCiPropFactory {}));
    registry.register(Box::new(SvyKmFactory {}));
    registry.register(Box::new(SvyLogrankFactory {}));
    registry.register(Box::new(SvyByFactory {}));
    registry.register(Box::new(SvyContrastFactory {}));
    registry.register(Box::new(SvyStandardizeFactory {}));
    registry.register(Box::new(RegTermTestFactory {}));
    // ── machine-learning nodes ────────────────────────────────────────
    #[cfg(feature = "bundle-ml")]
    registry.register_plugin(&nodes_ml::Plugin);
    registry
}
