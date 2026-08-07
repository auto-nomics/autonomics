//! # agentik-network — pure multi-agent topology routing.
//!
//! Defines a multi-agent system as a **graph spec** (nodes + edges +
//! termination conditions) and provides [`AgentNetwork`] — a pure state
//! machine that routes messages between agents based on the declared
//! topology.
//!
//! **No I/O.** [`AgentNetwork`] does not own agent handles, spawn tasks,
//! or perform any async operations. It takes `(agent_name, &AgentEvent)`
//! as input and produces `Vec<`[`RoutingAction`]`>` as output. The caller
//! (typically a runtime host) is responsible for executing the actions.
//!
//! ## Topology primitives
//!
//! - **Node** — one agent (identified by name, backed by a profile).
//! - **Edge** — a directed connection from one node to another, with a
//!   trigger that determines *when* a message is forwarded.
//! - **Termination** — declares when the entire network should stop.
//!
//! ## Example
//!
//! ```ignore
//! use agentik_network::{AgentNetwork, NetworkSpec, NodeSpec, EdgeSpec, EdgeTrigger, TerminationSpec};
//!
//! let spec = NetworkSpec {
//!     name: "review-loop".into(),
//!     nodes: vec![
//!         NodeSpec { name: "writer".into(), profile: "writer".into(),
//!                    initial_prompt: Some("Write an abstract.".into()) },
//!         NodeSpec { name: "reviewer".into(), profile: "reviewer".into(),
//!                    initial_prompt: None },
//!     ],
//!     edges: vec![
//!         EdgeSpec { from: "writer".into(), to: "reviewer".into(),
//!                    trigger: EdgeTrigger::OnDone, transform: None },
//!         EdgeSpec { from: "reviewer".into(), to: "writer".into(),
//!                    trigger: EdgeTrigger::OnDone, transform: None },
//!     ],
//!     termination: TerminationSpec::Any { specs: vec![
//!         TerminationSpec::Condition { node: "reviewer".into(), pattern: "ACCEPT".into() },
//!         TerminationSpec::MaxRounds { max: 10 },
//!     ]},
//! };
//!
//! let mut net = AgentNetwork::new(spec)?;
//! // Feed events, get routing actions...
//! ```

pub mod event;
pub mod presets;
pub mod router;
pub mod spec;

pub use event::NetworkEvent;
pub use router::{edge_should_fire, check_termination_spec, AgentNetwork, NetworkOutcome, RoutingAction, TerminationReason};
pub use spec::{EdgeSpec, EdgeTrigger, NetworkSpec, NodeSpec, TerminationSpec, TransformSpec};
