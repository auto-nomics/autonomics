pub(crate) use ratatui::text::{Line, Span};

use pulldown_cmark::Alignment;
use ratatui::style::{Modifier, Style};

use super::md_theme::MdTokens;
use super::text_layout::{WrappedLine, wrap_spans};

// ── Local table data ──────────────────────────────────────────────────────────

/// Lightweight table data for rendering. Contains just the parsed content
/// needed by the layout algorithm — no source-line metadata.
pub(crate) struct TableRenderData {
    pub headers: Vec<CellSpans>,
    pub rows: Vec<Vec<CellSpans>>,
    pub alignments: Vec<Alignment>,
    pub natural_widths: Vec<usize>,
}

type CellSpans = Vec<Span<'static>>;

// ── Private layout types ──────────────────────────────────────────────────────

/// Per markdown row, the wrapped output for each column plus the row's
/// physical height (max wrap-line count across columns).
///
/// # Invariants
///
/// - `cells.len() == num_cols` — one inner `Vec<WrappedLine>` per column.
/// - `height == cells.iter().map(|c| c.len()).max().unwrap_or(1)` — the
///   number of physical terminal rows this markdown row occupies after
///   wrapping. A row with all empty cells still occupies exactly one
///   physical row.
/// - Every `WrappedLine` inside `cells[c]` satisfies
///   `line.width <= col_widths[c]`, guaranteed by [`wrap_spans`].
pub(super) struct WrappedRow {
    /// Outer Vec length == num_cols. Each inner Vec is the wrapped output
    /// for that cell at its column width. Empty cells produce a single empty
    /// `WrappedLine` so `height` is always `>= 1`.
    pub(super) cells: Vec<Vec<WrappedLine>>,
    /// `max(cells[c].len())` — the number of physical terminal rows this
    /// markdown row occupies after wrapping.
    pub(super) height: usize,
}

/// Wrap every cell of every row (headers + body) to its column width.
///
/// Returns one `WrappedRow` per logical markdown row in the sequence
/// `[headers, body[0], body[1], ...]`.
///
/// # Arguments
///
/// * `headers`    – header cell spans.
/// * `body`       – body rows, each a `Vec<CellSpans>`.
/// * `col_widths` – allotted display-column width per column.
pub(super) fn wrap_table_rows(
    headers: &[CellSpans],
    body: &[Vec<CellSpans>],
    col_widths: &[usize],
) -> Vec<WrappedRow> {
    let num_cols = col_widths.len();

    // Helper: wrap one markdown row's cells to their column widths.
    let wrap_row = |cells: &[CellSpans]| -> WrappedRow {
        let wrapped_cells: Vec<Vec<WrappedLine>> = (0..num_cols)
            .map(|c| {
                let cell: &[Span<'static>] = cells.get(c).map_or(&[], |s| s.as_slice());
                let w = col_widths
                    .get(c)
                    .copied()
                    .unwrap_or(1)
                    .max(1)
                    .min(u16::MAX as usize) as u16;
                wrap_spans(cell, w)
            })
            .collect();
        // Every column produces at least one WrappedLine (wrap_spans
        // guarantees a non-empty result), so max is always Some.
        let height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
        WrappedRow {
            cells: wrapped_cells,
            height,
        }
    };

    let mut rows = Vec::with_capacity(1 + body.len());
    rows.push(wrap_row(headers));
    for row in body {
        rows.push(wrap_row(row));
    }
    rows
}

/// Emit the rendered ratatui `Line`s for one `WrappedRow` (`row.height` lines).
///
/// Top-aligns short cells: sub-rows beyond a cell's `cells[c].len()` are
/// padded with `col_widths[c]` spaces. Vertical bars are emitted on every
/// sub-row so column boundaries stay aligned.
///
/// `cell_style` is used for padding spans only; actual cell content retains
/// whatever style was set by the markdown renderer.
///
/// # Arguments
///
/// * `row`          – pre-wrapped row produced by [`wrap_table_rows`].
/// * `col_widths`   – same slice used when wrapping (widths in display columns).
/// * `alignments`   – per-column alignment from pulldown-cmark.
/// * `border_style` – style for `│` characters.
/// * `cell_style`   – style for padding / blank sub-row spans.
pub(super) fn emit_row_lines(
    row: &WrappedRow,
    col_widths: &[usize],
    alignments: &[Alignment],
    border_style: Style,
    cell_style: Style,
) -> Vec<Line<'static>> {
    let num_cols = col_widths.len();
    let mut out = Vec::with_capacity(row.height);

    for sub in 0..row.height {
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(num_cols * 4 + 1);
        spans.push(Span::styled("│".to_string(), border_style));

        for (c, &w) in col_widths.iter().enumerate().take(num_cols) {
            let alignment = alignments.get(c).copied().unwrap_or(Alignment::None);
            let cell_line: &[super::text_layout::WrappedSpan] = row
                .cells
                .get(c)
                .and_then(|lines| lines.get(sub))
                .map_or(&[], |l| l.spans.as_slice());

            // Display width of this sub-row's content.
            let cell_w: usize = cell_line.iter().map(|s| s.width as usize).sum();
            let padding = w.saturating_sub(cell_w);

            // Convert WrappedSpan -> ratatui Span (owned content, same style).
            // This is a small allocation; `WrappedSpan` content is already owned.
            let content_spans: Vec<Span<'static>> = cell_line
                .iter()
                .map(|ws| Span::styled(ws.content.clone(), ws.style))
                .collect();

            // Emit: leading space + alignment padding + content + trailing space + border.
            match alignment {
                Alignment::Right => {
                    let pad_str = format!(" {}", " ".repeat(padding));
                    spans.push(Span::styled(pad_str, cell_style));
                    spans.extend(content_spans);
                    spans.push(Span::styled(" │".to_string(), border_style));
                }
                Alignment::Center => {
                    let left = padding / 2;
                    let right = padding - left;
                    let pad_str = format!(" {}", " ".repeat(left));
                    spans.push(Span::styled(pad_str, cell_style));
                    spans.extend(content_spans);
                    let trail = format!("{} │", " ".repeat(right));
                    spans.push(Span::styled(trail, border_style));
                }
                Alignment::Left | Alignment::None => {
                    spans.push(Span::styled(" ".to_string(), cell_style));
                    spans.extend(content_spans);
                    let trail = format!("{} │", " ".repeat(padding));
                    spans.push(Span::styled(trail, border_style));
                }
            }
        }

        out.push(Line::from(spans));
    }

    out
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Lay out a table for the given `inner_width` and render it to a list of `Line`s.
///
/// When the table is too narrow to render (`inner_width < min_width`), returns a
/// single-line placeholder.
pub(crate) fn layout_table(
    table: &TableRenderData,
    inner_width: u16,
    tokens: &MdTokens,
) -> Vec<Line<'static>> {
    let num_cols = table
        .headers
        .len()
        .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));

    if num_cols == 0 {
        return vec![];
    }

    let border_style = Style::default().fg(tokens.table.border);
    let header_style = Style::default()
        .fg(tokens.table.header)
        .add_modifier(Modifier::BOLD);
    let cell_style = Style::default().fg(tokens.text.primary);
    let dim_style = Style::default().fg(tokens.text.muted);

    // Too-narrow check: need at least 1 char per cell + 2 padding + borders.
    let min_width = (num_cols * 3 + num_cols + 1).min(u16::MAX as usize) as u16;
    if inner_width < min_width {
        let placeholder = Line::from(Span::styled(
            "[ table \u{2014} too narrow, press \u{23ce} to expand ]".to_string(),
            dim_style,
        ));
        return vec![placeholder];
    }

    // Available content width after removing all borders (num_cols+1) and padding (2*num_cols).
    let target = (inner_width as usize)
        .saturating_sub(num_cols + 1)
        .saturating_sub(2 * num_cols);

    let col_widths = fair_share_widths(&table.natural_widths, num_cols, target);

    // Wrap all rows (index 0 = headers, 1..=rows.len() = body).
    let wrapped = wrap_table_rows(&table.headers, &table.rows, &col_widths);

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Top border.
    lines.push(border_line('┌', '─', '┬', '┐', &col_widths, border_style));

    // Header row(s).
    let header_row = &wrapped[0];
    lines.extend(emit_row_lines(
        header_row,
        &col_widths,
        &table.alignments,
        border_style,
        header_style,
    ));

    // Header separator.
    lines.push(border_line('├', '─', '┼', '┤', &col_widths, border_style));

    // Body rows.
    for body_row in wrapped.iter().skip(1) {
        lines.extend(emit_row_lines(
            body_row,
            &col_widths,
            &table.alignments,
            border_style,
            cell_style,
        ));
    }

    // Bottom border.
    lines.push(border_line('└', '─', '┴', '┘', &col_widths, border_style));

    lines
}

/// Compute column widths using a proportional fair-share algorithm.
///
/// If all naturals fit within `target`, returns natural widths (clamped to >= 1).
/// Otherwise, each column gets a minimum of `min(6, natural_width)`, and remaining
/// space is distributed proportionally to each column's excess over its minimum.
fn fair_share_widths(natural_widths: &[usize], num_cols: usize, target: usize) -> Vec<usize> {
    let naturals: Vec<usize> = (0..num_cols)
        .map(|i| natural_widths.get(i).copied().unwrap_or(1).max(1))
        .collect();

    let total_natural: usize = naturals.iter().sum();
    if total_natural <= target {
        return naturals;
    }

    let mins: Vec<usize> = naturals.iter().map(|&n| n.clamp(1, 6)).collect();
    let total_min: usize = mins.iter().sum();

    if total_min >= target {
        // Even minimums don't fit; distribute target evenly (each col gets at least 1).
        let per_col = (target / num_cols).max(1);
        return mins.iter().map(|&m| m.min(per_col).max(1)).collect();
    }

    let remaining = target - total_min;
    let total_excess: usize = naturals
        .iter()
        .zip(&mins)
        .map(|(&n, &m)| n.saturating_sub(m))
        .sum();

    let mut widths = mins.clone();
    for (i, (&natural, &min)) in naturals.iter().zip(&mins).enumerate() {
        let excess = natural.saturating_sub(min);
        if let Some(extra) = (excess * remaining).checked_div(total_excess) {
            widths[i] = (min + extra).min(natural);
        }
    }
    widths
}

/// Render a horizontal border line (top, separator, or bottom).
///
/// The four corner characters parameterise the three border kinds: `(┌, ─, ┬, ┐)` top,
/// `(├, ─, ┼, ┤)` separator, `(└, ─, ┴, ┘)` bottom.
pub(super) fn border_line(
    left: char,
    fill: char,
    mid: char,
    right: char,
    col_widths: &[usize],
    style: Style,
) -> Line<'static> {
    let mut s = String::with_capacity(col_widths.iter().sum::<usize>() + col_widths.len() * 4);
    s.push(left);
    for (i, &w) in col_widths.iter().enumerate() {
        // +2 for the single-space padding on each side
        for _ in 0..(w + 2) {
            s.push(fill);
        }
        if i + 1 < col_widths.len() {
            s.push(mid);
        }
    }
    s.push(right);
    Line::from(Span::styled(s, style))
}
