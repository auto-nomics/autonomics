//! `LayoutView` — one lens over both IRs (temp/06 §2, N1/M1).
//!
//! The engine is generic over this trait (monomorphized, no `dyn`), so
//! there is exactly one paint path and "backend parity" stops being a
//! discipline at the render layer — it is the type system's problem.
//!
//! The view is accessor-only: small `Copy` reference structs with
//! unified field access. Differences between the IRs are absorbed here
//! and only here (heap `&str` labels vs arena offset resolution, heap
//! `Option` vs arena sentinel conventions, heap `Vec` waypoints vs
//! arena shared-slice waypoints).

use crate::graph::Direction;
use crate::ir::NodeKind;

/// A node as the engine sees it.
#[derive(Debug, Clone, Copy)]
// Parity-by-construction (N1): every IR field is mirrored here and
// checked by the equivalence tests, whether or not the paint path reads
// it yet — adding an IR field without wiring the view must stay a
// compile error. Fields awaiting their consumer carry an allow.
pub(crate) struct NodeRef<'a> {
    pub id: usize,
    pub label: &'a str,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    #[allow(dead_code)] // no paint consumer yet (parity contract, N1)
    pub center_x: usize,
    #[allow(dead_code)] // no paint consumer yet (parity contract, N1)
    pub center_y: usize,
    #[allow(dead_code)] // no paint consumer yet (parity contract, N1)
    pub level: usize,
    #[allow(dead_code)] // no paint consumer yet (parity contract, N1)
    pub level_position: usize,
    pub kind: NodeKind,
    /// `self_loop_at.is_some()` by the D5 invariant — paint and
    /// hit-testing read the cell directly.
    #[allow(dead_code)] // superseded by self_loop_at (parity contract, N1)
    pub has_self_loop: bool,
    /// Self-loop marker cell (arena sentinel normalized to `None`).
    pub self_loop_at: Option<(usize, usize)>,
    /// Owning edge for dummy nodes; `None` for real nodes.
    #[allow(dead_code)] // consumer lands with RW7 dummy introspection
    pub edge_index: Option<usize>,
    /// Declared content kind (raw `NodeKindTag` value) — routes
    /// `paint_node`.
    pub content_tag: u8,
}

/// An edge path as the engine sees it — waypoints are a plain slice in
/// both backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathRef<'a> {
    Direct,
    Corner {
        bend_at: usize,
    },
    SideChannel {
        channel_at: usize,
        span_start: usize,
        span_end: usize,
    },
    MultiSegment {
        waypoints: &'a [(usize, usize)],
        start_offset: usize,
    },
    #[cfg(feature = "ports")]
    /// Explicit polyline — every bend stated (see
    /// `EdgePath::Orthogonal`).
    Orthogonal {
        bends: &'a [(usize, usize)],
    },
    Spline {
        cp1_x: usize,
        cp1_y: usize,
        cp2_x: usize,
        cp2_y: usize,
    },
}

/// An edge as the engine sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EdgeRef<'a> {
    pub from_id: usize,
    pub to_id: usize,
    pub from_x: usize,
    pub from_y: usize,
    pub to_x: usize,
    pub to_y: usize,
    pub edge_index: usize,
    /// `None` when the edge has no label (arena: empty label storage).
    pub label: Option<&'a str>,
    /// Meaningful iff `label.is_some()`.
    pub label_x: usize,
    /// Meaningful iff `label.is_some()`.
    pub label_y: usize,
    pub directed: bool,
    pub reversed: bool,
    /// Physical axis of the edge's trunk (temp/08 D2) — selects the
    /// compositor's paint path.
    pub flow_axis: crate::ir::FlowAxis,
    pub path: PathRef<'a>,
    pub from_port: crate::PortAttachment,
    pub to_port: crate::PortAttachment,
}

/// A preserved self-loop as the engine sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SelfLoopRef<'a> {
    /// The node the loop is on (user id).
    pub node_id: usize,
    /// The node's position in the view's node table (O(1) join;
    /// resolved at layout — hand-built IRs own its bounds).
    pub node_index: usize,
    /// Original graph insertion index (the style-callback convention).
    pub input_index: usize,
    /// `None` when unlabeled (arena: empty label storage).
    pub label: Option<&'a str>,
}

/// A subgraph (cluster) as the engine sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SubgraphRef<'a> {
    pub id: usize,
    /// Parent subgraph id; `None` for root-level clusters.
    pub parent: Option<usize>,
    pub label: &'a str,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// Accessor-only view over a laid-out graph. Implemented by both IRs;
/// the engine never touches an IR type directly.
pub(crate) trait LayoutView {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    #[allow(dead_code)] // no paint consumer yet (parity contract, N1)
    fn level_count(&self) -> usize;
    /// Never consulted by paint code (M4 — flow derives from
    /// coordinates); exposed for introspection consumers.
    #[allow(dead_code)]
    fn direction(&self) -> Direction;
    fn node_count(&self) -> usize;
    fn node(&self, index: usize) -> NodeRef<'_>;
    /// A node's declared custom content: painter + payload. Returns
    /// `(None, "")` for nodes without an entry — including blank
    /// custom nodes, which reserve their area and paint nothing.
    fn node_custom(&self, index: usize) -> (Option<super::style::NodePaintFn>, &str);
    fn edge_count(&self) -> usize;
    fn edge(&self, index: usize) -> EdgeRef<'_>;
    fn subgraph_count(&self) -> usize;
    fn subgraph(&self, index: usize) -> SubgraphRef<'_>;
    fn self_loop_count(&self) -> usize;
    fn self_loop(&self, index: usize) -> SelfLoopRef<'_>;
}

// ── Heap IR ──────────────────────────────────────────────────────────────

#[cfg(feature = "alloc")]
impl LayoutView for crate::ir::LayoutIR<'_> {
    fn width(&self) -> usize {
        crate::ir::LayoutIR::width(self)
    }

    fn height(&self) -> usize {
        crate::ir::LayoutIR::height(self)
    }

    fn level_count(&self) -> usize {
        crate::ir::LayoutIR::level_count(self)
    }

    fn direction(&self) -> Direction {
        crate::ir::LayoutIR::direction(self)
    }

    fn node_count(&self) -> usize {
        self.nodes().len()
    }

    fn node(&self, index: usize) -> NodeRef<'_> {
        let n = &self.nodes()[index];
        NodeRef {
            id: n.id,
            label: n.label,
            x: n.x,
            y: n.y,
            width: n.width,
            height: n.height,
            center_x: n.center_x,
            center_y: n.center_y,
            level: n.level,
            level_position: n.level_position,
            kind: n.kind,
            has_self_loop: n.has_self_loop,
            self_loop_at: n.self_loop_at,
            edge_index: n.edge_index,
            content_tag: n.content_tag,
        }
    }

    fn node_custom(&self, index: usize) -> (Option<super::style::NodePaintFn>, &str) {
        match self
            .custom_nodes
            .binary_search_by_key(&index, |entry| entry.0)
        {
            Ok(pos) => {
                let (_, painter, payload) = self.custom_nodes[pos];
                (painter, payload)
            }
            Err(_) => (None, ""),
        }
    }

    fn edge_count(&self) -> usize {
        self.edges().len()
    }

    fn edge(&self, index: usize) -> EdgeRef<'_> {
        let e = &self.edges()[index];
        let path = match &e.path {
            crate::ir::EdgePath::Direct => PathRef::Direct,
            crate::ir::EdgePath::Corner { bend_at } => PathRef::Corner { bend_at: *bend_at },
            crate::ir::EdgePath::SideChannel {
                channel_at,
                span_start,
                span_end,
            } => PathRef::SideChannel {
                channel_at: *channel_at,
                span_start: *span_start,
                span_end: *span_end,
            },
            crate::ir::EdgePath::MultiSegment {
                waypoints,
                start_offset,
            } => PathRef::MultiSegment {
                waypoints: waypoints.as_slice(),
                start_offset: *start_offset,
            },
            #[cfg(feature = "ports")]
            crate::ir::EdgePath::Orthogonal { bends } => PathRef::Orthogonal {
                bends: bends.as_slice(),
            },
            crate::ir::EdgePath::Spline {
                cp1_x,
                cp1_y,
                cp2_x,
                cp2_y,
            } => PathRef::Spline {
                cp1_x: *cp1_x,
                cp1_y: *cp1_y,
                cp2_x: *cp2_x,
                cp2_y: *cp2_y,
            },
        };
        EdgeRef {
            from_id: e.from_id,
            to_id: e.to_id,
            from_x: e.from_x,
            from_y: e.from_y,
            to_x: e.to_x,
            to_y: e.to_y,
            edge_index: e.edge_index,
            label: e.label,
            label_x: e.label_x,
            label_y: e.label_y,
            directed: e.directed,
            reversed: e.reversed,
            flow_axis: e.flow_axis,
            path,
            from_port: e.from_port,
            to_port: e.to_port,
        }
    }

    fn subgraph_count(&self) -> usize {
        self.subgraphs().len()
    }

    fn subgraph(&self, index: usize) -> SubgraphRef<'_> {
        let sg = &self.subgraphs()[index];
        SubgraphRef {
            id: sg.id,
            parent: sg.parent_id,
            label: sg.label,
            x: sg.x,
            y: sg.y,
            width: sg.width,
            height: sg.height,
        }
    }

    fn self_loop_count(&self) -> usize {
        self.self_loops().len()
    }

    fn self_loop(&self, index: usize) -> SelfLoopRef<'_> {
        let r = &self.self_loops()[index];
        SelfLoopRef {
            node_id: r.node_id,
            node_index: r.node_index,
            input_index: r.edge_index,
            // Empty = none, even for hand-built records (the arena
            // twin's len-0 storage cannot say Some("")).
            label: r.label.filter(|l| !l.is_empty()),
        }
    }
}

// ── Arena IR ─────────────────────────────────────────────────────────────

impl LayoutView for crate::ir::arena::LayoutIRArena<'_> {
    fn width(&self) -> usize {
        crate::ir::arena::LayoutIRArena::width(self)
    }

    fn height(&self) -> usize {
        crate::ir::arena::LayoutIRArena::height(self)
    }

    fn level_count(&self) -> usize {
        crate::ir::arena::LayoutIRArena::level_count(self)
    }

    fn direction(&self) -> Direction {
        crate::ir::arena::LayoutIRArena::direction(self)
    }

    fn node_count(&self) -> usize {
        crate::ir::arena::LayoutIRArena::node_count(self)
    }

    fn node(&self, index: usize) -> NodeRef<'_> {
        let n = self.node(index);
        NodeRef {
            id: n.id,
            label: self.node_label(index),
            x: n.x,
            y: n.y,
            width: n.width,
            height: n.height,
            center_x: n.center_x,
            center_y: n.center_y,
            level: n.level,
            level_position: n.level_position,
            kind: n.kind,
            has_self_loop: n.has_self_loop,
            self_loop_at: if n.self_loop_at == (usize::MAX, usize::MAX) {
                None
            } else {
                Some(n.self_loop_at)
            },
            edge_index: if n.edge_index == usize::MAX {
                None
            } else {
                Some(n.edge_index)
            },
            content_tag: n.content_tag,
        }
    }

    fn node_custom(&self, index: usize) -> (Option<super::style::NodePaintFn>, &str) {
        let entries = self.custom_nodes();
        match entries.binary_search_by_key(&index, |entry| entry.node_idx) {
            Ok(pos) => {
                let entry = &entries[pos];
                (entry.painter, self.custom_payload(entry))
            }
            Err(_) => (None, ""),
        }
    }

    fn edge_count(&self) -> usize {
        crate::ir::arena::LayoutIRArena::edge_count(self)
    }

    fn edge(&self, index: usize) -> EdgeRef<'_> {
        let e = self.edge(index);
        let path = match e.path {
            crate::ir::arena::EdgePathArena::Direct => PathRef::Direct,
            crate::ir::arena::EdgePathArena::Corner { bend_at } => PathRef::Corner { bend_at },
            crate::ir::arena::EdgePathArena::SideChannel {
                channel_at,
                span_start,
                span_end,
            } => PathRef::SideChannel {
                channel_at,
                span_start,
                span_end,
            },
            crate::ir::arena::EdgePathArena::MultiSegment {
                waypoints_start,
                waypoints_len,
                start_offset,
            } => PathRef::MultiSegment {
                waypoints: self.edge_waypoints_raw(waypoints_start, waypoints_len),
                start_offset,
            },
            #[cfg(feature = "ports")]
            crate::ir::arena::EdgePathArena::Orthogonal {
                bends_start,
                bends_len,
            } => PathRef::Orthogonal {
                bends: self.edge_waypoints_raw(bends_start, bends_len),
            },
            crate::ir::arena::EdgePathArena::Spline {
                cp1_x,
                cp1_y,
                cp2_x,
                cp2_y,
            } => PathRef::Spline {
                cp1_x,
                cp1_y,
                cp2_x,
                cp2_y,
            },
        };
        EdgeRef {
            from_id: e.from_id,
            to_id: e.to_id,
            from_x: e.from_x,
            from_y: e.from_y,
            to_x: e.to_x,
            to_y: e.to_y,
            edge_index: e.edge_index,
            label: if e.label_len > 0 {
                Some(self.edge_label(index))
            } else {
                None
            },
            label_x: e.label_x,
            label_y: e.label_y,
            directed: e.directed,
            reversed: e.reversed,
            flow_axis: e.flow_axis,
            path,
            from_port: e.from_port,
            to_port: e.to_port,
        }
    }

    fn subgraph_count(&self) -> usize {
        crate::ir::arena::LayoutIRArena::subgraph_count(self)
    }

    fn subgraph(&self, index: usize) -> SubgraphRef<'_> {
        let sg = &self.subgraphs()[index];
        SubgraphRef {
            id: sg.id,
            parent: if sg.parent_idx == usize::MAX {
                None
            } else {
                Some(sg.parent_idx)
            },
            label: self.subgraph_label(index),
            x: sg.x,
            y: sg.y,
            width: sg.width,
            height: sg.height,
        }
    }

    fn self_loop_count(&self) -> usize {
        self.self_loops().len()
    }

    fn self_loop(&self, index: usize) -> SelfLoopRef<'_> {
        let r = &self.self_loops()[index];
        SelfLoopRef {
            node_id: r.node_id,
            node_index: r.node_index,
            input_index: r.edge_index,
            label: if r.label_len == 0 {
                None
            } else {
                Some(self.self_loop_label(index))
            },
        }
    }
}

// ── Accessor-equivalence tests (RW1 exit criteria) ───────────────────────
