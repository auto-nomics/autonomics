//! Node abstractions and built-in implementations.
//!
//! Core traits ([`DagNode`], [`NodePorts`], [`NodeInput`]) and shared helpers
//! ([`numeric_util`], [`sink_common`]) are re-exported from [`dag_core`] via
//! thin shim modules so existing `super::meta::*` / `super::numeric_util::*`
//! paths keep working.

// ── Re-export shims for modules now living in dag-core ────────────────────
pub mod meta {
    pub use dag_core::node::*;
}
pub mod numeric_util {
    pub use dag_core::arrow_util::*;
}
pub mod sink_common {
    pub use dag_core::sink::*;
}

// ── Remaining inline node modules ─────────────────────────────────────────
pub mod bivariate_mixer;
pub mod causal;
pub mod cmest;
pub mod cpassoc;
pub mod hdl_l;
pub mod hdl_l_scan;
pub mod hlme;
pub mod lava;
pub mod lcv;
pub mod ldsc_common;
pub mod ldsc_hsq;
pub mod ldsc_rg;
pub mod ldsc_sldsc;
pub mod liability;
pub mod magma;
pub mod mediation;
pub mod mrlap;
pub mod mrpresso;
pub mod mtag;
pub mod mvmr;
pub mod sink_file;
pub mod sink_iceberg;
pub mod source_file;
pub mod source_iceberg;
pub mod source_opengwas;
pub mod source_opengwas_tophits;
pub mod source_opentargets;
pub mod survey_calibrate;
pub mod survey_common;
pub mod survey_describe;
pub mod survey_model;
pub mod survey_survival;
pub mod survey_test;
pub mod survey_utility;
pub mod susie_rss;
pub mod two_sample_mr;
pub mod univariate_mixer;

// ── Re-exports ────────────────────────────────────────────────────────────
pub use bivariate_mixer::{BivariateMixerNode, BivariateMixerNodeFactory, BivariateMixerNodeSpec};
pub use cpassoc::{CpassocConfig, CpassocNode, CpassocNodeFactory};
pub use hlme::{
    HlmeCompareNode, HlmeCompareNodeFactory, HlmeConfig, HlmeNode, HlmeNodeFactory,
    HlmePredictNode, HlmePredictNodeFactory,
};
pub use hdl_l::{HdlLNode, HdlLNodeFactory, HdlLSpec};
pub use lava::{
    LavaBivarNode, LavaBivarNodeFactory, LavaLocusNode, LavaLocusNodeFactory, LavaMultiregNode,
    LavaMultiregNodeFactory, LavaPcorNode, LavaPcorNodeFactory, LavaUnivNode, LavaUnivNodeFactory,
};
pub use lcv::{LcvConfig, LcvNode, LcvNodeFactory};
pub use ldsc_hsq::{LdscHsqConfig, LdscHsqNode, LdscHsqNodeFactory};
pub use ldsc_rg::{LdscRgConfig, LdscRgNode, LdscRgNodeFactory};
pub use ldsc_sldsc::{LdscSldscConfig, LdscSldscNode, LdscSldscNodeFactory};
pub use liability::{LiabilityConfig, LiabilityNode, LiabilityNodeFactory};
pub use meta::{DEFAULT_PORT, DagNode, NodeId, NodeInput, NodePorts, Port};
pub use mrlap::{MrlapNode, MrlapNodeFactory, MrlapSpec};
pub use mrpresso::{MrpressoConfig, MrpressoNode, MrpressoNodeFactory};
pub use mtag::{MtagConfig, MtagNode, MtagNodeFactory};
pub use mvmr::{MvmrConfig, MvmrNode, MvmrNodeFactory};
pub use sink_common::SinkMode;
pub use sink_file::{FileSinkNode, FileSinkNodeFactory, FileSinkNodeSpec, WriteFormat};
pub use sink_iceberg::{IcebergSinkNode, IcebergSinkNodeFactory, IcebergSinkNodeSpec};
pub use source_file::{FileFormat, FileSourceNode, FileSourceNodeFactory, FileSourceNodeSpec};
pub use source_iceberg::{IcebergSourceNode, IcebergSourceNodeFactory, IcebergSourceNodeSpec};
pub use source_opentargets::{
    OpentargetsAssociationsNode, OpentargetsAssociationsNodeFactory, OpentargetsAssociationsSpec,
    OpentargetsSearchNode, OpentargetsSearchNodeFactory, OpentargetsSearchSpec,
};
pub use source_opengwas_tophits::{
    OpengwasTophitsNode, OpengwasTophitsNodeFactory, OpengwasTophitsSpec,
};
pub use source_opengwas::{
    OpengwasAssociationsNode, OpengwasAssociationsNodeFactory, OpengwasAssociationsSpec,
    OpengwasPhewasNode, OpengwasPhewasNodeFactory, OpengwasPhewasSpec,
    OpengwasGwasinfoNode, OpengwasGwasinfoNodeFactory, OpengwasGwasinfoSpec,
    OpengwasGwasinfoSearchNode, OpengwasGwasinfoSearchNodeFactory, OpengwasGwasinfoSearchSpec,
    OpengwasVariantsRsidNode, OpengwasVariantsRsidNodeFactory, OpengwasVariantsRsidSpec,
    OpengwasVariantsChrposNode, OpengwasVariantsChrposNodeFactory, OpengwasVariantsChrposSpec,
    OpengwasLdClumpNode, OpengwasLdClumpNodeFactory, OpengwasLdClumpSpec,
};
pub use susie_rss::{SusieRssNode, SusieRssNodeFactory, SusieRssSpec};
pub use two_sample_mr::{
    TwoSampleMrNode, TwoSampleMrNodeFactory, TwoSampleMrNodeSpec, TwoSampleMrParameters,
};
pub use univariate_mixer::{
    UnivariateMixerNode, UnivariateMixerNodeFactory, UnivariateMixerNodeSpec,
};

// Survey-package nodes
pub use survey_calibrate::{
    CalibrateFactory, PostStratifyFactory, RakeFactory, TrimWeightsFactory,
};
pub use survey_common::{SurveyDesignSpec, SurveyStubNode};
pub use survey_describe::{
    SvyMeanFactory, SvyQuantileFactory, SvyRatioFactory, SvyTableFactory, SvyTotalFactory,
    SvyVarFactory,
};
pub use survey_model::{
    SvyCoxphFactory, SvyGlmFactory, SvyIvregFactory, SvyLoglinFactory, SvyMleFactory,
    SvyNlsFactory, SvyOlrFactory, SvySurvregFactory,
};
pub use survey_survival::{SvyKmFactory, SvyLogrankFactory};
pub use survey_test::{SvyChisqFactory, SvyCiPropFactory, SvyRankTestFactory, SvyTtestFactory};
pub use survey_utility::{
    RegTermTestFactory, SvyByFactory, SvyContrastFactory, SvyStandardizeFactory,
};
