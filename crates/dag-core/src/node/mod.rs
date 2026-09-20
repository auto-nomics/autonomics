//! Per-node metadata, ports, the typed input envelope, and the [`DagNode`] trait.
//!
//! The node model is split by responsibility:
//!
//! - [`id`]: stable node and port identifier aliases.
//! - [`port`] and [`port_set`]: individual ports and ordered port collections.
//! - [`layout`]: a node's input/output port layout.
//! - [`input`]: the typed envelope injected into a node during execution.
//! - [`node_trait`]: the contract implemented by every DAG node.
//! - [`arrow`]: shared helpers for node implementations that consume Arrow data.
//!
//! Each node declares a set of input ports and output ports. An edge in the DAG
//! connects exactly one upstream output port to one downstream input port and
//! carries exactly one value. At execution time the scheduler injects one
//! [`NodeInput`] per connected input port, tagged with the port index so the node
//! knows which slot each value belongs to.

pub mod arrow;
pub mod data_bundle;
pub mod id;
pub mod input;
pub mod layout;
pub mod node_trait;
pub mod port;
pub mod port_set;

pub use arrow::string_opt_values;
pub use data_bundle::{
    BundleRegistry, BundleRegistryError, DataBundle, DataBundleBinding, ResolvedDataBundle,
};
pub use id::{DEFAULT_PORT, NodeId, PortId};
pub use input::NodeInput;
pub use layout::NodePorts;
pub use node_trait::DagNode;
pub use port::Port;
pub use port_set::Ports;

#[cfg(test)]
pub(crate) mod test_support;
