//! Node abstractions and built-in implementations.
//!
//! [`meta`] defines the [`DagNode`] trait, [`NodePorts`], and [`NodeInput`] —
//! the contract every node fulfils. Concrete implementations live in
//! [`source_file`], [`source_iceberg`], [`sql_node`], [`sink_file`], and
//! [`sink_iceberg`].

pub mod bivariate_mixer;
pub mod bkmr;
pub mod causal;
pub mod chi_square;
pub mod cmest;
pub mod coloc;
pub mod cox_regression;
pub mod cpassoc;
pub mod cuminc;
pub mod echo_node;
pub mod epi_lasso;
pub mod epi_rcs;
pub mod epi_roc;
pub mod epi_wqs;
pub mod evalue;
pub mod fine_gray;
pub mod hdl_l;
pub mod hdl_l_scan;
pub mod hypothesize;
pub mod lava;
pub mod lcv;
pub mod ldsc_common;
pub mod ldsc_hsq;
pub mod ldsc_rg;
pub mod ldsc_sldsc;
pub mod liability;
pub mod linear_regression;
pub mod logistic_regression;
pub mod magma;
pub mod mediation;
pub mod meta;
pub mod mrlap;
pub mod mrpresso;
pub mod mtag;
pub mod mvmr;
pub mod numeric_util;
pub mod sink_common;
pub mod sink_file;
pub mod sink_iceberg;
pub mod source_file;
pub mod source_iceberg;
pub mod source_opengwas_tophits;
pub mod source_opentargets;
pub mod sql_node;
pub mod survey_calibrate;
pub mod survey_common;
pub mod survey_describe;
pub mod survey_model;
pub mod survey_survival;
pub mod survey_test;
pub mod survey_utility;
pub mod survival;
pub mod susie_rss;
pub mod two_sample_mr;
pub mod univariate_mixer;
pub mod viz;
pub mod ml;

pub use bivariate_mixer::{BivariateMixerNode, BivariateMixerNodeFactory, BivariateMixerNodeSpec};
pub use chi_square::{ChiSquareNode, ChiSquareNodeFactory, ChiSquareNodeSpec};
pub use cpassoc::{CpassocConfig, CpassocNode, CpassocNodeFactory};
pub use echo_node::{EchoNode, EchoNodeFactory, EchoNodeSpec};
pub use coloc::{ColocAbfConfig, ColocAbfNode, ColocAbfNodeFactory, DatasetSpec as ColocDatasetSpec};
pub use bkmr::{BkmrConfig, BkmrNode, BkmrNodeFactory};
pub use evalue::{EvalueConfig, EvalueNode, EvalueNodeFactory};
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
pub use linear_regression::{
    LinearRegressionNode, LinearRegressionNodeFactory, LinearRegressionNodeSpec,
};
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
pub use sql_node::{SqlNode, SqlNodeFactory, SqlNodeSpec};
pub use susie_rss::{SusieRssNode, SusieRssNodeFactory, SusieRssSpec};
pub use two_sample_mr::{
    TwoSampleMrNode, TwoSampleMrNodeFactory, TwoSampleMrNodeSpec, TwoSampleMrParameters,
};
pub use univariate_mixer::{
    UnivariateMixerNode, UnivariateMixerNodeFactory, UnivariateMixerNodeSpec,
};
pub use viz::{VizNode, VizNodeFactory, VizNodeSpec};

// Survey-package nodes (stub + codegen_r phase; Rust execute TBD).
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
