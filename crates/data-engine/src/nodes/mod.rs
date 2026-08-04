//! Node abstractions and built-in implementations.
//!
//! [`meta`] defines the [`DagNode`] trait, [`NodePorts`], and [`NodeInput`] —
//! the contract every node fulfils. Concrete implementations live in
//! [`source_file`], [`source_iceberg`], [`sql_node`], [`sink_file`], and
//! [`sink_iceberg`].

pub mod bivariate_mixer;
pub mod causal;
pub mod chi_square;
pub mod cmest;
pub mod cox_regression;
pub mod cpassoc;
pub mod echo_node;
pub mod epi_lasso;
pub mod epi_rcs;
pub mod hypothesize;
pub mod epi_roc;
pub mod epi_wqs;
pub mod hdl_l;
pub mod hdl_l_scan;
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
pub mod mtag;
pub mod numeric_util;
pub mod sink_common;
pub mod sink_file;
pub mod sink_iceberg;
pub mod source_file;
pub mod source_iceberg;
pub mod source_opentargets;
pub mod sql_node;
pub mod survival;
pub mod susie_rss;
pub mod two_sample_mr;
pub mod univariate_mixer;
pub mod viz;

pub use bivariate_mixer::{BivariateMixerNode, BivariateMixerNodeFactory, BivariateMixerNodeSpec};
pub use chi_square::{ChiSquareNode, ChiSquareNodeFactory, ChiSquareNodeSpec};
pub use cpassoc::{CpassocConfig, CpassocNode, CpassocNodeFactory};
pub use echo_node::{EchoNode, EchoNodeFactory, EchoNodeSpec};
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
pub use mtag::{MtagConfig, MtagNode, MtagNodeFactory};
pub use sink_common::SinkMode;
pub use sink_file::{FileSinkNode, FileSinkNodeFactory, FileSinkNodeSpec, WriteFormat};
pub use sink_iceberg::{IcebergSinkNode, IcebergSinkNodeFactory, IcebergSinkNodeSpec};
pub use source_file::{FileFormat, FileSourceNode, FileSourceNodeFactory, FileSourceNodeSpec};
pub use source_iceberg::{IcebergSourceNode, IcebergSourceNodeFactory, IcebergSourceNodeSpec};
pub use source_opentargets::{
    OpentargetsAssociationsNode, OpentargetsAssociationsNodeFactory, OpentargetsAssociationsSpec,
    OpentargetsSearchNode, OpentargetsSearchNodeFactory, OpentargetsSearchSpec,
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
