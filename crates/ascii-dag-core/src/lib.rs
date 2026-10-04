//! # ascii-dag
//!
//! Graph layout engine that renders to text — DAGs, and cyclic graphs
//! via dashed back edges — for error chains, build systems, and
//! dependency visualization.
//!
//! ## Features
//!
//! - **Small**: ~94 KB WASM no-alloc build (41 KB gzipped); see BENCHMARK.md
//! - **Fast**: Cached adjacency lists, O(1) lookups, banded zero-copy rendering
//! - **no_std + no-alloc**: the arena pipeline runs without a heap allocator
//! - **Modular**: Each component can be used independently
//! - **Safe**: Cycle detection built-in
//!
//! ## Performance
//!
//! - **Cached Adjacency Lists**: O(1) child/parent lookups (not O(E))
//! - **Zero Allocations**: Direct buffer writes with `write_node()`
//! - **HashMap Indexing**: O(1) ID→index instead of O(N) scans
//!
//! ## Feature Flags
//!
//! - `std` (default): Standard library support
//! - `generic` (default): Generic algorithms over your own types (cycle
//!   detection, topological sort, impact analysis, metrics). Implies `std` —
//!   these keep visited sets keyed by the caller's id type, bounded
//!   `Eq + Hash`, so they need a real `HashSet`.
//! - `alloc`: Heap-based `Graph` API without `std`
//! - `arena` (+ `arena-idx-u8`/`u16`/`u32`): No-alloc CSR layout and
//!   rendering on caller-provided arenas
//! - `ports` (default): declared edge attachment sides and their per-face
//!   positioning; off, attachment is the port-free rule and nothing of the
//!   machinery is linked (the embedded examples build without it)
//!
//! Non-fatal conditions (auto-created placeholders, omitted labels)
//! travel through the [`diagnostics`] channel as typed data — there
//! is no logging feature and the library never writes to stderr.
//!
//! To minimize bundle size, disable `generic`:
//! ```toml
//! ascii-dag = { version = "0.10", default-features = false, features = ["std"] }
//! ```
//!
//! ## Quick Start
//!
//! ```rust
//! use ascii_dag::graph::{Graph, RenderMode};
//!
//! // Batch construction (fast!)
//! let dag = Graph::from_edges(
//!     &[(1, "Error1"), (2, "Error2"), (3, "Error3")],
//!     &[(1, 2), (2, 3)]
//! );
//!
//! println!("{}", dag.render());
//! ```
//!
//! ## Ports
//!
//! An edge can declare which side of a node it leaves from and which
//! side it arrives on, on the handle [`Graph::add_edge`](graph::Graph::add_edge)
//! returns (or by index with `set_edge_ports`); the layout reports the
//! side each end actually took on every IR edge:
//!
//! ```rust
//! # #[cfg(feature = "ports")] {
//! use ascii_dag::{Graph, PhysicalSide, PortSide};
//!
//! let mut g = Graph::new();
//! g.add_node(1usize, "Gateway");
//! g.add_node(2usize, "Service");
//! g.add_node(3usize, "Cache");
//! g.add_edge(1usize, 2usize, None);
//! g.add_edge(1usize, 3usize, None).to_port(PortSide::West);
//!
//! let ir = g.compute_layout();
//! let cache = ir.edges().iter().find(|e| e.to_id == 3).unwrap();
//! assert_eq!(cache.to_port.requested, PortSide::West);
//! assert_eq!(cache.to_port.side, PhysicalSide::West);
//! # }
//! ```
//!
//! [`PortSide`] names a side three ways. Compass sides (`North`,
//! `East`, `South`, `West`) are fixed on the page. Flow sides follow
//! the direction: `Upstream` is the face the flow arrives on,
//! `Downstream` the face it leaves by. Rotations follow it too:
//! `Clockwise` is the traveler's right hand facing downstream,
//! `Counterclockwise` the left. `Auto` (the default) is head-on:
//! leave `Downstream`, arrive `Upstream`.
//!
//! | Side | `TopDown` | `BottomUp` | `LeftRight` | `RightLeft` |
//! |---|---|---|---|---|
//! | `Upstream` | North | South | West | East |
//! | `Downstream` | South | North | East | West |
//! | `Clockwise` | West | East | South | North |
//! | `Counterclockwise` | East | West | North | South |
//!
//! A face has one port by default, shared by every edge declared on
//! it; a [`PortPolicy`] — the graph's (`set_port_policy`) or one
//! node's (`set_node_port_policy`) — chooses `Paired` (an arrival and
//! a departure port), `Spread` (up to a [`PortBound`]) or `Custom` (the
//! [`PortPlacer`] registered with `set_port_placer`) instead. A node is never widened for its
//! ports, and a face with one cell holds one port whatever the policy.
//! Ends the layout could not honor are warnings on the run
//! (`W.Graph.Port.034` for a side on a self-loop, `W.Graph.Port.035`
//! when no lane beside the node was free). The guide:
//! `docs/ports.md`; runnable: `examples/ports.rs`, every section
//! through both pipelines.
//!
//! ## Modular Design
//!
//! The library is organized into separate, independently-usable modules:
//!
//! ### [`graph`] - Core DAG Structure
//! ```rust
//! use ascii_dag::graph::Graph;
//!
//! let mut dag = Graph::new();
//! dag.add_node(1, "A");
//! dag.add_node(2, "B");
//! dag.add_edge(1, 2, None);
//! ```
//!
//! ### Cycle Detection
//! ```rust
//! use ascii_dag::graph::Graph;
//!
//! let mut dag = Graph::new();
//! dag.add_edge(1, 2, None);
//! dag.add_edge(2, 1, None);
//! assert!(dag.has_cycle());
//! ```
//!
//! ### Generic Cycle Detection
//! Works with any data structure via higher-order functions:
//! ```rust
//! # #[cfg(feature = "generic")]
//! # {
//! use ascii_dag::algorithms::cycles::generic::detect_cycle_fn;
//!
//! let get_deps = |id: &usize| match id {
//!     1 => vec![2],
//!     2 => vec![3],
//!     _ => vec![],
//! };
//!
//! let cycle = detect_cycle_fn(&[1, 2, 3], get_deps);
//! assert!(cycle.is_none());
//! # }
//! ```
//!
//! ### Generic Topological Sorting
//! Sort any dependency graph into execution order:
//! ```rust
//! # #[cfg(feature = "generic")]
//! # {
//! use ascii_dag::algorithms::generic::topological_sort_fn;
//!
//! let get_deps = |task: &&str| match *task {
//!     "deploy" => vec!["build"],
//!     "build" => vec!["compile"],
//!     "compile" => vec![],
//!     _ => vec![],
//! };
//!
//! let sorted = topological_sort_fn(&["deploy", "compile", "build"], get_deps).unwrap();
//! // Result: ["compile", "build", "deploy"]
//! assert_eq!(sorted[0], "compile");
//! # }
//! ```
//!
//! ### Graph Layout
//! Sugiyama hierarchical layout for positioning nodes.
//!
//! ### ASCII Rendering
//! Vertical, horizontal, and cycle visualization modes.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
// Allow certain clippy warnings that are cosmetic or intentional:
// - too_many_arguments: Some internal functions have many params for performance
// - unnecessary_cast: Casts like `x as usize` are kept for clarity when index types vary
// - needless_range_loop: Some loops index multiple arrays making iterators awkward
// - collapsible_if: Some nested ifs are more readable uncollapsed
#![allow(clippy::too_many_arguments)]
#![allow(clippy::unnecessary_cast)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_else_if)]

#[cfg(feature = "alloc")]
extern crate alloc;

// ── Module hierarchy ─────────────────────────────────────────────────────
//
//   graph/          DAG struct + arena allocator + CSR graph
//   algorithms/     cycles, generic analysis, Sugiyama layout
//   ir/             layout intermediate representation
//   render/         unified render engine + glyph/palette utilities

#[cfg(not(any(feature = "layout-vertical", feature = "layout-horizontal")))]
compile_error!(
    "ascii-dag: enable at least one layout axis feature — \
     `layout-vertical` (TopDown/BottomUp) and/or `layout-horizontal` \
     (LeftRight/RightLeft). Both are in the default feature set."
);

pub mod algorithms;
pub mod diagnostics;
pub mod errors;
pub mod graph;
pub mod ir;
pub mod render;

// ── Convenience re-exports (alloc-dependent) ────────────────────────────

pub use diagnostics::{
    BorrowedReport, CountingDiagnostics, Diagnostic, DiagnosticContext, DiagnosticCounts,
    DiagnosticKind, DiagnosticRef, DiagnosticRun, DiagnosticSink, DiagnosticSubject, FnDiagnostics,
    IgnoreDiagnostics, ProjectedFailure, Report, Severity, SliceDiagnostics,
};
#[cfg(feature = "alloc")]
pub use diagnostics::{OwnedReport, VecDiagnostics};
#[cfg(feature = "alloc")]
pub use errors::ErrorChain;
pub use errors::GraphError;
pub use graph::RenderMode;
pub use graph::{AUTO, Auto, IdOrAuto, NodeId};
#[cfg(feature = "alloc")]
pub use graph::{
    Direction, EdgeHandle, EdgeInsertion, Graph, MissingNodePolicy, NodeInsertion, Subgraph,
};
#[cfg(feature = "alloc")]
pub use ir::{EdgePath, FlowAxis, LayoutEdge, LayoutIR, LayoutIRBuilder, LayoutNode, SubgraphInfo};
pub use render::colors::Palette;
pub use render::engine::{
    ArmWeight, ArmWeights, CellKind, CellMarker, CellView, CompositionRequirements,
    MarkerDirection, SceneComposer, TerminalRenderer,
};
pub use render::engine::{BoxedNode, CustomNode, NodeContent, SimpleNode};
pub use render::engine::{CellColor, HitResult};
pub use render::engine::{
    Charset, ColorMode, ComposeBudget, EmitOptions, LabelOverflow, LabelPlacementPolicy,
    LabelPolicy, LayoutSource, PlanOptions, PlanRun, RenderOptions, Scene, ScenePlanner,
};
pub use render::engine::{
    EdgePathView, EdgeView, LabelSlot, LabelView, NodeKind, NodeOrigin, NodeView, SubgraphView,
};
pub use render::engine::{LabelPosition, LineWeight, MarkerShape, SubgraphBorder};
// Primary config types (always available, no alloc needed)
pub use algorithms::sugiyama::config::{
    AlgorithmConfig, CycleBreaking, Layering, LayoutConfig, Positioning, Routing,
};
pub use algorithms::sugiyama::crossing::{CrossingReducer, FAST, QUALITY, STANDARD};
pub use algorithms::sugiyama::ports::{
    EdgeEnd, PhysicalSide, Port, PortAttachment, PortBound, PortPlacer, PortPolicy, PortSide,
    PortSlot,
};
