//! Node abstractions and built-in implementations.
//!
//! [`meta`] defines the [`DagNode`] trait, [`NodeMeta`], and [`NodeInput`] —
//! the contract every node fulfils. Concrete implementations live in
//! [`source`], [`sql_node`], and [`sink`].

pub mod bivariate_mixer;
pub mod ldsc_hsq;
pub mod linear_regression;
pub mod meta;
pub mod mock_node;
pub mod sink;
pub mod source;
pub mod sql_node;
pub mod univariate_mixer;

pub use bivariate_mixer::{
    BivariateMixerNode, BivariateMixerNodeFactory, BivariateMixerNodeSpec,
};
pub use ldsc_hsq::{LdscHsqConfig, LdscHsqNode, LdscHsqNodeFactory};
pub use linear_regression::{
    LinearRegressionNode, LinearRegressionNodeFactory, LinearRegressionNodeSpec,
};
pub use meta::{DEFAULT_PORT, DagNode, NodeId, NodeInput, NodeMeta, Port};
pub use mock_node::{MockNodeFactory, MockNodeSpec};
pub use sink::{Sink, SinkMode, SinkNode, SinkNodeFactory, SinkNodeSpec, WriteFormat};
pub use source::{FileFormat, Source, SourceNode, SourceNodeFactory, SourceNodeSpec};
pub use univariate_mixer::{
    UnivariateMixerNode, UnivariateMixerNodeFactory, UnivariateMixerNodeSpec,
};
pub use sql_node::{SqlNode, SqlNodeFactory, SqlNodeSpec};
