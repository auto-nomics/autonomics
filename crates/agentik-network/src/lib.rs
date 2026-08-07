//! # agentik-network — persistent mutable multi-agent topology routing.
//!
//! [`AgentNetwork`] is a long-lived topology manager (same lifetime as
//! [`RuntimeHost`](runtime::RuntimeHost)). It owns a mutable
//! [`NetworkGraph`] (petgraph-backed) that supports dynamic node/edge
//! operations at runtime. Routing decisions are computed by
//! [`process_event`](AgentNetwork::process_event), which returns
//! [`RoutingAction`]s for the host to execute.
//!
//! ## Topology primitives
//!
//! - **Node** — one agent (identified by name, backed by a profile).
//! - **Edge** — a directed connection with a trigger condition.
//! - **Termination** — declares when a run should stop.

pub mod event;
pub mod graph;
pub mod router;
pub mod spec;

pub use event::NetworkEvent;
pub use graph::NetworkGraph;
pub use router::{
    edge_should_fire, check_termination_spec, AgentNetwork, NetworkOutcome, RoutingAction,
    TerminationReason,
};
pub use spec::{EdgeSpec, EdgeTrigger, NetworkSpec, NodeSpec, TerminationSpec, TransformSpec};
