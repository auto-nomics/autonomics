//! Core graph data structure.
//!
//! This module provides the fundamental graph structure with nodes and edges.
//! The primary type is `Graph` (feature `alloc`).
//!
//! ## Performance Characteristics
//!
//! - **Node/Edge Insertion**: O(1) amortized with HashMap and cached adjacency lists
//! - **Child/Parent Lookups**: O(1) via cached adjacency lists (not O(E) iteration)
//! - **ID→Index Mapping**: O(1) via HashMap (not O(N) scan)
//! - **Node Width**: O(1) via pre-computed cache
//!
//! ## Memory Overhead
//!
//! Per node:
//! - ~100 bytes (node data, caches, adjacency list headers)
//!
//! Per edge:
//! - ~16 bytes (adjacency list entries, both directions)
//!
//! ## Security
//!
//! - No unsafe code
//! - For untrusted input, consider limiting maximum nodes/edges to prevent resource exhaustion
//! - Maximum node ID: `usize::MAX` (up to 20 decimal digits)

pub mod arena;

/// Rendering mode for the DAG visualization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderMode {
    /// Render chains vertically (takes more vertical space)
    Vertical,

    /// Render chains horizontally when possible (compact, one-line for simple chains)
    Horizontal,

    /// Auto-detect: horizontal for simple chains, vertical for complex graphs
    #[default]
    Auto,
}

/// Rank direction — the axis levels flow along.
///
/// Recorded on the layout IR; IR coordinates are physical (they match
/// rendered cells). For `BottomUp`, the heap layout path flips its
/// result into physical coordinates.
///
/// Parses from the conventional short forms (case-insensitive):
/// `"TB"`/`"TD"`, `"BT"`, `"LR"`, `"RL"`.
///
/// All four directions are laid out natively and painted through the
/// same geometry-driven primitives. `TopDown`/`BottomUp` stack levels
/// as rows; `LeftRight`/`RightLeft` make levels COLUMNS — sized by
/// node widths, with edges running in horizontal trunks — so a wide,
/// shallow graph reads better sideways. `BottomUp` and `RightLeft`
/// are exact mirrors of their counterparts, applied to the finished
/// layout, so IR coordinates always match rendered cells.
/// Variants are gated by the axis features: `TopDown`/`BottomUp` exist
/// under `layout-vertical`, `LeftRight`/`RightLeft` under
/// `layout-horizontal` (both are default features). A disabled
/// direction is a compile error, never a runtime fallback. The enum is
/// `#[non_exhaustive]` so a feature union adding variants can never
/// break a downstream exhaustive `match` — always keep a wildcard arm.
///
/// The default is the first enabled axis, vertical before horizontal:
/// both/vertical-only → `TopDown`, horizontal-only → `LeftRight`.
/// It re-resolves with the feature set at **compile time**: code that
/// relies on the default automatically picks up the other axis's
/// default when the enabled axes change — no code changes needed. The
/// flip side: a dependency's feature union that enables an axis your
/// crate did not ask for silently changes the resolved default (and
/// with it the rendered orientation). That is why libraries should set
/// a direction explicitly; the default is an application-level
/// convenience.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Direction {
    /// Levels flow top → bottom (default; edges point down).
    #[cfg(feature = "layout-vertical")]
    TopDown,
    /// Levels flow bottom → top (edges point up).
    #[cfg(feature = "layout-vertical")]
    BottomUp,
    /// Levels flow left → right (edges point right).
    #[cfg(feature = "layout-horizontal")]
    LeftRight,
    /// Levels flow right → left (edges point left).
    #[cfg(feature = "layout-horizontal")]
    RightLeft,
}

impl Direction {
    /// The feature-dependent default (first enabled axis, vertical
    /// before horizontal) as a `const` — usable in `const fn` preset
    /// constructors, and what `Default` delegates to. Resolved at
    /// compile time from the enabled features: change the axis set and
    /// every default-relying call site follows automatically.
    #[cfg(feature = "layout-vertical")]
    pub const DEFAULT: Direction = Direction::TopDown;
    /// The feature-dependent default (first enabled axis, vertical
    /// before horizontal) as a `const` — usable in `const fn` preset
    /// constructors, and what `Default` delegates to. Resolved at
    /// compile time from the enabled features: change the axis set and
    /// every default-relying call site follows automatically.
    #[cfg(all(feature = "layout-horizontal", not(feature = "layout-vertical")))]
    pub const DEFAULT: Direction = Direction::LeftRight;
}

impl Default for Direction {
    fn default() -> Self {
        Direction::DEFAULT
    }
}

/// Compile-time guidance marker: if this type shows up in one of your
/// build errors, the direction you named exists but is behind a
/// disabled cargo feature of ascii-dag — the accompanying deprecation
/// note names the feature to enable.
#[cfg(not(all(feature = "layout-vertical", feature = "layout-horizontal")))]
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct DisabledDirectionSeeDeprecationNote;

/// Guidance shims: same names as the feature-disabled variants, so a
/// use site resolves to THIS const instead of dying with a bare
/// "variant not found" — the type error plus the deprecation note tell
/// the user exactly which feature to enable.
#[cfg(not(feature = "layout-horizontal"))]
#[doc(hidden)]
#[allow(non_upper_case_globals)] // shim names must match the gated variants
impl Direction {
    #[deprecated = "`Direction::LeftRight` needs ascii-dag's `layout-horizontal` cargo feature — enable it (it is in the default feature set)"]
    pub const LeftRight: DisabledDirectionSeeDeprecationNote = DisabledDirectionSeeDeprecationNote;
    #[deprecated = "`Direction::RightLeft` needs ascii-dag's `layout-horizontal` cargo feature — enable it (it is in the default feature set)"]
    pub const RightLeft: DisabledDirectionSeeDeprecationNote = DisabledDirectionSeeDeprecationNote;
}

#[cfg(not(feature = "layout-vertical"))]
#[doc(hidden)]
#[allow(non_upper_case_globals)] // shim names must match the gated variants
impl Direction {
    #[deprecated = "`Direction::TopDown` needs ascii-dag's `layout-vertical` cargo feature — enable it (it is in the default feature set)"]
    pub const TopDown: DisabledDirectionSeeDeprecationNote = DisabledDirectionSeeDeprecationNote;
    #[deprecated = "`Direction::BottomUp` needs ascii-dag's `layout-vertical` cargo feature — enable it (it is in the default feature set)"]
    pub const BottomUp: DisabledDirectionSeeDeprecationNote = DisabledDirectionSeeDeprecationNote;
}

/// Error returned when parsing a [`Direction`] from an unknown string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseDirectionError;

impl core::fmt::Display for ParseDirectionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        #[cfg(all(feature = "layout-vertical", feature = "layout-horizontal"))]
        return f.write_str("unknown direction (expected TB/TD, BT, LR, or RL)");
        #[cfg(all(feature = "layout-vertical", not(feature = "layout-horizontal")))]
        return f.write_str(
            "unknown direction (expected TB/TD or BT; LR/RL need ascii-dag's \
             `layout-horizontal` feature)",
        );
        #[cfg(all(feature = "layout-horizontal", not(feature = "layout-vertical")))]
        return f.write_str(
            "unknown direction (expected LR or RL; TB/TD/BT need ascii-dag's \
             `layout-vertical` feature)",
        );
    }
}

impl core::str::FromStr for Direction {
    type Err = ParseDirectionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Untyped input is rejected at this boundary; a string naming a
        // feature-disabled axis parses like any unknown string (the
        // typed variant does not exist to return).
        #[cfg(feature = "layout-vertical")]
        if s.eq_ignore_ascii_case("TB") || s.eq_ignore_ascii_case("TD") {
            return Ok(Direction::TopDown);
        }
        #[cfg(feature = "layout-vertical")]
        if s.eq_ignore_ascii_case("BT") {
            return Ok(Direction::BottomUp);
        }
        #[cfg(feature = "layout-horizontal")]
        if s.eq_ignore_ascii_case("LR") {
            return Ok(Direction::LeftRight);
        }
        #[cfg(feature = "layout-horizontal")]
        if s.eq_ignore_ascii_case("RL") {
            return Ok(Direction::RightLeft);
        }
        Err(ParseDirectionError)
    }
}

// Everything below requires the `alloc` feature (Vec, String, HashMap).
#[cfg(feature = "alloc")]
use alloc::{string::String, vec, vec::Vec};

#[cfg(feature = "alloc")]
use crate::algorithms::sugiyama::config::LayoutConfig;
#[cfg(feature = "alloc")]
#[allow(deprecated)]
use crate::algorithms::sugiyama::config::SugiyamaConfig;
#[cfg(feature = "alloc")]
use crate::algorithms::sugiyama::crossing::CrossingReducer;
#[cfg(all(feature = "alloc", feature = "ports"))]
use crate::algorithms::sugiyama::ports::{Port, PortPolicy, PortSide};
#[cfg(feature = "alloc")]
use crate::errors::GraphError;
#[cfg(feature = "alloc")]
use crate::render::engine::{NodeContent, NodeKindTag, NodePaintFn};

/// A named subgraph (cluster) for visual grouping.
///
/// Subgraphs define logical clusters that the layout engine renders with
/// box-drawing borders.  They can be nested (a subgraph may have a parent).
///
/// # Zero-Cost When Unused
///
/// If no subgraphs are added, the layout engine skips all subgraph-related
/// processing — there is no per-node overhead.
///
/// # Examples
///
/// ```
/// use ascii_dag::graph::Graph;
///
/// let mut g = Graph::new();
/// g.add_node(1, "Server");
/// g.add_node(2, "Database");
/// g.add_edge(1, 2, None);
///
/// let backend = g.add_subgraph("Backend");
/// g.put_nodes(&[1, 2]).inside(backend).unwrap();
/// ```
#[cfg(feature = "alloc")]
#[derive(Clone, Debug)]
pub struct Subgraph<'a> {
    /// Unique identifier (assigned by [`Graph::add_subgraph`]).
    pub id: usize,
    /// Display label rendered on the top border.
    pub label: &'a str,
    /// Parent subgraph ID for nesting (`None` = top-level).
    pub parent_id: Option<usize>,
}

#[cfg(all(feature = "alloc", feature = "std"))]
use std::collections::{HashMap, HashSet};

#[cfg(all(feature = "alloc", not(feature = "std")))]
use alloc::collections::{BTreeMap as HashMap, BTreeSet as HashSet};

// ── Node handles & the AUTO sentinel ─────────────────────────────────────

/// A typed handle to a node, returned by `Graph::add_node`.
///
/// Handles flow back into the edge and subgraph APIs (which accept
/// `impl Into<NodeId>`), so graphs can be built end-to-end without
/// hand-tracked integer ids:
///
/// ```
/// use ascii_dag::{Graph, AUTO};
///
/// let mut g = Graph::new();
/// let a = g.add_node(AUTO, "A");
/// let b = g.add_node(AUTO, "B");
/// g.add_edge(a, b, None);
/// ```
///
/// Raw `usize` ids keep working everywhere — `NodeId` is the safer
/// *recommended* path, not a fence: it converts from and to `usize`
/// freely and **carries no graph provenance** — a handle obtained from
/// graph A is accepted by graph B, where it names whatever node has
/// that id there (possibly none, in which case edge endpoints
/// auto-create a placeholder as raw ids always have).
///
/// The handle vocabulary (`NodeId`, [`Auto`]/[`AUTO`], [`IdOrAuto`])
/// is allocation-free and available in every build, `no_std`
/// included — a no-alloc component can produce and pass handles
/// (e.g. `(NodeId, NodeId)` edge pairs) for an alloc-enabled host to
/// assemble into a `Graph`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(usize);

impl NodeId {
    /// The raw id this handle names.
    #[inline]
    pub fn id(self) -> usize {
        self.0
    }
}

impl From<usize> for NodeId {
    #[inline]
    fn from(id: usize) -> Self {
        NodeId(id)
    }
}

impl From<NodeId> for usize {
    #[inline]
    fn from(handle: NodeId) -> usize {
        handle.0
    }
}

/// The type of the [`AUTO`] sentinel — see `Graph::add_node`.
#[derive(Debug, Clone, Copy)]
pub struct Auto;

/// Receipt for one [`Graph::add_edge`] call — always produced, uniform
/// in shape: an edge was always inserted, and the booleans answer the
/// one question the caller cannot already answer (did an endpoint get
/// auto-created?). Reached through the [`EdgeHandle`] `add_edge`
/// returns (which derefs to it). Deliberately NOT `#[must_use]`: a
/// receipt is branchable domain data, not a warning — ignoring it is
/// legitimate, and statement-position calls compile unchanged.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeInsertion {
    /// The new edge's input index (insertion order) — the same
    /// identity style callbacks, diagnostics, and
    /// `EdgeView::input_index` use.
    pub edge: usize,
    /// Whether the source endpoint was auto-created as a placeholder.
    pub created_source: bool,
    /// Whether the target endpoint was auto-created as a placeholder.
    pub created_target: bool,
}

/// What [`Graph::add_edge`] returns: the [`EdgeInsertion`] receipt
/// (through `Deref`, so `.edge` / `.created_*` read directly) plus
/// eager, chainable layout declarations for the edge just inserted.
/// Nothing is deferred — the edge exists before this handle does, and
/// each declaration mutates the graph immediately; the handle is
/// merely the shortest spelling of "…and about that edge". Bind
/// `*handle` (or [`receipt`](Self::receipt)) to keep the `Copy`
/// receipt past the graph borrow.
#[cfg(feature = "alloc")]
pub struct EdgeHandle<'g, 'a> {
    #[cfg_attr(not(feature = "ports"), allow(dead_code))] // read only by the port setters
    graph: &'g mut Graph<'a>,
    receipt: EdgeInsertion,
}

#[cfg(feature = "alloc")]
impl core::ops::Deref for EdgeHandle<'_, '_> {
    type Target = EdgeInsertion;
    #[inline]
    fn deref(&self) -> &EdgeInsertion {
        &self.receipt
    }
}

#[cfg(feature = "alloc")]
impl core::fmt::Debug for EdgeHandle<'_, '_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EdgeHandle")
            .field("receipt", &self.receipt)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "alloc")]
impl EdgeHandle<'_, '_> {
    /// The `Copy` receipt, detached from the graph borrow.
    pub fn receipt(&self) -> EdgeInsertion {
        self.receipt
    }

    /// Declare the side the edge leaves its DECLARED `from` node
    /// from. The layout reports the side it used on the IR edge's
    /// `from_port`; the crate docs' *Ports* section lists the sides.
    ///
    /// The name mirrors `add_edge(from, to)`; clippy's naming
    /// heuristic reads `from_*` as a constructor, which this is not.
    #[cfg(feature = "ports")]
    #[allow(clippy::wrong_self_convention)]
    pub fn from_port(self, port: impl Into<Port>) -> Self {
        self.graph
            .declare_port(self.receipt.edge, 0, port.into().side());
        self
    }

    /// Declare the side the edge arrives at its DECLARED `to` node
    /// on. The layout reports the side it used on the IR edge's
    /// `to_port`.
    ///
    /// Mirrors `add_edge(from, to)`; clippy's heuristic reads `to_*`
    /// as a borrowing conversion, which this is not.
    #[cfg(feature = "ports")]
    #[allow(clippy::wrong_self_convention)]
    pub fn to_port(self, port: impl Into<Port>) -> Self {
        self.graph
            .declare_port(self.receipt.edge, 1, port.into().side());
        self
    }
}

/// How [`Graph::add_edge`] treats an edge endpoint that was never
/// declared. Declaring the policy — even the default — is intent:
/// placeholder creation then stops warning (the [`EdgeInsertion`]
/// receipt remains the record). A rejecting policy is an additive
/// future variant.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingNodePolicy {
    /// The 0.10-compatible behavior: auto-create a placeholder
    /// (`NodeOrigin::EdgeInferred`), recorded in the receipt.
    #[default]
    AutoCreate,
}

/// Receipt for one [`Graph::add_node`] call — always returned,
/// uniform in shape: the node's handle (`AUTO` callers read their
/// assigned id here) plus whether the call replaced an existing node.
/// Deliberately NOT `#[must_use]` (statement-position calls compile
/// unchanged), and it converts into [`NodeId`], so handles keep
/// flowing into `add_edge`/`put_nodes` exactly as before.
///
/// Replace-on-duplicate is the standing semantic;
/// `replaced_involving_auto` marks the variant worth attention — an
/// explicit id overwriting an auto-numbered node, or a saturated
/// `AUTO` overwriting an existing one. That condition is delivered
/// here, at the call site, because a replacement is a point EVENT:
/// it is not derivable from later graph state, and the graph stores
/// no diagnostic history (0.10's `W.Graph.Node.007` stderr warning
/// is this receipt's predecessor).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeInsertion {
    /// The inserted node's handle (the assigned id for `AUTO`).
    pub node: NodeId,
    /// Whether an existing node with this id was replaced.
    pub replaced: bool,
    /// Whether the replacement involved `AUTO` numbering on either
    /// side — the variant worth attention.
    pub replaced_involving_auto: bool,
}

impl From<NodeInsertion> for NodeId {
    #[inline]
    fn from(receipt: NodeInsertion) -> NodeId {
        receipt.node
    }
}

impl From<NodeInsertion> for IdOrAuto {
    #[inline]
    fn from(receipt: NodeInsertion) -> IdOrAuto {
        IdOrAuto::Id(receipt.node.0)
    }
}

impl From<NodeInsertion> for usize {
    #[inline]
    fn from(receipt: NodeInsertion) -> usize {
        receipt.node.0
    }
}

impl NodeInsertion {
    /// The raw id (mirrors [`NodeId::id`]) — AUTO callers read their
    /// assignment here.
    pub fn id(&self) -> usize {
        self.node.0
    }
}

/// A pending layout run — see [`Graph::layout`]. Configure with
/// [`with_config`](Self::with_config), then finish with exactly one
/// terminal.
#[cfg(feature = "alloc")]
#[must_use = "a layout run does nothing until a terminal (.compute / .reported / .quiet) runs it"]
pub struct LayoutRun<'g, 'a> {
    graph: &'g Graph<'a>,
    config: Option<&'g LayoutConfig<'g>>,
}

#[cfg(feature = "alloc")]
impl<'g, 'a> LayoutRun<'g, 'a> {
    /// Use `config` instead of the standard layout configuration.
    pub fn with_config(mut self, config: &'g LayoutConfig<'g>) -> Self {
        self.config = Some(config);
        self
    }

    /// Canonical terminal: replay the graph's mutation diagnostics
    /// into `diagnostics`, then compute the layout.
    pub fn compute(
        self,
        diagnostics: &mut crate::diagnostics::DiagnosticContext<'_>,
    ) -> crate::ir::LayoutIR<'a> {
        // Standing conditions, re-derived from the graph per run —
        // nothing is stored and nothing is consumed (like compiler
        // warnings, a condition reports on every run until it is
        // fixed). Deterministic order: implicit placeholders in node
        // insertion order, then the crossing-passes note. Point
        // events (an AUTO-involved replacement, a placeholder's
        // creation moment) belong to their receipts at the call site.
        if self.graph.missing_node_policy.is_none() {
            for &(id, _) in &self.graph.nodes {
                if self.graph.auto_created.contains(&id) {
                    diagnostics
                        .emit(crate::diagnostics::DiagnosticKind::PlaceholderCreated { node: id });
                }
            }
        }
        // The passes note describes the GRAPH-OWNED configuration; a
        // `.with_config(...)` override replaces that configuration
        // for this run, so the note would describe a config the run
        // never uses — it applies only when the graph's own config
        // is the one selected.
        if self.config.is_none() {
            if let Some(note) = self.graph.passes_note {
                diagnostics.emit(note);
            }
        }
        // Port conditions derivable from the graph alone, per run: a
        // declared side on a self-loop is deferred. (Several edges
        // sharing one port cell is the ordinary drawing — every fan-in
        // and fan-out does it — and never a condition.) Edges in input
        // order.
        #[cfg(feature = "ports")]
        let frame = {
            use crate::algorithms::sugiyama::ports::frame;
            let direction = self.config.map_or(self.graph.direction(), |c| c.direction);
            frame(direction)
        };
        #[cfg(feature = "ports")]
        if !self.graph.edge_ports.is_empty() {
            use crate::algorithms::sugiyama::ports::PortSide;
            for (ei, &(from_id, to_id, _)) in self.graph.edges.iter().enumerate() {
                if from_id != to_id {
                    continue;
                }
                let (a, b) = self.graph.edge_ports.get(ei).copied().unwrap_or_default();
                if !matches!(a, PortSide::Auto) || !matches!(b, PortSide::Auto) {
                    diagnostics.emit(crate::diagnostics::DiagnosticKind::PortIgnoredOnSelfLoop {
                        edge: ei,
                    });
                }
            }
        }
        let ir = match self.config {
            Some(config) => self.graph.compute_layout_with_config(config),
            None => self.graph.compute_layout(),
        };
        // The layout's own verdicts: a declared side whose end attached
        // elsewhere (no lane, or the cell was taken) — read back from the
        // attachments the IR reports, so the condition is exactly what
        // the drawing shows.
        #[cfg(feature = "ports")]
        if !self.graph.edge_ports.is_empty() {
            use crate::algorithms::sugiyama::ports::{EndRole, Face, PortSide};
            let (axis, flipped) = frame;
            for e in ir.edges() {
                for (end, attachment) in [
                    (crate::EdgeEnd::Source, e.from_port),
                    (crate::EdgeEnd::Target, e.to_port),
                ] {
                    if matches!(attachment.requested, PortSide::Auto) {
                        continue;
                    }
                    let role = match (end, e.reversed) {
                        (crate::EdgeEnd::Source, false) | (crate::EdgeEnd::Target, true) => {
                            EndRole::Source
                        }
                        _ => EndRole::Target,
                    };
                    let requested =
                        Face::of(attachment.requested, axis, flipped, role).physical(axis, flipped);
                    if requested != attachment.side {
                        diagnostics.emit(crate::diagnostics::DiagnosticKind::PortUnroutable {
                            edge: e.edge_index,
                            end,
                            requested,
                            resolved: attachment.side,
                        });
                    }
                }
            }
        }
        ir
    }

    /// Report terminal: run with an owned collector and package the
    /// (infallible) outcome with everything collected.
    pub fn reported(
        self,
    ) -> crate::diagnostics::OwnedReport<crate::ir::LayoutIR<'a>, core::convert::Infallible> {
        let mut run =
            crate::diagnostics::DiagnosticRun::new(crate::diagnostics::VecDiagnostics::default());
        let ir = {
            let mut cx = run.context();
            self.compute(&mut cx)
        };
        run.finish(Ok(ir))
    }

    /// Quiet terminal: explicitly discard diagnostics
    /// (constructs [`IgnoreDiagnostics`](crate::IgnoreDiagnostics)).
    pub fn quiet(self) -> crate::ir::LayoutIR<'a> {
        let mut run = crate::diagnostics::DiagnosticRun::new(crate::diagnostics::IgnoreDiagnostics);
        let mut cx = run.context();
        self.compute(&mut cx)
    }
}

/// Auto-numbering sentinel for `Graph::add_node`'s id slot:
/// `g.add_node(AUTO, "label")` lets the graph pick the next id above
/// every id it has seen. `Auto` does not convert to [`NodeId`], so the
/// sentinel cannot appear where an *existing* node is referenced
/// (`add_edge(AUTO, …)` is a compile error).
pub const AUTO: Auto = Auto;

/// An explicit node id or the [`AUTO`] sentinel, for
/// `Graph::add_node`'s id slot.
///
/// `From<usize>` keeps every existing call site compiling — including
/// bare integer literals. STANDING GUARD (compile-pinned by test):
/// `usize` must remain the **only** integer `From` impl here and on
/// [`NodeId`]; a second one would re-ambiguate bare literals onto
/// Rust's `i32` fallback and break them.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum IdOrAuto {
    /// A caller-chosen id.
    Id(usize),
    /// Graph-assigned: the next id above every id seen so far.
    Auto,
}

impl From<usize> for IdOrAuto {
    #[inline]
    fn from(id: usize) -> Self {
        IdOrAuto::Id(id)
    }
}

impl From<Auto> for IdOrAuto {
    #[inline]
    fn from(_: Auto) -> Self {
        IdOrAuto::Auto
    }
}

impl From<NodeId> for IdOrAuto {
    #[inline]
    fn from(handle: NodeId) -> Self {
        IdOrAuto::Id(handle.0)
    }
}

/// A directed graph with ASCII rendering capabilities.
///
/// Despite the crate name (`ascii-dag`), `Graph` supports cycles — they are
/// detected and broken automatically during layout.  Use [`Requirements::dag()`]
/// if you need to validate acyclicity before layout.
///
/// [`Requirements::dag()`]: crate::Requirements::dag
///
/// # Examples
///
/// ```
/// use ascii_dag::Graph;
///
/// let mut g = Graph::new();
/// g.add_node(1, "Start");
/// g.add_node(2, "End");
/// g.add_edge(1, 2, None);
///
/// let output = g.render();
/// assert!(output.contains("Start"));
/// assert!(output.contains("End"));
/// ```
#[cfg(feature = "alloc")]
#[allow(deprecated)]
#[derive(Clone, Default)]
pub struct Graph<'a> {
    pub(crate) nodes: Vec<(usize, &'a str)>,
    pub(crate) edges: Vec<(usize, usize, Option<&'a str>)>,
    /// Declared attachment sides per edge — a layout input, consumed
    /// by attachment resolution and reported back as resolved
    /// geometry. PAY-PER-USE: stays empty (no allocation) until the
    /// first declaration, which materializes it to `edges.len()` with
    /// `Auto`/`Auto`; readers treat a missing entry as `Auto`. A graph
    /// that never declares a port stores nothing per edge.
    #[cfg(feature = "ports")]
    pub(crate) edge_ports: Vec<(PortSide, PortSide)>,
    /// The graph-wide port policy — how a node places the ends
    /// declared on a face; `Single` unless set.
    #[cfg(feature = "ports")]
    pub(crate) port_policy: PortPolicy,
    /// Per-node overrides of [`port_policy`](Self::port_policy), by
    /// node id, sorted; empty until the first override.
    #[cfg(feature = "ports")]
    pub(crate) node_port_policies: Vec<(usize, PortPolicy)>,
    /// The one placer every `Custom` policy runs; `None` until
    /// [`set_port_placer`](Self::set_port_placer).
    #[cfg(feature = "ports")]
    pub(crate) port_placer: Option<crate::PortPlacer>,
    pub(crate) render_mode: RenderMode,
    pub(crate) direction: Direction,
    pub(crate) auto_created: HashSet<usize>, // Track auto-created nodes for visual distinction (O(1) lookups)
    pub(crate) id_to_index: HashMap<usize, usize>, // Cache id→index mapping (O(1) lookups)
    pub(crate) node_widths: Vec<usize>,      // Cached formatted widths
    pub(crate) node_heights: Vec<usize>,     // Cached node heights (1 = single-line)
    pub(crate) children: Vec<Vec<usize>>,    // Adjacency list: children[idx] = child indices
    pub(crate) parents: Vec<Vec<usize>>,     // Adjacency list: parents[idx] = parent indices
    pub(crate) sugiyama_config: SugiyamaConfig, // Full Sugiyama pipeline configuration
    pub(crate) subgraphs: Vec<Subgraph<'a>>, // Named clusters
    pub(crate) node_subgraph: HashMap<usize, usize>, // node_id → subgraph_id
    pub(crate) next_subgraph_id: usize,      // Monotonic ID counter
    pub(crate) next_auto: usize,             // AUTO id source: 1 above every id seen (saturating)
    pub(crate) auto_numbered: HashSet<usize>, // Ids assigned via AUTO (replace diagnostics)
    // D6 sparse+packed content storage: 1 B/node kind tag; painter +
    // payload only for nodes that have them, keyed by node index
    // (sorted — appends are naturally ordered, replaces upsert).
    pub(crate) node_kind_tag: Vec<u8>,
    pub(crate) node_custom: Vec<(usize, Option<NodePaintFn>, &'a str)>,
    /// Missing-node policy; `None` = never set (the implicit 0.10
    /// default, which makes placeholder creation diagnostic-worthy).
    missing_node_policy: Option<MissingNodePolicy>,
    /// The current crossing-passes condition (clamped or excessive),
    /// if any — a CONDITION SLOT reflecting the live configuration,
    /// never an event log: each setter call overwrites it, a sane
    /// value or a direct pipeline clears it, and every
    /// diagnostic-aware layout run emits it (without consuming — the
    /// condition holds until fixed). Plain `Copy` state: no interior
    /// mutability, so `Graph` stays `Send + Sync` and mutation never
    /// allocates for diagnostics.
    passes_note: Option<crate::diagnostics::DiagnosticKind>,
}

#[cfg(feature = "alloc")]
#[allow(deprecated)]
impl<'a> Graph<'a> {
    /// Create a new empty DAG.
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::Graph;
    /// let dag = Graph::new();
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a DAG from pre-defined nodes and edges (batch construction).
    ///
    /// This is more efficient than using the builder API for static graphs.
    /// For edges with labels, use [`from_edges_labeled`](Self::from_edges_labeled).
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::Graph;
    ///
    /// let dag = Graph::from_edges(
    ///     &[(1, "A"), (2, "B"), (3, "C")],
    ///     &[(1, 2), (2, 3)]
    /// );
    /// ```
    pub fn from_edges(nodes: &[(usize, &'a str)], edges: &[(usize, usize)]) -> Self {
        let mut dag = Self {
            nodes: nodes.to_vec(),
            edges: Vec::new(),
            #[cfg(feature = "ports")]
            edge_ports: Vec::new(),
            #[cfg(feature = "ports")]
            port_policy: PortPolicy::Single,
            #[cfg(feature = "ports")]
            node_port_policies: Vec::new(),
            #[cfg(feature = "ports")]
            port_placer: None,
            render_mode: RenderMode::default(),
            direction: Direction::default(),
            auto_created: HashSet::new(),
            id_to_index: HashMap::new(),
            node_widths: Vec::new(),
            node_heights: Vec::new(),
            children: Vec::new(),
            parents: Vec::new(),
            sugiyama_config: SugiyamaConfig::default(),
            subgraphs: Vec::new(),
            node_subgraph: HashMap::new(),
            next_subgraph_id: 0,
            next_auto: 0,
            auto_numbered: HashSet::new(),
            node_kind_tag: Vec::new(),
            node_custom: Vec::new(),
            missing_node_policy: None,
            passes_note: None,
        };

        // Build id_to_index map, widths cache, and heights cache; seed
        // the AUTO counter above every batch-supplied id in the same
        // pass.
        let mut next_auto = 0usize;
        for (idx, &(id, label)) in dag.nodes.iter().enumerate() {
            dag.id_to_index.insert(id, idx);
            let width = dag.compute_node_width(id, label);
            dag.node_widths.push(width);
            dag.node_heights.push(1);
            dag.node_kind_tag.push(NodeKindTag::Simple.to_u8());
            next_auto = next_auto.max(id.saturating_add(1));
        }
        dag.next_auto = next_auto;

        // Initialize adjacency lists
        dag.children.resize(dag.nodes.len(), Vec::new());
        dag.parents.resize(dag.nodes.len(), Vec::new());

        // Add edges (may auto-create missing nodes)
        for &(from, to) in edges {
            dag.add_edge(from, to, None);
        }

        dag
    }

    /// Create a DAG from pre-defined nodes and labeled edges (batch construction).
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::Graph;
    ///
    /// let dag = Graph::from_edges_labeled(
    ///     &[(1, "A"), (2, "B"), (3, "C")],
    ///     &[(1, 2, Some("uses")), (2, 3, None)]
    /// );
    /// ```
    pub fn from_edges_labeled(
        nodes: &[(usize, &'a str)],
        edges: &[(usize, usize, Option<&'a str>)],
    ) -> Self {
        let mut dag = Self {
            nodes: nodes.to_vec(),
            edges: Vec::new(),
            #[cfg(feature = "ports")]
            edge_ports: Vec::new(),
            #[cfg(feature = "ports")]
            port_policy: PortPolicy::Single,
            #[cfg(feature = "ports")]
            node_port_policies: Vec::new(),
            #[cfg(feature = "ports")]
            port_placer: None,
            render_mode: RenderMode::default(),
            direction: Direction::default(),
            auto_created: HashSet::new(),
            id_to_index: HashMap::new(),
            node_widths: Vec::new(),
            node_heights: Vec::new(),
            children: Vec::new(),
            parents: Vec::new(),
            sugiyama_config: SugiyamaConfig::default(),
            subgraphs: Vec::new(),
            node_subgraph: HashMap::new(),
            next_subgraph_id: 0,
            next_auto: 0,
            auto_numbered: HashSet::new(),
            node_kind_tag: Vec::new(),
            node_custom: Vec::new(),
            missing_node_policy: None,
            passes_note: None,
        };

        // Build id_to_index map, widths cache, and heights cache; seed
        // the AUTO counter above every batch-supplied id in the same
        // pass.
        let mut next_auto = 0usize;
        for (idx, &(id, label)) in dag.nodes.iter().enumerate() {
            dag.id_to_index.insert(id, idx);
            let width = dag.compute_node_width(id, label);
            dag.node_widths.push(width);
            dag.node_heights.push(1);
            dag.node_kind_tag.push(NodeKindTag::Simple.to_u8());
            next_auto = next_auto.max(id.saturating_add(1));
        }
        dag.next_auto = next_auto;

        // Initialize adjacency lists
        dag.children.resize(dag.nodes.len(), Vec::new());
        dag.parents.resize(dag.nodes.len(), Vec::new());

        // Add edges (may auto-create missing nodes)
        for &(from, to, label) in edges {
            dag.add_edge(from, to, label);
        }

        dag
    }

    /// Set the rendering mode.
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::{Graph, RenderMode};
    ///
    /// let mut dag = Graph::new();
    /// dag.set_render_mode(RenderMode::Horizontal);
    /// ```
    pub fn set_render_mode(&mut self, mode: RenderMode) {
        self.render_mode = mode;
    }

    /// Set the rank direction (see [`Direction`]).
    ///
    /// Applies when computing the layout via [`render`](Self::render) or
    /// [`compute_layout`](Self::compute_layout). When you build a
    /// [`LayoutConfig`] yourself and call
    /// [`compute_layout_with_config`](Self::compute_layout_with_config),
    /// the config's `direction` wins.
    ///
    /// All four directions render. One caveat on the
    /// [`render`](Self::render) convenience: under
    /// [`RenderMode::Auto`] a simple chain uses the compact
    /// left-to-right form only for `TopDown`; any other direction is
    /// laid out and painted normally. Asking for
    /// [`RenderMode::Horizontal`] explicitly always gives the chain
    /// form, whatever the direction.
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::{Direction, Graph};
    ///
    /// let mut dag = Graph::new();
    /// dag.set_direction(Direction::DEFAULT);
    /// # #[cfg(feature = "layout-vertical")]
    /// dag.set_direction(Direction::BottomUp);
    /// ```
    pub fn set_direction(&mut self, direction: Direction) {
        self.direction = direction;
    }

    /// The rank direction set via [`set_direction`](Self::set_direction)
    /// (`TopDown` by default) — e.g. to carry it into a
    /// [`LayoutConfig`] for the CSR pipeline.
    pub fn direction(&self) -> Direction {
        self.direction
    }

    /// Set the number of passes for the crossing reduction algorithm.
    ///
    /// This is a **compatibility shim** — it replaces the entire pipeline
    /// with `[Median(passes)]`.  Prefer [`set_crossing_pipeline`](Self::set_crossing_pipeline)
    /// for full control.
    ///
    /// - `0`: Skip crossing reduction entirely
    /// - `1-4`: Good for most graphs
    /// - `8-10`: Better layouts for complex tangled graphs, but slower
    ///
    /// Values > 20 trigger a warning.  Values > 1000 are clamped to 0.
    pub fn set_crossing_reduction_passes(&mut self, passes: usize) {
        let (p, note) = Self::validate_passes(passes);
        // A condition slot, not a log: the note describes the CURRENT
        // value, so each call overwrites it (a sane value clears it),
        // and every diagnostic-aware layout run reports it until the
        // configuration is fixed.
        self.passes_note = note;
        self.sugiyama_config.crossing_pipeline = if p == 0 {
            Vec::new()
        } else {
            vec![CrossingReducer::Median(p)]
        };
    }

    /// Set the crossing reduction pipeline.
    ///
    /// The pipeline is a sequence of [`CrossingReducer`] strategies applied
    /// in order.  Use the presets [`FAST`](crate::algorithms::sugiyama::crossing::FAST),
    /// [`STANDARD`](crate::algorithms::sugiyama::crossing::STANDARD), or
    /// [`QUALITY`](crate::algorithms::sugiyama::crossing::QUALITY), or build
    /// your own.
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::Graph;
    /// use ascii_dag::algorithms::sugiyama::crossing::{CrossingReducer, QUALITY};
    ///
    /// let mut dag = Graph::new();
    /// dag.set_crossing_pipeline(QUALITY);
    /// ```
    pub fn set_crossing_pipeline(&mut self, pipeline: &[CrossingReducer]) {
        // Replaces the compatibility shim's value wholesale — any
        // standing passes note describes a configuration that no
        // longer exists.
        self.passes_note = None;
        self.sugiyama_config.crossing_pipeline = pipeline.to_vec();
    }

    /// Validate crossing reduction passes, returning a safe value.
    /// - Values > 1000 are treated as accidental (e.g., -1i32 as usize) and clamped to 0
    /// - Values > 20 trigger a warning about diminishing returns
    #[inline]
    fn validate_passes(passes: usize) -> (usize, Option<crate::diagnostics::DiagnosticKind>) {
        if passes > 1000 {
            (
                0,
                Some(crate::diagnostics::DiagnosticKind::CrossingPassesClamped {
                    requested: passes,
                    clamped_to: 0,
                }),
            )
        } else if passes > 20 {
            (
                passes,
                Some(
                    crate::diagnostics::DiagnosticKind::CrossingPassesExcessive {
                        requested: passes,
                    },
                ),
            )
        } else {
            (passes, None)
        }
    }

    /// Builder method: set render mode (chainable).
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::{Graph, RenderMode};
    ///
    /// let dag = Graph::new()
    ///     .with_render_mode(RenderMode::Horizontal)
    ///     .with_crossing_reduction_passes(6);
    /// ```
    pub fn with_render_mode(mut self, mode: RenderMode) -> Self {
        self.render_mode = mode;
        self
    }

    /// Builder method: set the rank direction (chainable).
    pub fn with_direction(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }

    /// Builder method: set crossing reduction passes (chainable).
    ///
    /// **Compatibility shim** — see [`set_crossing_reduction_passes`](Self::set_crossing_reduction_passes).
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::Graph;
    ///
    /// let dag = Graph::new()
    ///     .with_crossing_reduction_passes(8);  // More passes for complex graphs
    /// ```
    pub fn with_crossing_reduction_passes(mut self, passes: usize) -> Self {
        self.set_crossing_reduction_passes(passes);
        self
    }

    /// Builder method: set crossing reduction pipeline (chainable).
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::Graph;
    /// use ascii_dag::algorithms::sugiyama::crossing::QUALITY;
    ///
    /// let dag = Graph::new()
    ///     .with_crossing_pipeline(QUALITY);
    /// ```
    pub fn with_crossing_pipeline(mut self, pipeline: &[CrossingReducer]) -> Self {
        // Delegate so the condition slot stays consistent: replacing
        // the pipeline clears any standing passes note, in the
        // builder chain exactly as in the setter.
        self.set_crossing_pipeline(pipeline);
        self
    }

    /// Create a DAG with a specific render mode.
    ///
    /// **Deprecated**: Prefer `Graph::new().with_render_mode(mode)` for consistency.
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::{Graph, RenderMode};
    ///
    /// let dag = Graph::with_mode(RenderMode::Horizontal);
    /// ```
    pub fn with_mode(mode: RenderMode) -> Self {
        Self::new().with_render_mode(mode)
    }

    /// Record `id` as seen so a later [`AUTO`] pick lands above it.
    /// Saturating: near `usize::MAX` the counter pins to the top and a
    /// subsequent `AUTO` falls through to the documented
    /// replace-on-duplicate semantics (don't park explicit ids there —
    /// that band already collides with synthetic dummy ids).
    #[inline]
    fn bump_next_auto(&mut self, id: usize) {
        let next = id.saturating_add(1);
        if next > self.next_auto {
            self.next_auto = next;
        }
    }

    /// Upsert the sparse painter/payload entry for a node index —
    /// entries exist only for nodes that have a painter or a non-empty
    /// payload (D6). The list is sorted by node index; appends during
    /// construction hit the `Err(end)` arm, replaces upsert in place.
    fn set_custom_entry(&mut self, idx: usize, painter: Option<NodePaintFn>, payload: &'a str) {
        let pos = self.node_custom.binary_search_by_key(&idx, |entry| entry.0);
        let keep = painter.is_some() || !payload.is_empty();
        match (pos, keep) {
            (Ok(at), true) => self.node_custom[at] = (idx, painter, payload),
            (Ok(at), false) => {
                self.node_custom.remove(at);
            }
            (Err(at), true) => self.node_custom.insert(at, (idx, painter, payload)),
            (Err(_), false) => {}
        }
    }

    /// Add a node to the DAG. Returns a typed [`NodeId`] handle usable
    /// wherever a node id is accepted (edges, subgraph placement).
    ///
    /// The content slot accepts anything implementing
    /// [`NodeContent`]: a bare `&str` (the classic `[label]` node,
    /// byte-identical to previous releases), a built-in object
    /// ([`SimpleNode`](crate::SimpleNode) /
    /// [`BoxedNode`](crate::BoxedNode)), or a user type /
    /// [`CustomNode`](crate::CustomNode) carrying its own size,
    /// painter, and payload. The declaration is the *only* source of
    /// what the node is — there is no style-side override. The object
    /// is resolved once, here — it may be a temporary.
    ///
    /// The id slot takes an explicit `usize` **or** the [`AUTO`]
    /// sentinel, which assigns the next id above every id this graph
    /// has seen — explicit ids first (e.g. [`Graph::from_edges`]) then
    /// `AUTO` extras stay collision-free until the counter saturates
    /// at `usize::MAX` (only reachable by explicitly parking ids
    /// there); a saturated `AUTO`, like any duplicate id, falls
    /// through to the replace semantics below.
    ///
    /// If the node already exists (auto-created by `add_edge`, or added
    /// earlier), this replaces its label — promoting auto-created
    /// placeholders to explicit nodes. The returned [`NodeInsertion`]
    /// receipt records the replacement, and flags the variant worth
    /// attention: `AUTO` numbering involved on either side (an
    /// explicit id overwriting an auto-numbered node, or a saturated
    /// `AUTO` overwriting anything).
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::{Graph, AUTO};
    ///
    /// let mut dag = Graph::new();
    /// dag.add_node(1, "MyNode");            // explicit id
    /// let n = dag.add_node(AUTO, "Auto");   // graph-assigned id
    /// dag.add_edge(1, n, None);             // handles flow into edges
    /// ```
    pub fn add_node(
        &mut self,
        id: impl Into<IdOrAuto>,
        node: impl NodeContent<'a>,
    ) -> NodeInsertion {
        let (id, incoming_auto) = match id.into() {
            IdOrAuto::Id(id) => (id, false),
            IdOrAuto::Auto => (self.next_auto, true),
        };
        // Resolve the content BEFORE any graph state mutates: the
        // accessors are user code and may panic — a caught panic must
        // not leave the AUTO counter or diagnostics state advanced for
        // an insertion that never happened ("successful creation
        // only"). Accessors should be cheap and pure; resolution may
        // call them more than once (the default `size()` reads
        // `label()`).
        let label = node.label();
        let (width, height) = node.size();
        let kind = node.kind();
        let painter = node.painter();
        let payload = node.payload();
        let size_is_implicit = node.size_is_implicit();
        self.bump_next_auto(id);
        // Replacement is a point event, so the receipt is its record:
        // a duplicate involving AUTO on either side (an explicit id
        // silently overwriting an auto-numbered node, or a saturated
        // AUTO overwriting an existing node) is the variant worth the
        // caller's attention.
        let replaced = self.id_to_index.contains_key(&id);
        let replaced_involving_auto =
            replaced && (incoming_auto || self.auto_numbered.contains(&id));
        {
            // Track how this id was assigned (auto or explicit) so the
            // check above can see the existing side next time.
            if incoming_auto {
                self.auto_numbered.insert(id);
            } else {
                self.auto_numbered.remove(&id);
            }
        }
        // Empty-label placeholders render as ⟨id⟩ — an id-dependent
        // width the content object cannot know. Only content whose
        // `size()` is the provided default (`&str`, `&String`,
        // `SimpleNode` — size provenance, not a value heuristic) gets
        // today's formula; every overridden `size()` is authoritative.
        let width = if label.is_empty() && size_is_implicit {
            self.compute_node_width(id, label)
        } else {
            width
        };
        let height = height.max(1);
        // Check if node already exists (could be auto-created) - O(1) with HashMap
        if let Some(&idx) = self.id_to_index.get(&id) {
            // Promote auto-created node to explicit node
            self.nodes[idx] = (id, label);
            // Remove from auto_created set - O(1)
            self.auto_created.remove(&id);
            self.node_widths[idx] = width;
            self.node_heights[idx] = height;
            self.node_kind_tag[idx] = kind.to_u8();
            self.set_custom_entry(idx, painter, payload);
        } else {
            // Brand new node
            let idx = self.nodes.len();
            self.nodes.push((id, label));
            self.id_to_index.insert(id, idx);
            self.node_widths.push(width);
            self.node_heights.push(height);
            self.node_kind_tag.push(kind.to_u8());
            // Extend adjacency lists
            self.children.push(Vec::new());
            self.parents.push(Vec::new());
            self.set_custom_entry(idx, painter, payload);
        }
        NodeInsertion {
            node: NodeId(id),
            replaced,
            replaced_involving_auto,
        }
    }

    /// Add an edge from one node to another with an optional label.
    ///
    /// If either node doesn't exist, it will be auto-created as a placeholder.
    /// You can later call `add_node` to provide a label for auto-created nodes.
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::Graph;
    ///
    /// let mut dag = Graph::new();
    /// dag.add_node(1, "A");
    /// dag.add_node(2, "B");
    /// dag.add_node(3, "C");
    /// dag.add_edge(1, 2, None);  // A -> B (no label)
    /// dag.add_edge(2, 3, Some("depends on"));  // B -> C with label
    /// ```
    ///
    /// Accepts raw ids and [`NodeId`] handles alike. The [`AUTO`]
    /// sentinel is *not* accepted here — an edge references nodes, and
    /// "auto" is meaningless as a reference.
    pub fn add_edge<'g>(
        &'g mut self,
        from: impl Into<NodeId>,
        to: impl Into<NodeId>,
        label: Option<&'a str>,
    ) -> EdgeHandle<'g, 'a> {
        let (from, to) = (from.into().id(), to.into().id());
        let created_source = self.ensure_node_exists(from);
        let created_target = self.ensure_node_exists(to);
        let edge = self.edges.len();
        self.edges.push((from, to, label));
        #[cfg(feature = "ports")]
        if !self.edge_ports.is_empty() {
            // Only a graph that already declared ports keeps the table
            // parallel; everyone else pays nothing here.
            self.edge_ports.push((PortSide::Auto, PortSide::Auto));
        }

        // Update adjacency lists (O(1) lookups)
        if let (Some(&from_idx), Some(&to_idx)) =
            (self.id_to_index.get(&from), self.id_to_index.get(&to))
        {
            self.children[from_idx].push(to_idx);
            self.parents[to_idx].push(from_idx);
        }
        EdgeHandle {
            receipt: EdgeInsertion {
                edge,
                created_source,
                created_target,
            },
            graph: self,
        }
    }

    /// Declare how [`add_edge`](Self::add_edge) treats an undeclared
    /// endpoint. Setting the policy — even to the default — is
    /// declared intent: placeholder creation then stops emitting the
    /// placeholder diagnostic (the [`EdgeInsertion`] receipt remains
    /// the record either way).
    pub fn set_missing_node_policy(&mut self, policy: MissingNodePolicy) {
        self.missing_node_policy = Some(policy);
    }

    /// Declare the sides an edge (by input index) attaches to —
    /// the index form of the handle's `from_port` / `to_port`, for a
    /// declaration made after insertion; `false` for an unknown edge.
    #[cfg(feature = "ports")]
    pub fn set_edge_ports(&mut self, edge: usize, source: PortSide, target: PortSide) -> bool {
        if edge >= self.edges.len() {
            return false;
        }
        self.declare_port(edge, 0, source);
        self.declare_port(edge, 1, target);
        true
    }

    /// Set the graph-wide port policy: how a node places the ends
    /// declared on each of its faces (`Single` unless set — one shared
    /// port per face). `false`, and nothing changes, for `Custom`
    /// before a placer is registered
    /// ([`set_port_placer`](Self::set_port_placer)).
    #[cfg(feature = "ports")]
    pub fn set_port_policy(&mut self, policy: PortPolicy) -> bool {
        if policy == PortPolicy::Custom && self.port_placer.is_none() {
            return false;
        }
        self.port_policy = policy;
        true
    }

    /// The graph-wide port policy.
    #[cfg(feature = "ports")]
    pub fn port_policy(&self) -> PortPolicy {
        self.port_policy
    }

    /// Register the placer every `Custom` policy runs — one per graph,
    /// told the node id, so one function places every node. A later
    /// registration replaces it for every `Custom` node.
    #[cfg(feature = "ports")]
    pub fn set_port_placer(&mut self, placer: crate::PortPlacer) {
        self.port_placer = Some(placer);
    }

    /// The registered placer, if any.
    #[cfg(feature = "ports")]
    pub fn port_placer(&self) -> Option<crate::PortPlacer> {
        self.port_placer
    }

    /// Override the port policy for one node; `false` for an unknown
    /// node, or for `Custom` before a placer is registered. A face
    /// with one cell holds one port whatever the policy.
    #[cfg(feature = "ports")]
    pub fn set_node_port_policy(&mut self, node: impl Into<NodeId>, policy: PortPolicy) -> bool {
        let id = node.into().id();
        if !self.id_to_index.contains_key(&id)
            || (policy == PortPolicy::Custom && self.port_placer.is_none())
        {
            return false;
        }
        match self.node_port_policies.binary_search_by_key(&id, |e| e.0) {
            Ok(i) => self.node_port_policies[i].1 = policy,
            Err(i) => self.node_port_policies.insert(i, (id, policy)),
        }
        true
    }

    /// Remove a node's override so it follows the graph-wide policy
    /// again, now and after that policy changes; `false` when the node
    /// had none.
    #[cfg(feature = "ports")]
    pub fn clear_node_port_policy(&mut self, node: impl Into<NodeId>) -> bool {
        let id = node.into().id();
        match self.node_port_policies.binary_search_by_key(&id, |e| e.0) {
            Ok(i) => {
                self.node_port_policies.remove(i);
                true
            }
            Err(_) => false,
        }
    }

    /// The port policy a node is declared under: its override, else
    /// the graph-wide policy. Unknown nodes read the graph-wide one.
    #[cfg(feature = "ports")]
    pub fn node_port_policy(&self, node: impl Into<NodeId>) -> PortPolicy {
        let id = node.into().id();
        self.node_port_policies
            .binary_search_by_key(&id, |e| e.0)
            .map_or(self.port_policy, |i| self.node_port_policies[i].1)
    }

    /// Record one declared side (`end` 0 = from, 1 = to) of an
    /// existing edge. The table materializes on the first NON-`Auto`
    /// declaration — the only moment a port-declaring graph pays; an
    /// explicit `Auto` on an empty table is exactly what an undeclared
    /// edge already means, so it costs nothing.
    #[cfg(feature = "ports")]
    fn declare_port(&mut self, edge: usize, end: usize, side: PortSide) {
        debug_assert!(edge < self.edges.len());
        if self.edge_ports.is_empty() && matches!(side, PortSide::Auto) {
            return;
        }
        if self.edge_ports.len() < self.edges.len() {
            self.edge_ports
                .resize(self.edges.len(), (PortSide::Auto, PortSide::Auto));
        }
        let slot = &mut self.edge_ports[edge];
        if end == 0 {
            slot.0 = side;
        } else {
            slot.1 = side;
        }
    }

    /// Get the label for an edge, if any.
    #[inline]
    pub fn edge_label(&self, edge_idx: usize) -> Option<&'a str> {
        self.edges.get(edge_idx).and_then(|e| e.2)
    }

    /// Ensure a node exists, auto-creating if missing.
    /// Auto-created nodes will be visually distinct (rendered with ⟨⟩ instead of [])
    /// until explicitly defined with add_node.
    /// Returns whether a placeholder was created.
    fn ensure_node_exists(&mut self, id: usize) -> bool {
        // O(1) lookup with HashMap
        if !self.id_to_index.contains_key(&id) {
            // No diagnostic is recorded here: an implicit placeholder
            // is a standing CONDITION (`auto_created` + undeclared
            // policy), re-derived by each diagnostic-aware layout run
            // — nothing is stored, and the receipt serves the call
            // site. An explicitly declared policy silences the
            // condition, whenever it is declared.

            // Create node with empty label. An id-creating site: the
            // AUTO counter must clear implicitly created ids too.
            self.bump_next_auto(id);
            let idx = self.nodes.len();
            self.nodes.push((id, ""));
            self.auto_created.insert(id); // O(1) insert
            self.id_to_index.insert(id, idx); // O(1) insert
            let width = self.compute_node_width(id, "");
            self.node_widths.push(width);
            self.node_heights.push(1);
            self.node_kind_tag.push(NodeKindTag::Simple.to_u8());
            // Extend adjacency lists
            self.children.push(Vec::new());
            self.parents.push(Vec::new());
            return true;
        }
        false
    }

    /// Check if a node was auto-created (for visual distinction)
    pub(crate) fn is_auto_created(&self, id: usize) -> bool {
        self.auto_created.contains(&id) // O(1) with HashSet
    }

    /// Write an unsigned integer to a string buffer without allocation.
    /// This avoids format! bloat in no_std builds.
    #[inline]
    pub(crate) fn write_usize(buf: &mut String, mut n: usize) {
        if n == 0 {
            buf.push('0');
            return;
        }
        let mut digits = [0u8; 20]; // Max digits for u64
        let mut i = 0;
        while n > 0 {
            digits[i] = (n % 10) as u8 + b'0';
            n /= 10;
            i += 1;
        }
        // Write in reverse order
        while i > 0 {
            i -= 1;
            buf.push(digits[i] as char);
        }
    }

    /// Count digits in a number (for width calculation)
    #[inline]
    fn count_digits(mut n: usize) -> usize {
        if n == 0 {
            return 1;
        }
        let mut count = 0;
        while n > 0 {
            count += 1;
            n /= 10;
        }
        count
    }

    /// Compute the formatted width of a node
    pub(crate) fn compute_node_width(&self, id: usize, label: &str) -> usize {
        if label.is_empty() || self.is_auto_created(id) {
            // ⟨ID⟩ format
            2 + Self::count_digits(id) // ⟨ + digits + ⟩
        } else {
            // [Label] format
            2 + label.chars().count() // [ + label + ]
        }
    }

    /// Write a formatted node directly to output buffer (avoids intermediate String allocation)
    #[inline]
    pub(crate) fn write_node(&self, output: &mut String, id: usize, label: &str) {
        if label.is_empty() || self.is_auto_created(id) {
            output.push('⟨');
            Self::write_usize(output, id);
            output.push('⟩');
        } else {
            output.push('[');
            output.push_str(label);
            output.push(']');
        }
    }

    /// Get children of a node (returns IDs, not indices).
    /// Uses cached adjacency lists for O(1) lookup instead of O(E) iteration.
    /// NOTE: This allocates a new Vec. For hot paths, use `children_count` + `get_children_indices`.
    #[inline]
    pub(crate) fn get_children(&self, node_id: usize) -> Vec<usize> {
        if let Some(&idx) = self.id_to_index.get(&node_id) {
            // Convert child indices back to IDs
            self.children[idx]
                .iter()
                .map(|&child_idx| self.nodes[child_idx].0)
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get parents of a node (returns IDs, not indices).
    /// Uses cached adjacency lists for O(1) lookup instead of O(E) iteration.
    /// NOTE: This allocates a new Vec. For hot paths, use `parents_count` + `get_parents_indices`.
    #[inline]
    pub(crate) fn get_parents(&self, node_id: usize) -> Vec<usize> {
        if let Some(&idx) = self.id_to_index.get(&node_id) {
            // Convert parent indices back to IDs
            self.parents[idx]
                .iter()
                .map(|&parent_idx| self.nodes[parent_idx].0)
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Count children of a node by index (zero-allocation).
    #[inline]
    pub(crate) fn children_count(&self, node_idx: usize) -> usize {
        self.children.get(node_idx).map_or(0, |c| c.len())
    }

    /// Count parents of a node by index (zero-allocation).
    #[inline]
    pub(crate) fn parents_count(&self, node_idx: usize) -> usize {
        self.parents.get(node_idx).map_or(0, |p| p.len())
    }

    /// Get node index from ID using O(1) HashMap lookup
    #[inline]
    pub(crate) fn node_index(&self, id: usize) -> Option<usize> {
        self.id_to_index.get(&id).copied()
    }

    /// Get cached width for a node index
    #[inline]
    pub(crate) fn get_node_width(&self, idx: usize) -> usize {
        self.node_widths.get(idx).copied().unwrap_or(0)
    }

    /// Get cached height for a node index
    #[inline]
    pub(crate) fn get_node_height(&self, idx: usize) -> usize {
        self.node_heights.get(idx).copied().unwrap_or(1)
    }

    /// Estimate the buffer size needed for rendering.
    ///
    /// Use this to pre-allocate a buffer for [`render_to`](Self::render_to).
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::Graph;
    ///
    /// let dag = Graph::from_edges(
    ///     &[(1, "A"), (2, "B")],
    ///     &[(1, 2)]
    /// );
    ///
    /// let size = dag.estimate_size();
    /// let mut buffer = String::with_capacity(size);
    /// dag.render_to(&mut buffer);
    /// ```
    pub fn estimate_size(&self) -> usize {
        // Estimate based on empirical measurements:
        // - Each level takes ~width characters (canvas can be very wide)
        // - Vertical layouts have many levels with connection lines
        // - For layered graphs with skip-edges, canvas can be quite wide
        //
        // Vertical layout: nodes spread across canvas width + connection lines
        // Each node level line + ~5 connection lines per level
        // Canvas width roughly: nodes_per_level * 15 chars
        // Height roughly: levels * 6 lines
        let n = self.nodes.len();
        let est_levels = n.isqrt().max(1);
        let est_width = (n / est_levels).max(1) * 15;
        let est_height = est_levels * 6;
        let base = est_width * est_height * 3; // UTF-8 chars average ~3 bytes
        base.max(n * 100) // Ensure minimum sensible estimate
    }

    /// Compute the layout intermediate representation for this DAG.
    ///
    /// This returns a renderer-agnostic representation of the laid-out graph
    /// that can be consumed by various renderers (ASCII, ANSI colors, SVG, etc.).
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::Graph;
    ///
    /// let dag = Graph::from_edges(
    ///     &[(1, "A"), (2, "B"), (3, "C")],
    ///     &[(1, 2), (1, 3), (2, 3)]
    /// );
    ///
    /// let ir = dag.compute_layout();
    ///
    /// // Inspect layout
    /// println!("Width: {}, Height: {}", ir.width(), ir.height());
    /// for node in ir.nodes() {
    ///     println!("{} at ({}, {})", node.label, node.x, node.y);
    /// }
    /// ```
    pub fn compute_layout(&self) -> crate::ir::LayoutIR<'a> {
        let mut config: LayoutConfig<'_> = LayoutConfig::from(&self.sugiyama_config);
        config.direction = self.direction;
        self.layout_with(&config)
    }

    /// Dispatch to the axis profile the direction needs (temp/08 D1):
    /// `LeftRight`/`RightLeft` lay out natively through `Horizontal`
    /// — levels become columns, edges run in horizontal trunks —
    /// everything else through `Vertical`. `RightLeft` is `LeftRight`
    /// mirrored on x, applied inside the pipeline.
    fn layout_with(&self, config: &LayoutConfig<'_>) -> crate::ir::LayoutIR<'a> {
        #[cfg(feature = "layout-horizontal")]
        use crate::algorithms::sugiyama::geometry::Horizontal;
        #[cfg(feature = "layout-vertical")]
        use crate::algorithms::sugiyama::geometry::Vertical;
        match config.direction {
            #[cfg(feature = "layout-horizontal")]
            Direction::LeftRight | Direction::RightLeft => {
                crate::algorithms::sugiyama::heap::compute_layout_cfg::<Horizontal>(self, config)
            }
            #[cfg(feature = "layout-vertical")]
            _ => crate::algorithms::sugiyama::heap::compute_layout_cfg::<Vertical>(self, config),
        }
    }

    /// Begin a layout run — the diagnostic-aware entry point, as a
    /// builder because its inputs are optional. Finish with one of
    /// three terminals:
    ///
    /// - [`compute(&mut cx)`](LayoutRun::compute) — canonical: emits
    ///   into your run's context and composes across phases;
    /// - [`reported()`](LayoutRun::reported) — owns a complete
    ///   [`OwnedReport`](crate::OwnedReport) for this operation;
    /// - [`quiet()`](LayoutRun::quiet) — explicitly discards
    ///   diagnostics.
    ///
    /// Mutation-context diagnostics are standing CONDITIONS,
    /// re-derived from the graph at each diagnostic-aware run and
    /// emitted before the layout computes — the graph stores no
    /// diagnostic state: implicit auto-created placeholders (in node
    /// insertion order, while the missing-node policy stays
    /// undeclared) and the current crossing-passes note. Like a
    /// compiler warning, a condition reports on every run until it is
    /// fixed — declare the policy, promote the placeholder, set a
    /// sane pass count. Point events belong to receipts at the call
    /// site ([`EdgeInsertion`], [`NodeInsertion`]). Quiet paths
    /// ([`quiet()`](LayoutRun::quiet), `compute_layout`,
    /// `compute_layout_with_config`) are exactly equivalent: with no
    /// stored diagnostics there is nothing to consume, leak, or
    /// deliver late.
    pub fn layout<'g>(&'g self) -> LayoutRun<'g, 'a> {
        LayoutRun {
            graph: self,
            config: None,
        }
    }

    /// Compute the layout using a custom [`LayoutConfig`].
    ///
    /// This is the preferred API for controlling layout behaviour.
    /// The config borrows its crossing pipeline, so it can be constructed
    /// from static presets or from a `SugiyamaConfig`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::{Graph, LayoutConfig};
    ///
    /// let dag = Graph::from_edges(
    ///     &[(1, "A"), (2, "B"), (3, "C")],
    ///     &[(1, 2), (2, 3)]
    /// );
    ///
    /// let ir = dag.compute_layout_with_config(&LayoutConfig::quality());
    /// ```
    pub fn compute_layout_with_config(&self, config: &LayoutConfig<'_>) -> crate::ir::LayoutIR<'a> {
        let mut dag = self.clone();
        dag.render_mode = config.render_mode;
        dag.layout_with(config)
    }

    // ── Subgraph / Cluster API ───────────────────────────────────────────

    /// Create a new named subgraph (cluster) and return its ID.
    ///
    /// The subgraph has no members and no parent until you call
    /// [`put_nodes`](Self::put_nodes) or [`put_subgraphs`](Self::put_subgraphs).
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::Graph;
    ///
    /// let mut g = Graph::new();
    /// let backend = g.add_subgraph("Backend");
    /// let frontend = g.add_subgraph("Frontend");
    /// assert_ne!(backend, frontend);
    /// ```
    pub fn add_subgraph(&mut self, label: &'a str) -> usize {
        let id = self.next_subgraph_id;
        self.next_subgraph_id += 1;
        self.subgraphs.push(Subgraph {
            id,
            label,
            parent_id: None,
        });
        id
    }

    /// Start a fluent builder to place nodes inside a subgraph.
    ///
    /// Returns a [`NodePlacer`] whose [`.inside(sg)`](NodePlacer::inside)
    /// method assigns all listed nodes to the given subgraph.
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::Graph;
    ///
    /// let mut g = Graph::new();
    /// g.add_node(1, "A");
    /// g.add_node(2, "B");
    /// let sg = g.add_subgraph("Cluster");
    /// g.put_nodes(&[1, 2]).inside(sg).unwrap();
    /// assert_eq!(g.node_subgraph(1), Some(sg));
    /// ```
    ///
    /// The slice may hold raw `usize` ids or [`NodeId`] handles.
    pub fn put_nodes<'g, N: Into<NodeId> + Copy>(
        &'g mut self,
        node_ids: &'g [N],
    ) -> NodePlacer<'g, 'a, N> {
        NodePlacer {
            graph: self,
            node_ids,
        }
    }

    /// Start a fluent builder to nest subgraphs inside a parent.
    ///
    /// Returns a [`SubgraphPlacer`] whose [`.inside(parent)`](SubgraphPlacer::inside)
    /// method sets the parent for all listed subgraphs.  Cycle detection
    /// prevents a subgraph from being nested inside itself or a descendant.
    ///
    /// # Examples
    ///
    /// ```
    /// use ascii_dag::graph::Graph;
    ///
    /// let mut g = Graph::new();
    /// let outer = g.add_subgraph("Outer");
    /// let inner = g.add_subgraph("Inner");
    /// g.put_subgraphs(&[inner]).inside(outer).unwrap();
    /// ```
    pub fn put_subgraphs<'g>(&'g mut self, sg_ids: &'g [usize]) -> SubgraphPlacer<'g, 'a> {
        SubgraphPlacer {
            graph: self,
            sg_ids,
        }
    }

    /// Number of subgraphs defined on this graph.
    #[inline]
    pub fn subgraph_count(&self) -> usize {
        self.subgraphs.len()
    }

    /// Which subgraph a node belongs to, if any.
    #[inline]
    pub fn node_subgraph(&self, node_id: usize) -> Option<usize> {
        self.node_subgraph.get(&node_id).copied()
    }

    /// Whether any subgraphs have been defined.
    ///
    /// The layout engine uses this to short-circuit all subgraph-related
    /// processing when there are none.
    #[inline]
    pub fn has_subgraphs(&self) -> bool {
        !self.subgraphs.is_empty()
    }

    /// Get a subgraph by ID.
    #[inline]
    pub fn subgraph(&self, id: usize) -> Option<&Subgraph<'a>> {
        self.subgraphs.iter().find(|sg| sg.id == id)
    }

    /// Get all subgraphs.
    #[inline]
    pub fn subgraphs(&self) -> &[Subgraph<'a>] {
        &self.subgraphs
    }

    /// Check whether `ancestor` is an ancestor of `sg_id` in the nesting
    /// hierarchy (or is `sg_id` itself).  Used for cycle detection.
    fn is_ancestor(&self, sg_id: usize, ancestor: usize) -> bool {
        let mut current = Some(sg_id);
        while let Some(id) = current {
            if id == ancestor {
                return true;
            }
            current = self
                .subgraphs
                .iter()
                .find(|s| s.id == id)
                .and_then(|s| s.parent_id);
        }
        false
    }
}

// ── Fluent placement builders ────────────────────────────────────────────

/// Fluent builder returned by [`Graph::put_nodes`].
///
/// Call [`.inside(subgraph_id)`](NodePlacer::inside) to assign nodes to a cluster.
#[cfg(feature = "alloc")]
pub struct NodePlacer<'g, 'a, N = usize> {
    graph: &'g mut Graph<'a>,
    node_ids: &'g [N],
}

#[cfg(feature = "alloc")]
impl<'g, 'a, N: Into<NodeId> + Copy> NodePlacer<'g, 'a, N> {
    /// Assign every node in the list to the given subgraph.
    ///
    /// # Errors
    ///
    /// - [`GraphError::SubgraphNotFound`] if `sg_id` does not exist.
    /// - [`GraphError::NodeNotFound`] if any node ID is not in the graph.
    pub fn inside(self, sg_id: usize) -> Result<(), GraphError> {
        // Validate subgraph exists
        if !self.graph.subgraphs.iter().any(|s| s.id == sg_id) {
            return Err(GraphError::SubgraphNotFound(sg_id));
        }
        // Validate & assign each node. Placement only references
        // existing nodes — it never creates, so it is not an
        // id-creating site for the AUTO counter.
        for &nid in self.node_ids {
            let nid: usize = nid.into().id();
            if !self.graph.id_to_index.contains_key(&nid) {
                return Err(GraphError::NodeNotFound(nid));
            }
            self.graph.node_subgraph.insert(nid, sg_id);
        }
        Ok(())
    }
}

#[cfg(feature = "alloc")]
/// Fluent builder returned by [`Graph::put_subgraphs`].
///
/// Call [`.inside(parent_id)`](SubgraphPlacer::inside) to nest subgraphs.
pub struct SubgraphPlacer<'g, 'a> {
    graph: &'g mut Graph<'a>,
    sg_ids: &'g [usize],
}

#[cfg(feature = "alloc")]
impl<'g, 'a> SubgraphPlacer<'g, 'a> {
    /// Nest every subgraph in the list inside the given parent.
    ///
    /// # Errors
    ///
    /// - [`GraphError::SubgraphNotFound`] if `parent_id` or any child ID
    ///   does not exist.
    /// - [`GraphError::SubgraphCycle`] if nesting would create a cycle
    ///   (e.g., A inside B inside A).
    pub fn inside(self, parent_id: usize) -> Result<(), GraphError> {
        // Validate parent exists
        if !self.graph.subgraphs.iter().any(|s| s.id == parent_id) {
            return Err(GraphError::SubgraphNotFound(parent_id));
        }
        for &child_id in self.sg_ids {
            // Validate child exists
            if !self.graph.subgraphs.iter().any(|s| s.id == child_id) {
                return Err(GraphError::SubgraphNotFound(child_id));
            }
            // Cycle check: parent must not be a descendant of child
            if self.graph.is_ancestor(parent_id, child_id) {
                return Err(GraphError::SubgraphCycle);
            }
            // Set parent
            if let Some(sg) = self.graph.subgraphs.iter_mut().find(|s| s.id == child_id) {
                sg.parent_id = Some(parent_id);
            }
        }
        Ok(())
    }
}
