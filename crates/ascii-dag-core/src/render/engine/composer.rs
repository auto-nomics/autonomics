//! `SceneComposer` — the cell answers.
//!
//! A composer turns a [`Scene`] into a stream of [`CellView`]s: one
//! callback per canvas cell, row-major, each carrying the cell's
//! meaning, resolved color, and hit/pick owner. It is always
//! **color-complete** and **owner-complete** — the same scene serves a
//! terminal, an SVG writer, and an interactive picker without
//! replanning.
//!
//! The composer owns ONE retained workspace chunk; every per-compose
//! buffer (band canvas, color plane, paint scratch, the ownership
//! plane and its scratch) is carved from it by a fresh bump arena per
//! visit. Carving is pointer arithmetic — at steady state a repaint
//! allocates nothing, and there is no reset step and no `unsafe`.
//! Sizing comes from [`Scene::composition_requirements`]: scene
//! cardinalities folded through the exact carve sequence with checked
//! arithmetic. A composer accepts ANY scene whose requirements fit its
//! chunk: the heap composer grows to a new high-water mark (the only
//! allocating event); a fixed-workspace composer reports
//! [`GraphError::RenderWorkspaceTooSmall`] at preflight — byte units,
//! nothing carved.
//!
//! Composition is banded internally (the band cap comes from
//! [`ComposeBudget`], memory behavior only): band boundaries are
//! deliberately UNOBSERVABLE — the callback sees a seamless row-major
//! stream, and workspace size never affects the values.

use core::ops::ControlFlow;

use super::cell::Cell;
use super::cells::{CellKind, CellView};
use super::color::CellColor;
use super::compose::{BandCanvas, PaintScratch, composite_band};
use super::config::ComposeBudget;
use super::owner::{
    OwnerScratch, OwnerSweep, owner_incidence_capacity, owner_prepare, owner_rasterize_band,
    owner_to_hit,
};
use super::plan::RenderPlan;
use super::scene::{Scene, ViewRef};
use super::view::LayoutView;
use crate::GraphError;
use crate::graph::arena::Arena;

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

/// Workspace descriptor for one scene under one [`ComposeBudget`]:
/// scene cardinalities (exact run capacity, element counts, the
/// ownership tables' sizes), not a width×height formula. Opaque;
/// sizes a [`SceneComposer`].
#[derive(Debug, Clone, Copy)]
pub struct CompositionRequirements {
    pub(crate) band_rows_cap: usize,
    pub(crate) band_rows: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) run_capacity: usize,
    pub(crate) nodes: usize,
    pub(crate) edges: usize,
    pub(crate) subgraphs: usize,
    pub(crate) elements: usize,
    pub(crate) incidence: usize,
    pub(crate) bytes: Option<usize>,
}

impl CompositionRequirements {
    /// Bytes of workspace one composition of this scene needs —
    /// checked arithmetic over the exact carve sequence. `None` means
    /// the requirement itself overflows (the scene cannot fit any
    /// workspace).
    pub fn workspace_bytes(&self) -> Option<usize> {
        self.bytes
    }

    fn compute<V: LayoutView>(view: &V, plan: &RenderPlan<'_>, budget: &ComposeBudget) -> Self {
        let height = plan.height().max(1);
        let band_rows_cap = budget.band_rows_cap.max(1);
        let band_rows = plan.max_band_rows(band_rows_cap).max(1);
        let mut req = Self {
            band_rows_cap,
            band_rows,
            width: plan.width(),
            height,
            run_capacity: plan.run_capacity(),
            nodes: view.node_count(),
            edges: view.edge_count(),
            subgraphs: view.subgraph_count(),
            elements: plan.elements().len(),
            // `usize::MAX` marks an overflowed incidence sum; the
            // checked fold below then reports the whole requirement
            // unfittable.
            incidence: owner_incidence_capacity(plan, band_rows).unwrap_or(usize::MAX),
            bytes: None,
        };
        req.bytes = req.layout_bytes();
        req
    }

    /// Bytes of workspace a
    /// [`TerminalRenderer::new_in`](super::terminal::TerminalRenderer::new_in)
    /// needs for this scene under `emit` — the LEAN terminal profile:
    /// the same checked carve-mirrored fold as
    /// [`workspace_bytes`](Self::workspace_bytes), but with no
    /// ownership plane and a color plane only for colored emission.
    /// The semantic composer's introspection cost never becomes an
    /// unavoidable part of the terminal surface. `None` means the
    /// requirement itself overflows.
    pub fn terminal_workspace_bytes(&self, emit: &super::config::EmitOptions) -> Option<usize> {
        self.terminal_bytes(!matches!(emit.color_mode, super::color::ColorMode::None))
    }

    /// Internal bool-flag form of
    /// [`terminal_workspace_bytes`](Self::terminal_workspace_bytes).
    pub(crate) fn terminal_bytes(&self, colored: bool) -> Option<usize> {
        use core::mem::{align_of, size_of};
        let area = self.width.checked_mul(self.band_rows)?;
        let scratch_terms = PaintScratch::carve_layout(
            self.run_capacity,
            self.subgraphs,
            self.edges,
            self.nodes,
            colored,
            self.width,
            self.band_rows,
        )?;
        let extra_terms: [(Option<usize>, usize); 2] = [
            (area.checked_mul(size_of::<Cell>()), align_of::<Cell>()),
            if colored {
                (
                    area.checked_mul(size_of::<CellColor>()),
                    align_of::<CellColor>(),
                )
            } else {
                (Some(0), 1)
            },
        ];
        let mut cursor = 0usize;
        let mut max_align = 1usize;
        for &(bytes, align) in scratch_terms.iter() {
            max_align = max_align.max(align);
            cursor = cursor.checked_add(align - 1)? & !(align - 1);
            cursor = cursor.checked_add(bytes)?;
        }
        for (bytes, align) in extra_terms {
            let bytes = bytes?;
            max_align = max_align.max(align);
            cursor = cursor.checked_add(align - 1)? & !(align - 1);
            cursor = cursor.checked_add(bytes)?;
        }
        cursor.checked_add(max_align - 1)
    }

    /// Fold the exact `(bytes, align)` carve sequence — the
    /// `PaintScratch` carves in their real order, then the band
    /// canvas, the color plane, and the ownership carves — plus
    /// max-align headroom for the chunk's base pointer. Runs on every
    /// visit preflight, so: fixed-size term list, no allocation.
    fn layout_bytes(&self) -> Option<usize> {
        use core::mem::{align_of, size_of};
        let area = self.width.checked_mul(self.band_rows)?;
        let u32_term = |count: usize| (count.checked_mul(size_of::<u32>()), align_of::<u32>());

        let scratch_terms = PaintScratch::carve_layout(
            self.run_capacity,
            self.subgraphs,
            self.edges,
            self.nodes,
            true, // the composer is always color-complete
            self.width,
            self.band_rows,
        )?;
        let extra_terms: [(Option<usize>, usize); 11] = [
            (area.checked_mul(size_of::<Cell>()), align_of::<Cell>()),
            (
                area.checked_mul(size_of::<CellColor>()),
                align_of::<CellColor>(),
            ),
            u32_term(area),                           // owner plane
            u32_term(self.width.checked_add(1)?),     // claim scratch
            u32_term(self.edges),                     // edge_slot
            u32_term(self.elements),                  // by_y_min
            u32_term(self.elements),                  // active set
            u32_term(self.band_rows.checked_add(1)?), // row offsets
            u32_term(self.band_rows),                 // row cursors
            u32_term(self.incidence),                 // row incidence
            u32_term(self.nodes),                     // node→loop-record map
        ];

        let mut cursor = 0usize;
        let mut max_align = 1usize;
        for &(bytes, align) in scratch_terms.iter() {
            max_align = max_align.max(align);
            cursor = cursor.checked_add(align - 1)? & !(align - 1);
            cursor = cursor.checked_add(bytes)?;
        }
        for (bytes, align) in extra_terms {
            let bytes = bytes?;
            max_align = max_align.max(align);
            cursor = cursor.checked_add(align - 1)? & !(align - 1);
            cursor = cursor.checked_add(bytes)?;
        }
        cursor.checked_add(max_align - 1)
    }
}

impl Scene<'_, '_> {
    /// Workspace descriptor for composing this scene under `budget` —
    /// hand it to [`SceneComposer::new`] or
    /// [`SceneComposer::new_in`].
    pub fn composition_requirements(&self, budget: &ComposeBudget) -> CompositionRequirements {
        with_view!(self, v => CompositionRequirements::compute(v, self.plan(), budget))
    }
}

enum Workspace<'ws> {
    /// Growable retained chunk: grows to each new high-water mark,
    /// never shrinks, allocates nothing at steady state.
    #[cfg(feature = "alloc")]
    Heap(alloc::vec::Vec<u8>),
    /// Caller-provided fixed chunk: a scene that does not fit is a
    /// documented error, never a grow.
    Fixed(&'ws mut [u8]),
}

/// Composes [`Scene`]s into per-cell answers, retaining its workspace
/// across visits and scenes.
///
/// Construct from a scene's [`CompositionRequirements`]; the composer
/// then accepts any scene (and any number of repaints) whose
/// requirements fit — growing on the heap when they don't, erroring
/// in fixed-workspace mode. Steady-state repaint through a fitting
/// composer performs zero allocations (pinned by test).
pub struct SceneComposer<'ws> {
    ws: Workspace<'ws>,
    band_rows_cap: usize,
}

#[cfg(feature = "alloc")]
impl SceneComposer<'static> {
    /// Heap composer, presized for `requirements`; visits for larger
    /// scenes grow the workspace to their high-water mark.
    ///
    /// Unfittable requirements (`workspace_bytes()` returned `None`)
    /// presize nothing; every visit of such a scene then reports
    /// [`GraphError::RenderWorkspaceTooSmall`] instead of attempting
    /// an absurd allocation.
    pub fn new(requirements: CompositionRequirements) -> Self {
        Self {
            ws: Workspace::Heap(alloc::vec![
                0u8;
                requirements.workspace_bytes().unwrap_or(0)
            ]),
            band_rows_cap: requirements.band_rows_cap,
        }
    }
}

impl<'ws> SceneComposer<'ws> {
    /// No-alloc composer over a caller-provided workspace. Fails at
    /// construction — with BYTE units, nothing carved — when
    /// `requirements` do not fit.
    pub fn new_in(
        requirements: CompositionRequirements,
        workspace: &'ws mut [u8],
    ) -> Result<Self, GraphError> {
        let needed = requirements.workspace_bytes().unwrap_or(usize::MAX);
        if needed > workspace.len() {
            return Err(GraphError::RenderWorkspaceTooSmall {
                needed_bytes: needed,
                got_bytes: workspace.len(),
            });
        }
        Ok(Self {
            ws: Workspace::Fixed(workspace),
            band_rows_cap: requirements.band_rows_cap,
        })
    }

    /// Final cells, row-major, one callback per cell: `(x, y,
    /// CellView)`. Composed in bands internally; band boundaries are
    /// unspecified and UNOBSERVABLE — the callback sees a seamless
    /// row-major stream, and workspace size never affects the values.
    ///
    /// The `CellView` is callback-scoped (a lending visitor) — copy
    /// its fields to retain cells; the view itself cannot escape the
    /// callback.
    pub fn visit_cells<F>(&mut self, scene: &Scene<'_, '_>, mut f: F) -> Result<(), GraphError>
    where
        F: FnMut(usize, usize, CellView<'_>),
    {
        self.try_visit_cells(scene, |x, y, cell| {
            f(x, y, cell);
            ControlFlow::<core::convert::Infallible>::Continue(())
        })
        .map(|_| ())
    }

    /// Fallible/early-exit form for real sinks (an SVG writer, a TUI
    /// buffer with damage tracking): the callback can stop the visit
    /// with `ControlFlow::Break(B)` — a consumer failure or an
    /// intentional early exit — and the composer distinguishes its own
    /// errors (`Err(GraphError)`) from the consumer's break value.
    pub fn try_visit_cells<B, F>(
        &mut self,
        scene: &Scene<'_, '_>,
        f: F,
    ) -> Result<ControlFlow<B>, GraphError>
    where
        F: FnMut(usize, usize, CellView<'_>) -> ControlFlow<B>,
    {
        let cap = self.band_rows_cap;
        with_view!(scene, v => {
            let req = CompositionRequirements::compute(v, scene.plan(), &ComposeBudget::new().with_band_rows_cap(cap));
            // An overflowed requirement is an ERROR in both modes —
            // never a `usize::MAX` heap grow.
            let Some(needed) = req.workspace_bytes() else {
                let got_bytes = match &self.ws {
                    #[cfg(feature = "alloc")]
                    Workspace::Heap(buf) => buf.len(),
                    Workspace::Fixed(buf) => buf.len(),
                };
                return Err(GraphError::RenderWorkspaceTooSmall {
                    needed_bytes: usize::MAX,
                    got_bytes,
                });
            };
            let chunk: &mut [u8] = match &mut self.ws {
                #[cfg(feature = "alloc")]
                Workspace::Heap(buf) => {
                    if needed > buf.len() {
                        buf.resize(needed, 0); // the only allocating event
                    }
                    buf.as_mut_slice()
                }
                Workspace::Fixed(buf) => {
                    if needed > buf.len() {
                        return Err(GraphError::RenderWorkspaceTooSmall {
                            needed_bytes: needed,
                            got_bytes: buf.len(),
                        });
                    }
                    &mut buf[..]
                }
            };
            let got_bytes = chunk.len();
            // A fresh bump arena over the retained chunk each visit:
            // carves cannot outlive the call — no reset, no `unsafe`.
            let arena = Arena::new(chunk);
            visit_core(v, scene.plan(), &req, &arena, got_bytes, f)
        })
    }
}

/// One generic visit over the private lens — monomorphized per
/// backend; the enum dispatch above keeps the public types
/// non-generic.
fn visit_core<V: LayoutView, B, F>(
    view: &V,
    plan: &RenderPlan<'_>,
    req: &CompositionRequirements,
    arena: &Arena<'_>,
    got_bytes: usize,
    mut f: F,
) -> Result<ControlFlow<B>, GraphError>
where
    F: FnMut(usize, usize, CellView<'_>) -> ControlFlow<B>,
{
    let width = req.width;
    let area = width * req.band_rows;
    let oom = || GraphError::RenderWorkspaceTooSmall {
        needed_bytes: req.bytes.unwrap_or(usize::MAX),
        got_bytes,
    };

    let mut scratch = PaintScratch::carve(view, plan, true, req.band_rows, arena)?;
    let cells = arena.alloc_slice_default::<Cell>(area).ok_or_else(oom)?;
    let colors = arena
        .alloc_slice_default::<CellColor>(area)
        .ok_or_else(oom)?;
    let owner_plane = arena.alloc_slice_default::<u32>(area).ok_or_else(oom)?;
    let carve_u32 = |n: usize| arena.alloc_slice_default::<u32>(n).ok_or_else(oom);
    let mut owner_scratch = OwnerScratch {
        claim_next: carve_u32(width + 1)?,
        edge_slot: carve_u32(req.edges)?,
        by_y_min: carve_u32(req.elements)?,
        active: carve_u32(req.elements)?,
        row_off: carve_u32(req.band_rows + 1)?,
        row_cur: carve_u32(req.band_rows)?,
        row_inc: carve_u32(req.incidence)?,
        node_loop_slot: carve_u32(req.nodes)?,
    };
    let mut sweep = OwnerSweep::default();
    owner_prepare(plan, view, &mut owner_scratch, &mut sweep);

    if plan.height() == 0 {
        return Ok(ControlFlow::Continue(()));
    }
    let mut y0 = 0usize;
    while y0 < req.height {
        let rows = req.band_rows.min(req.height - y0);
        let mut canvas = BandCanvas::new(cells, Some(colors), width, y0, rows);
        composite_band(view, plan, &mut canvas, &mut scratch);
        let band_plane = &mut owner_plane[..width * rows];
        owner_rasterize_band(
            plan,
            view,
            y0,
            y0 + rows,
            width,
            band_plane,
            &mut owner_scratch,
            &mut sweep,
        );
        for row in 0..rows {
            let y = y0 + row;
            let cell_row = canvas.row(row);
            let color_row = canvas.color_row(row).expect("composer is color-complete");
            for x in 0..width {
                let cell = CellView {
                    kind: CellKind::from_cell(cell_row[x]),
                    color: color_row[x],
                    owner: owner_to_hit(plan, view, band_plane[row * width + x]),
                    _reserved: core::marker::PhantomData,
                };
                if let ControlFlow::Break(b) = f(x, y, cell) {
                    return Ok(ControlFlow::Break(b));
                }
            }
        }
        y0 += rows;
    }
    Ok(ControlFlow::Continue(()))
}
