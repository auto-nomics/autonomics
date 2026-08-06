//! # agentik-network — declarative multi-agent network topology engine.
//!
//! Defines a multi-agent system as a **graph spec** (nodes + edges +
//! termination conditions) and drives it with a single multiplexing
//! conductor. Agents remain isolated actors — they receive user-style
//! messages and emit [`AgentEvent`](agentik_sdk::types::AgentEvent)s as
//! usual. The conductor routes messages between them based on the declared
//! topology.
//!
//! ## Topology primitives
//!
//! - **Node** — one agent (identified by name, backed by an
//!   [`AgentProfile`](agentik_core::AgentProfile)).
//! - **Edge** — a directed connection from one node to another, with a
//!   trigger that determines *when* a message is forwarded.
//! - **Termination** — declares when the entire network should stop.
//!
//! ## Example: adversarial review loop (arena)
//!
//! ```ignore
//! use agentik_network::{NetworkSpec, NodeSpec, EdgeSpec, EdgeTrigger, TerminationSpec};
//!
//! let spec = NetworkSpec {
//!     name: "manuscript-arena".into(),
//!     nodes: vec![
//!         NodeSpec {
//!             name: "researcher".into(),
//!             profile: "researcher".into(),
//!             initial_prompt: Some("Write a manuscript abstract.".into()),
//!         },
//!         NodeSpec {
//!             name: "reviewer".into(),
//!             profile: "reviewer".into(),
//!             initial_prompt: None,
//!         },
//!     ],
//!     edges: vec![
//!         EdgeSpec {
//!             from: "researcher".into(),
//!             to: "reviewer".into(),
//!             trigger: EdgeTrigger::OnDone,
//!             transform: None,
//!         },
//!         EdgeSpec {
//!             from: "reviewer".into(),
//!             to: "researcher".into(),
//!             trigger: EdgeTrigger::OnDone,
//!             transform: None,
//!         },
//!     ],
//!     termination: TerminationSpec::Any(vec![
//!         TerminationSpec::Condition {
//!             node: "reviewer".into(),
//!             pattern: "ACCEPT".into(),
//!         },
//!         TerminationSpec::MaxRounds(5),
//!     ]),
//! };
//! ```

pub mod conductor;
pub mod error;
pub mod event;
pub mod presets;
pub mod spec;

pub use conductor::{AgentNetwork, NetworkOutcome, TerminationReason};
pub use error::NetworkError;
pub use event::NetworkEvent;
pub use spec::{EdgeSpec, EdgeTrigger, NetworkSpec, NodeSpec, TerminationSpec, TransformSpec};
