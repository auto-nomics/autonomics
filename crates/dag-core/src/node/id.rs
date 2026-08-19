//! Identifier aliases shared by the node and graph models.

/// Unique identifier for a node in the DAG.
pub type NodeId = String;

/// Numeric identifier for an individual port on a node.
///
/// Ports are indexed sequentially starting from 0. An edge in the DAG connects
/// one `(node, PortId)` pair on the output side to another `(node, PortId)`
/// pair on the input side.
pub type PortId = u8;

/// The default port name used when a single-input/single-output node does not
/// name its ports explicitly.
pub const DEFAULT_PORT: &str = "default";
