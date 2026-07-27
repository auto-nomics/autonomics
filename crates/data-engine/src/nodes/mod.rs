//! Node abstractions and built-in implementations.
//!
//! [`meta`] defines the [`DagNode`] trait, [`NodePorts`], and [`NodeInput`] —
//! the contract every node fulfils. Concrete implementations live in
//! [`source_file`], [`source_iceberg`], [`sql_node`], [`sink_file`], and
//! [`sink_iceberg`].

pub mod bivariate_mixer;
pub mod echo_node;
pub mod ldsc_hsq;
pub mod ldsc_rg;
pub mod lava;
pub mod liability;
pub mod linear_regression;
pub mod meta;
pub mod mr;
pub mod sink_common;
pub mod sink_file;
pub mod sink_iceberg;
pub mod source_file;
pub mod source_iceberg;
pub mod sql_node;
pub mod test_source;
pub mod univariate_mixer;
pub mod viz;

pub use bivariate_mixer::{BivariateMixerNode, BivariateMixerNodeFactory, BivariateMixerNodeSpec};
pub use echo_node::{EchoNode, EchoNodeFactory, EchoNodeSpec};
pub use ldsc_hsq::{LdscHsqConfig, LdscHsqNode, LdscHsqNodeFactory};
pub use ldsc_rg::{LdscRgConfig, LdscRgNode, LdscRgNodeFactory};
pub use lava::{
    LavaBivarNode, LavaBivarNodeFactory, LavaLocusNode, LavaLocusNodeFactory, LavaMultiregNode,
    LavaMultiregNodeFactory, LavaPcorNode, LavaPcorNodeFactory, LavaUnivNode, LavaUnivNodeFactory,
};
pub use liability::{LiabilityConfig, LiabilityNode, LiabilityNodeFactory};
pub use linear_regression::{
    LinearRegressionNode, LinearRegressionNodeFactory, LinearRegressionNodeSpec,
};
pub use meta::{DEFAULT_PORT, DagNode, NodeId, NodeInput, NodePorts, Port};
pub use mr::{MrNode, MrNodeFactory, MrNodeSpec, MrParameters};
pub use sink_common::SinkMode;
pub use sink_file::{FileSinkNode, FileSinkNodeFactory, FileSinkNodeSpec, WriteFormat};
pub use sink_iceberg::{IcebergSinkNode, IcebergSinkNodeFactory, IcebergSinkNodeSpec};
pub use source_file::{FileFormat, FileSourceNode, FileSourceNodeFactory, FileSourceNodeSpec};
pub use source_iceberg::{IcebergSourceNode, IcebergSourceNodeFactory, IcebergSourceNodeSpec};
pub use sql_node::{SqlNode, SqlNodeFactory, SqlNodeSpec};
pub use test_source::{TestSourceFactory, TestSourceNode, TestSourceSpec};
pub use univariate_mixer::{
    UnivariateMixerNode, UnivariateMixerNodeFactory, UnivariateMixerNodeSpec,
};
pub use viz::{VizNode, VizNodeFactory, VizNodeSpec};
