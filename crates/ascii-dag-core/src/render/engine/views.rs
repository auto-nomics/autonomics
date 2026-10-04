//! Scene element views — storage-neutral, read-only projections.
//!
//! Every view is a small `Copy` value constructed on demand from the
//! scene's private lens: no view is materialized or stored, borrowed
//! text and waypoints LEND from whichever IR backend produced the
//! layout (zero allocation, pinned by test), and no heap-only type can
//! appear in a field (`Copy` is the guardrail; the cross-backend
//! construction tests are the proof). Both backends produce
//! field-for-field identical views.

use super::color::CellColor;
use super::node_content::NodeKindTag;
use super::plan::{LabelPlan, RenderPlan};
use super::scene::{Scene, ViewRef};
use super::style::{LabelPosition, LineWeight, MarkerShape, SubgraphBorder};
use super::view::{LayoutView, PathRef};
use crate::ir::FlowAxis;

/// Real-vs-dummy: the scene's top-level node dichotomy (deliberately
/// EXHAUSTIVE — a closed promise, matchable without a wildcard).
/// Provenance detail lives inside [`Real`](Self::Real) and can grow
/// without disturbing real/dummy matches.
///
/// This is the scene vocabulary; the IR keeps its flat
/// [`ir::NodeKind`](crate::ir::NodeKind) (`Explicit | Implicit |
/// Dummy`) untouched until the 0.12 IR overhaul.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A node of the input graph.
    Real {
        /// How the node entered the graph.
        origin: NodeOrigin,
    },
    /// A routing waypoint synthesized by layout. Appears in
    /// [`Scene::nodes`] only when
    /// [`PlanOptions::show_dummy_nodes`](super::config::PlanOptions::show_dummy_nodes)
    /// is set (matching what paints and what hit-tests).
    Dummy,
}

/// How a real node entered the graph.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeOrigin {
    /// Declared via `add_node`.
    Declared,
    /// Auto-created by an edge referencing an undeclared id.
    EdgeInferred,
}

/// One node of the scene.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct NodeView<'s> {
    /// Graph id — `None` for dummies: their raw ids are synthetic and
    /// backend-specific, so the scene never exposes them. Dummies
    /// carry [`dummy_of`](Self::dummy_of) instead.
    pub id: Option<usize>,
    /// Backend-stable dummy identity: the owning edge's INPUT index
    /// (the style-callback / [`EdgeView::input_index`] convention) and
    /// the level the dummy occupies — the same pair
    /// [`HitResult::Dummy`](super::plan::HitResult::Dummy) reports.
    /// `None` for real nodes.
    pub dummy_of: Option<(usize, usize)>,
    /// Left edge, in cells.
    pub x: usize,
    /// Top edge, in rows.
    pub y: usize,
    /// Width in cells (including brackets/padding).
    pub width: usize,
    /// Height in rows.
    pub height: usize,
    /// Real-vs-dummy dichotomy.
    pub kind: NodeKind,
    /// Label text (empty for dummies).
    pub label: &'s str,
    /// Declared content kind (simple / boxed / custom).
    pub content: NodeKindTag,
    /// Custom-node payload — the **data** half of the template/data
    /// pair, rejoined by the scene ("geometry and keyed content meet
    /// again here"). `None` when the node declared no custom content.
    pub payload: Option<&'s str>,
}

/// One edge of the scene, fully resolved (style callbacks already
/// ran, exactly once, at plan time).
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct EdgeView<'s> {
    /// Position in the scene's edge list — the hit-testing, legend,
    /// and palette-default convention
    /// ([`HitResult::Edge`](super::plan::HitResult::Edge)).
    pub scene_index: usize,
    /// Original graph insertion index — the style-callback convention
    /// ([`EdgeStyleCtx::edge_index`](super::style::EdgeStyleCtx)).
    /// The two diverge when self-loops exist (self-loops are absent
    /// from the routed list).
    pub input_index: usize,
    /// Source node id.
    pub from_id: usize,
    /// Target node id.
    pub to_id: usize,
    /// Whether an arrowhead was requested.
    pub directed: bool,
    /// Reversed during cycle breaking (renders dashed by default).
    pub reversed: bool,
    /// Resolved stroke color (plain emission ignores it).
    pub color: CellColor,
    /// Resolved stroke weight.
    pub weight: LineWeight,
    /// Marker at the LOGICAL source end. Which geometric end paints
    /// follows [`reversed`](Self::reversed) — the view exposes both
    /// markers plus `reversed` so consumers can reconstruct either
    /// framing.
    pub marker_source: MarkerShape,
    /// Marker at the LOGICAL target end.
    pub marker_target: MarkerShape,
    /// Routed source anchor, physical `(x, y)`.
    pub from: (usize, usize),
    /// Routed target anchor, physical `(x, y)`.
    pub to: (usize, usize),
    /// Routed geometry.
    pub path: EdgePathView<'s>,
    /// Physical axis of the edge's trunk.
    pub flow_axis: FlowAxis,
    /// How the end at the declared source is attached — the requested
    /// side and the physical side it landed on (`from` is the cell).
    pub from_port: crate::PortAttachment,
    /// How the end at the declared target is attached (`to` is the cell).
    pub to_port: crate::PortAttachment,
    /// The edge's label, if it declared one — with its resolved color
    /// and where it ended up (inline, legend, or omitted).
    pub label: Option<LabelView<'s>>,
}

/// Storage-neutral routed path. The heap IR's `Vec` waypoints and the
/// arena IR's shared-slice waypoints both LEND here — no IR type
/// appears in the scene surface.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgePathView<'s> {
    /// Straight flow segment.
    Direct,
    /// L-shaped connection with one cross-axis segment at `bend_at`.
    Corner {
        /// The level-axis line the cross segment runs on.
        bend_at: usize,
    },
    /// Routed through a far cross-axis channel.
    SideChannel {
        /// Cross-axis line of the channel.
        channel_at: usize,
        /// Level-axis start of the channel span.
        span_start: usize,
        /// Level-axis end of the channel span.
        span_end: usize,
    },
    /// Multi-segment path through routing waypoints — physical
    /// `(x, y)` cells, lent from the IR.
    MultiSegment {
        /// The waypoint cells, in path order.
        waypoints: &'s [(usize, usize)],
        /// Level-axis offset of the first bend past the source.
        start_offset: usize,
    },
    #[cfg(feature = "ports")]
    /// An explicit orthogonal polyline — every turn stated, lent from
    /// the IR as physical `(x, y)` bend cells (see
    /// [`EdgePath::Orthogonal`](crate::ir::EdgePath::Orthogonal)).
    Orthogonal {
        /// The bend cells, in path order.
        bends: &'s [(usize, usize)],
    },
    /// A preserved self-loop: no routed path, one marker cell.
    SelfLoop {
        /// The `↺` marker cell.
        at: (usize, usize),
    },
}

/// An edge label, resolved: text, color, and where it landed.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct LabelView<'s> {
    /// Label text (never transliterated).
    pub text: &'s str,
    /// Resolved label color.
    pub color: CellColor,
    /// Where the label ended up under the plan's label policy.
    pub slot: LabelSlot,
}

/// Where a label landed.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSlot {
    /// Painted inline at `(x, y)`, spanning `len` cells (quotes
    /// included).
    Inline {
        /// Left cell of the painted span.
        x: usize,
        /// Row of the painted span.
        y: usize,
        /// Painted length in cells.
        len: usize,
    },
    /// Overflowed to the legend
    /// ([`LabelOverflow::Legend`](super::config::LabelOverflow::Legend)).
    Legend,
    /// Unplaceable, and the policy says omit — the label appears
    /// nowhere in the output.
    Omitted,
}

/// One cluster of the scene.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct SubgraphView<'s> {
    /// Subgraph id.
    pub id: usize,
    /// Immediate parent cluster, if nested.
    pub parent: Option<usize>,
    /// Cluster label.
    pub label: &'s str,
    /// Left edge, in cells.
    pub x: usize,
    /// Top edge, in rows.
    pub y: usize,
    /// Width in cells.
    pub width: usize,
    /// Height in rows.
    pub height: usize,
    /// Resolved border style.
    pub border: SubgraphBorder,
    /// Resolved border color.
    pub color: CellColor,
    /// Resolved label position.
    pub label_position: LabelPosition,
}

/// Dispatch one expression over both lens backends.
macro_rules! with_view {
    ($scene:expr, $v:ident => $e:expr) => {
        match *$scene.view() {
            #[cfg(feature = "alloc")]
            ViewRef::Heap($v) => $e,
            ViewRef::Arena($v) => $e,
        }
    };
}

impl Scene<'_, '_> {
    /// The scene's nodes. Dummies (routing waypoints) appear only
    /// when the scene was planned with `show_dummy_nodes` — matching
    /// what paints and what hit-tests.
    pub fn nodes(&self) -> impl Iterator<Item = NodeView<'_>> + '_ {
        let count = with_view!(self, v => LayoutView::node_count(v));
        let show_dummies = self.plan().show_dummy_nodes();
        (0..count)
            .map(move |i| self.node_view_at(i))
            .filter(move |n| show_dummies || !matches!(n.kind, NodeKind::Dummy))
    }

    /// One node by its graph id (real nodes only — dummies have no
    /// graph id; find them via [`nodes`](Self::nodes)).
    pub fn node(&self, id: usize) -> Option<NodeView<'_>> {
        let count = with_view!(self, v => LayoutView::node_count(v));
        (0..count)
            .map(|i| self.node_view_at(i))
            .find(|n| n.id == Some(id))
    }

    /// The scene's edges, in scene order: routed edges first, then
    /// preserved self-loops (identity, label, and resolved style
    /// intact — style callbacks ran for them at plan time).
    pub fn edges(&self) -> impl Iterator<Item = EdgeView<'_>> + '_ {
        (0..self.scene_edge_count()).map(move |i| self.edge_view_at(i))
    }

    /// One edge by scene index (self-loops sit after the routed list).
    pub fn edge(&self, scene_index: usize) -> Option<EdgeView<'_>> {
        (scene_index < self.scene_edge_count()).then(|| self.edge_view_at(scene_index))
    }

    fn scene_edge_count(&self) -> usize {
        with_view!(self, v => LayoutView::edge_count(v) + LayoutView::self_loop_count(v))
    }

    /// The scene's clusters, in declaration order.
    pub fn subgraphs(&self) -> impl Iterator<Item = SubgraphView<'_>> + '_ {
        let count = with_view!(self, v => LayoutView::subgraph_count(v));
        (0..count).map(move |i| self.subgraph_view_at(i))
    }

    /// Edges whose labels overflowed to the legend, in emission order
    /// (the same list [`legend_entries`](Self::legend_entries)
    /// indexes) — self-loop labels included: they have no inline
    /// placement host, so a labeled loop always lands here (or is
    /// omitted, per the overflow policy).
    pub fn legend(&self) -> impl Iterator<Item = EdgeView<'_>> + '_ {
        self.legend_entries()
            .iter()
            .map(move |&i| self.edge_view_at(i))
    }

    fn node_view_at(&self, index: usize) -> NodeView<'_> {
        with_view!(self, v => {
            let n = LayoutView::node(v, index);
            let (painter, payload) = LayoutView::node_custom(v, index);
            let kind = match n.kind {
                crate::ir::NodeKind::Explicit => NodeKind::Real {
                    origin: NodeOrigin::Declared,
                },
                crate::ir::NodeKind::Implicit => NodeKind::Real {
                    origin: NodeOrigin::EdgeInferred,
                },
                crate::ir::NodeKind::Dummy => NodeKind::Dummy,
            };
            let dummy = matches!(kind, NodeKind::Dummy);
            NodeView {
                id: (!dummy).then_some(n.id),
                dummy_of: if dummy {
                    Some((n.edge_index.unwrap_or(usize::MAX), n.level))
                } else {
                    None
                },
                x: n.x,
                y: n.y,
                width: n.width,
                height: n.height,
                kind,
                label: n.label,
                content: NodeKindTag::from_u8(n.content_tag),
                // The lens yields `(None, "")` for nodes without a
                // custom entry; a fully blank custom node stores no
                // entry, so this mapping is lossless.
                payload: if painter.is_none() && payload.is_empty() {
                    None
                } else {
                    Some(payload)
                },
            }
        })
    }

    fn edge_view_at(&self, index: usize) -> EdgeView<'_> {
        let plan = self.plan();
        let routed = with_view!(self, v => LayoutView::edge_count(v));
        if index >= routed {
            return self.self_loop_view_at(index, index - routed);
        }
        with_view!(self, v => {
            let e = LayoutView::edge(v, index);
            let ep = plan.edge_plan(index);
            EdgeView {
                scene_index: index,
                input_index: e.edge_index,
                from_id: e.from_id,
                to_id: e.to_id,
                directed: e.directed,
                reversed: e.reversed,
                color: ep.color,
                weight: ep.weight,
                marker_source: ep.marker_start,
                marker_target: ep.marker_end,
                from: (e.from_x, e.from_y),
                to: (e.to_x, e.to_y),
                path: match e.path {
                    // The IR's `Spline` variant is a forward-compat
                    // stub the layout engine never emits; the
                    // compositor falls back to `Direct`, and the view
                    // boundary does the same.
                    PathRef::Direct | PathRef::Spline { .. } => EdgePathView::Direct,
                    PathRef::Corner { bend_at } => EdgePathView::Corner { bend_at },
                    PathRef::SideChannel {
                        channel_at,
                        span_start,
                        span_end,
                    } => EdgePathView::SideChannel {
                        channel_at,
                        span_start,
                        span_end,
                    },
                    PathRef::MultiSegment {
                        waypoints,
                        start_offset,
                    } => EdgePathView::MultiSegment {
                        waypoints,
                        start_offset,
                    },
                    #[cfg(feature = "ports")]
                    PathRef::Orthogonal { bends } => EdgePathView::Orthogonal { bends },
                },
                flow_axis: e.flow_axis,
                from_port: e.from_port,
                to_port: e.to_port,
                label: e.label.map(|text| LabelView {
                    text,
                    color: ep.label_color,
                    slot: label_slot(plan, index),
                }),
            }
        })
    }

    /// Synthesize the view of preserved self-loop `j` (scene index
    /// `scene_index`). The marker cell anchors both endpoints; the
    /// label — which has no inline placement host — reports where it
    /// actually went (legend or omitted).
    fn self_loop_view_at(&self, scene_index: usize, j: usize) -> EdgeView<'_> {
        /// The side the `↺` cell sits on: one cell past the node on
        /// the cross axis — East under vertical flows, South under
        /// horizontal ones.
        fn loop_marker_side(flow_axis: FlowAxis) -> crate::PhysicalSide {
            match flow_axis {
                FlowAxis::Y => crate::PhysicalSide::East,
                FlowAxis::X => crate::PhysicalSide::South,
            }
        }
        let plan = self.plan();
        with_view!(self, v => {
            let r = LayoutView::self_loop(v, j);
            // O(1) record→node join through the record's stored index.
            let at = if r.node_index < LayoutView::node_count(v) {
                LayoutView::node(v, r.node_index).self_loop_at
            } else {
                None
            }
            .unwrap_or((0, 0));
            let ep = plan.scene_edge_plan(scene_index);
            let flow_axis = flow_axis_of(LayoutView::direction(v));
            EdgeView {
                scene_index,
                input_index: r.input_index,
                from_id: r.node_id,
                to_id: r.node_id,
                directed: true,
                reversed: false,
                color: ep.color,
                weight: ep.weight,
                marker_source: ep.marker_start,
                marker_target: ep.marker_end,
                from: at,
                to: at,
                path: EdgePathView::SelfLoop { at },
                flow_axis,
                // A preserved loop has no routed ends: both report the
                // marker's side (past the node on the cross axis), as
                // undeclared — declared sides on loops are deferred.
                from_port: crate::PortAttachment::auto(loop_marker_side(flow_axis)),
                to_port: crate::PortAttachment::auto(loop_marker_side(flow_axis)),
                label: r.label.map(|text| LabelView {
                    text,
                    color: ep.label_color,
                    slot: if plan.legend_entries().binary_search(&scene_index).is_ok() {
                        LabelSlot::Legend
                    } else {
                        LabelSlot::Omitted
                    },
                }),
            }
        })
    }

    fn subgraph_view_at(&self, index: usize) -> SubgraphView<'_> {
        let plan = self.plan();
        with_view!(self, v => {
            let sg = LayoutView::subgraph(v, index);
            let sp = plan.subgraph_plan(index);
            SubgraphView {
                id: sg.id,
                parent: sg.parent,
                label: sg.label,
                x: sg.x,
                y: sg.y,
                width: sg.width,
                height: sg.height,
                border: sp.border,
                color: sp.color,
                label_position: sp.label_pos,
            }
        })
    }
}

/// The physical trunk axis for a given rank direction (an `if`
/// rather than a `match` so each single-axis build sees only the
/// variants it has).
fn flow_axis_of(direction: crate::graph::Direction) -> FlowAxis {
    #[cfg(feature = "layout-horizontal")]
    if matches!(
        direction,
        crate::graph::Direction::LeftRight | crate::graph::Direction::RightLeft
    ) {
        return FlowAxis::X;
    }
    let _ = direction;
    FlowAxis::Y
}

/// Where edge `index`'s label landed under this plan. Both plan lists
/// are built in ascending edge order, so lookups are binary searches.
fn label_slot(plan: &RenderPlan<'_>, index: usize) -> LabelSlot {
    let labels = plan.labels();
    let Ok(pos) = labels.binary_search_by_key(&index, |l: &LabelPlan| l.edge_index) else {
        // An edge with label text but no label plan cannot happen for
        // routed edges today; be conservative rather than panic.
        return LabelSlot::Omitted;
    };
    let label = &labels[pos];
    if label.paints_under(plan.label_placement()) {
        LabelSlot::Inline {
            x: label.x,
            y: label.y,
            len: label.len,
        }
    } else if plan.legend_entries().binary_search(&index).is_ok() {
        LabelSlot::Legend
    } else {
        LabelSlot::Omitted
    }
}
