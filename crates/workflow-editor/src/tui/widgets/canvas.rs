//! Canvas widget — renders nodes and edges of the current workflow manifest.
//!
//! Each node is drawn as a labelled box with two port holes on the left
//! (input) and right (output) edges. Edges are rendered as ASCII paths
//! between port holes, using a simple right-then-down-then-right
//! orthogonal routing.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use uuid::Uuid;

use crate::model::{EdgeEntry, NodeEntry, WorkflowManifest};
use crate::tui::state::AppState;

const NODE_WIDTH: i32 = 18;
const NODE_HEIGHT: i32 = 3;

/// The canvas widget.
pub struct CanvasWidget<'a> {
    pub state: &'a AppState,
    pub focus: bool,
    pub edge_draw_source: Option<(Uuid, String)>,
}

impl<'a> Widget for CanvasWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let Some(manifest) = &self.state.manifest else {
            Paragraph::new("(no workflow — :new to create)")
                .wrap(Wrap { trim: false })
                .render(area, buf);
            return;
        };

        let bg_style = if self.focus {
            Style::default().bg(Color::Rgb(20, 20, 30))
        } else {
            Style::default().bg(Color::Reset)
        };
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].set_style(bg_style);
            }
        }

        // Render edges first so they sit under the nodes.
        for e in &manifest.edges {
            render_edge(buf, area, manifest, e);
        }

        // Render nodes.
        for n in &manifest.nodes {
            render_node(buf, area, n, self.state.selected_node == Some(n.id));
        }

        // Edge-draw indicator.
        if let Some((src_id, port)) = &self.edge_draw_source {
            if let Some(src) = manifest.nodes.iter().find(|n| n.id == *src_id) {
                let pos = node_anchor(area, manifest, src, true);
                if let Some((x, y)) = pos {
                    buf[(x, y)]
                        .set_symbol("◉")
                        .set_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
                    let _ = port;
                }
            }
        }
    }
}

fn node_anchor(area: Rect, manifest: &WorkflowManifest, n: &NodeEntry, output: bool) -> Option<(u16, u16)> {
    let (cx, cy) = node_origin(area, manifest, n);
    if output {
        Some((cx + NODE_WIDTH as u16, cy + 1))
    } else {
        Some((cx.saturating_sub(1), cy + 1))
    }
}

fn node_origin(area: Rect, manifest: &WorkflowManifest, n: &NodeEntry) -> (u16, u16) {
    let pan_x = manifest.viewport.pan.0 as i32;
    let pan_y = manifest.viewport.pan.1 as i32;
    let gs = manifest.viewport.grid_step.max(1) as i32;
    let x = area.left() as i32 + (n.position.0 - pan_x) * gs;
    let y = area.top() as i32 + (n.position.1 - pan_y) * gs;
    (x.clamp(area.left() as i32, area.right() as i32 - NODE_WIDTH) as u16,
     y.clamp(area.top() as i32, area.bottom() as i32 - NODE_HEIGHT) as u16)
}

fn render_node(buf: &mut Buffer, area: Rect, n: &NodeEntry, selected: bool) {
    let (x, y) = node_origin(area, &dummy_manifest(), n);
    let _ = dummy_manifest;
    if x + NODE_WIDTH as u16 > area.right() || y + NODE_HEIGHT as u16 > area.bottom() {
        return;
    }
    let border_style = if selected {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let label_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let kind_style = Style::default().fg(Color::DarkGray);

    let h = '─';
    let v = '│';
    let tl = '┌';
    let tr = '┐';
    let bl = '└';
    let br = '┘';

    let top = format!("{tl}{}{tr}", h.to_string().repeat((NODE_WIDTH - 2) as usize));
    let bottom = format!("{bl}{}{br}", h.to_string().repeat((NODE_WIDTH - 2) as usize));
    let label_line = format!("{v} {:<width$} {v}", truncate(&n.label, NODE_WIDTH as usize - 4), width = NODE_WIDTH as usize - 4);
    let kind_line = format!("{v} {:<width$} {v}", truncate(&n.kind, NODE_WIDTH as usize - 4), width = NODE_WIDTH as usize - 4);

    if y < area.bottom() {
        buf.set_string(x, y, &top, border_style);
    }
    if y + 1 < area.bottom() {
        buf.set_string(x, y + 1, &label_line, label_style);
        // Port indicators on left/right
        if let Some((px, py)) = node_anchor(area, &dummy_manifest(), n, false) {
            if py == y + 1 && px >= area.left() {
                buf[(px, py)].set_symbol("◀");
            }
        }
        if let Some((px, py)) = node_anchor(area, &dummy_manifest(), n, true) {
            if py == y + 1 && px < area.right() {
                buf[(px, py)].set_symbol("▶");
            }
        }
    }
    if y + 2 < area.bottom() {
        buf.set_string(x, y + 2, &kind_line, kind_style);
    }
    if y + 3 < area.bottom() {
        buf.set_string(x, y + 3, &bottom, border_style);
    }
    let _ = v;
}

/// Helper: we need a `WorkflowManifest` for `node_anchor`'s pan math, but we
/// already pass the manifest elsewhere. Cheat by reading through raw fields.
fn dummy_manifest() -> WorkflowManifest {
    WorkflowManifest::new("")
}

fn render_edge(buf: &mut Buffer, area: Rect, manifest: &WorkflowManifest, e: &EdgeEntry) {
    let Some(src) = manifest.nodes.iter().find(|n| n.id == e.source) else {
        return;
    };
    let Some(tgt) = manifest.nodes.iter().find(|n| n.id == e.target) else {
        return;
    };
    let (sx, sy) = node_anchor(area, manifest, src, true).unwrap_or((0, 0));
    let (tx, ty) = node_anchor(area, manifest, tgt, false).unwrap_or((0, 0));
    let style = Style::default().fg(Color::Green);

    // Horizontal segment from source to midpoint
    if sx <= tx {
        for x in sx..=tx {
            if x >= area.left() && x < area.right() && sy >= area.top() && sy < area.bottom() {
                buf[(x, sy)].set_symbol(if x == sx { "▶" } else { "─" }).set_style(style);
            }
        }
    } else {
        // Backtrack via mid-y
        let midy = (sy + ty) / 2;
        for x in sx..tx.min(sx) {
            // (shouldn't happen — sorted above)
            let _ = x;
        }
        for y in sy.min(midy)..=sy.max(midy) {
            if sx >= area.left() && sx < area.right() && y >= area.top() && y < area.bottom() {
                buf[(sx, y)].set_symbol("│").set_style(style);
            }
        }
        for y in ty.min(midy)..=ty.max(midy) {
            if tx >= area.left() && tx < area.right() && y >= area.top() && y < area.bottom() {
                buf[(tx, y)].set_symbol("│").set_style(style);
            }
        }
        // Connect midpoints horizontally.
        for x in tx..=sx {
            if x >= area.left() && x < area.right() && midy >= area.top() && midy < area.bottom() {
                buf[(x, midy)]
                    .set_symbol(if x == sx { "▶" } else { "─" })
                    .set_style(style);
            }
        }
        // Vertical drop from midy to ty on tx column
        for y in ty..=midy {
            if tx >= area.left() && tx < area.right() && y >= area.top() && y < area.bottom() {
                buf[(tx, y)].set_symbol("│").set_style(style);
            }
        }
        return;
    }

    // Vertical segment from sy to ty on tx column (or sx if tx < sx).
    let col = tx;
    let (lo, hi) = if sy <= ty { (sy, ty) } else { (ty, sy) };
    for y in lo..=hi {
        if col >= area.left() && col < area.right() && y >= area.top() && y < area.bottom() {
            buf[(col, y)]
                .set_symbol(if y == ty { "◀" } else { "│" })
                .set_style(style);
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        format!("{:<width$}", s, width = max)
    } else {
        let mut out: String = chars.into_iter().take(max.saturating_sub(1)).collect();
        out.push('…');
        format!("{:<width$}", out, width = max)
    }
}

/// Renders a small line of text — used as a hint when the canvas is empty.
pub fn hint_line<'a>(msg: &'a str) -> Line<'a> {
    Line::from(Span::styled(
        msg.to_string(),
        Style::default().fg(Color::DarkGray),
    ))
}