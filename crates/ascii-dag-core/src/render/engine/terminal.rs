//! `TerminalRenderer` — retained terminal emission over a [`Scene`].
//!
//! The scene-pipeline shape of `render_with`/`render_to_bytes`: plan
//! once with a [`ScenePlanner`](super::scene::ScenePlanner), then
//! render the scene repeatedly — same emission options or different
//! ones, same renderer — with zero replanning and, at steady state,
//! zero allocation. Emission goes through exactly the same
//! plan→compose→emit path as the one-step wrappers, so output is
//! byte-identical to them for matching options (pinned by test).
//!
//! The renderer runs the LEAN terminal profile: cells only, a color
//! plane only for colored emission, no ownership plane — the semantic
//! composer's introspection cost never becomes part of the terminal
//! surface. Workspace behavior mirrors the composer: one retained
//! chunk, a fresh bump arena per render, growth on the heap, a
//! documented byte-unit error in fixed-workspace mode.

use super::cell::Cell;
use super::color::{CellColor, ColorMode};
use super::compose::PaintScratch;
use super::composer::CompositionRequirements;
use super::config::{ComposeBudget, EmitOptions};
use super::emit::ByteSink;
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

enum Workspace<'ws> {
    /// Growable retained chunk (never shrinks; steady state allocates
    /// nothing).
    #[cfg(feature = "alloc")]
    Heap(alloc::vec::Vec<u8>),
    /// Caller-provided fixed chunk: misfits are documented errors.
    Fixed(&'ws mut [u8]),
}

/// Renders [`Scene`]s to terminal text, retaining its workspace across
/// renders and scenes.
///
/// Emission options are construction state here (a renderer IS "how I
/// write"); planning options never are — one scene serves any number
/// of renderers, and [`GraphError::RenderSinkFailed`] surfaces writer
/// failures that the bare `fmt::Result` wrappers cannot name.
pub struct TerminalRenderer<'ws> {
    ws: Workspace<'ws>,
    emit: EmitOptions,
    band_rows_cap: usize,
}

#[cfg(feature = "alloc")]
impl TerminalRenderer<'static> {
    /// Heap renderer, presized for `requirements` under `emit`;
    /// renders of larger scenes grow the workspace to their
    /// high-water mark. Unfittable requirements presize nothing and
    /// every render then reports
    /// [`GraphError::RenderWorkspaceTooSmall`].
    pub fn new(emit: &EmitOptions, requirements: CompositionRequirements) -> Self {
        let colored = !matches!(emit.color_mode, ColorMode::None);
        Self {
            ws: Workspace::Heap(alloc::vec![
                0u8;
                requirements.terminal_bytes(colored).unwrap_or(0)
            ]),
            emit: *emit,
            band_rows_cap: requirements.band_rows_cap,
        }
    }
}

impl<'ws> TerminalRenderer<'ws> {
    /// No-alloc renderer over a caller-provided workspace. Fails at
    /// construction — byte units, nothing carved — when
    /// `requirements` do not fit under `emit`.
    pub fn new_in(
        emit: &EmitOptions,
        requirements: CompositionRequirements,
        workspace: &'ws mut [u8],
    ) -> Result<Self, GraphError> {
        let colored = !matches!(emit.color_mode, ColorMode::None);
        let needed = requirements.terminal_bytes(colored).unwrap_or(usize::MAX);
        if needed > workspace.len() {
            return Err(GraphError::RenderWorkspaceTooSmall {
                needed_bytes: needed,
                got_bytes: workspace.len(),
            });
        }
        Ok(Self {
            ws: Workspace::Fixed(workspace),
            emit: *emit,
            band_rows_cap: requirements.band_rows_cap,
        })
    }

    /// Render `scene` into any writer. A writer failure surfaces as
    /// [`GraphError::RenderSinkFailed`] — rendering state is
    /// unaffected and the render may be retried.
    pub fn render<W: core::fmt::Write>(
        &mut self,
        scene: &Scene<'_, '_>,
        out: &mut W,
    ) -> Result<(), GraphError> {
        self.render_impl(scene, out, |_| GraphError::RenderSinkFailed)
    }

    /// Render `scene` into a caller byte buffer (the no-alloc sink
    /// shape); returns the bytes written. An undersized buffer reports
    /// [`GraphError::RenderOutputTooSmall`].
    pub fn render_into(
        &mut self,
        scene: &Scene<'_, '_>,
        out: &mut [u8],
    ) -> Result<usize, GraphError> {
        let mut sink = ByteSink::new(out);
        // ByteSink's only failure mode is running out of buffer.
        match self.render_impl(scene, &mut sink, |_| GraphError::RenderOutputTooSmall) {
            Ok(()) => Ok(sink.written()),
            Err(e) => Err(e),
        }
    }

    fn render_impl<W: core::fmt::Write>(
        &mut self,
        scene: &Scene<'_, '_>,
        out: &mut W,
        sink_err: impl Fn(core::fmt::Error) -> GraphError,
    ) -> Result<(), GraphError> {
        let colored = !matches!(self.emit.color_mode, ColorMode::None);
        let cap = self.band_rows_cap;
        let emit = self.emit;
        with_view!(scene, v => {
            let req = scene.composition_requirements(&ComposeBudget::new().with_band_rows_cap(cap));
            let Some(needed) = req.terminal_bytes(colored) else {
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
            let arena = Arena::new(chunk);
            render_core(v, scene.plan(), &emit, &req, &arena, got_bytes, out)
                .map_err(|e| match e {
                    RenderFailure::Workspace(err) => err,
                    RenderFailure::Sink(fmt_err) => sink_err(fmt_err),
                })
        })
    }
}

impl Scene<'_, '_> {
    /// Upper bound on the bytes rendering this scene under `emit` can
    /// produce — sizes the caller's buffer for
    /// [`TerminalRenderer::render_into`]. The output-bytes third of
    /// the sizing split (scene storage:
    /// `estimate_scene_size` on the layout; workspace:
    /// [`CompositionRequirements`](super::composer::CompositionRequirements)).
    pub fn estimate_output_size(&self, emit: &EmitOptions) -> usize {
        let colored = !matches!(emit.color_mode, ColorMode::None);
        with_view!(self, v => super::emit::estimate_output_size(v, colored, emit.render_legend))
    }
}

/// Internal failure split: workspace carving vs the caller's sink.
enum RenderFailure {
    Workspace(GraphError),
    Sink(core::fmt::Error),
}

fn render_core<V: LayoutView, W: core::fmt::Write>(
    view: &V,
    plan: &RenderPlan<'_>,
    emit: &EmitOptions,
    req: &CompositionRequirements,
    arena: &Arena<'_>,
    got_bytes: usize,
    out: &mut W,
) -> Result<(), RenderFailure> {
    let colored = !matches!(emit.color_mode, ColorMode::None);
    let area = req.width * req.band_rows;
    let oom = || {
        RenderFailure::Workspace(GraphError::RenderWorkspaceTooSmall {
            needed_bytes: req.terminal_bytes(colored).unwrap_or(usize::MAX),
            got_bytes,
        })
    };
    let mut scratch = PaintScratch::carve(view, plan, colored, req.band_rows, arena)
        .map_err(RenderFailure::Workspace)?;
    let cells = arena.alloc_slice_default::<Cell>(area).ok_or_else(oom)?;
    let colors = if colored {
        Some(
            arena
                .alloc_slice_default::<CellColor>(area)
                .ok_or_else(oom)?,
        )
    } else {
        None
    };
    super::emit_bands(
        view,
        plan,
        emit,
        req.band_rows_cap,
        &mut scratch,
        cells,
        colors,
        out,
    )
    .map_err(RenderFailure::Sink)
}
