//! Quote → anchor resolution for agent-created PDF highlights.
//!
//! Agents cannot know PDF geometry: `bib_add_note` therefore takes only a
//! verbatim quote (plus an optional page hint) and this module locates the
//! quote in the Zotero SDT structured text, validates the match, and derives
//! everything the PDF reader needs to paint the highlight:
//!
//! - `rects` in normalized PDF page space (`space: "pdf"`), merged per visual
//!   line from the per-character `textMap` runs, ready for the page view.
//! - `text_range` — the exact SDT selector (`[blockRef…, textNode, offset]`)
//!   Reading Mode prefers over rectangles.
//! - one anchor per page, mirroring what the reader itself stores for a
//!   selection that crosses a page break.
//!
//! The position/geometry semantics mirror the GUI side exactly (the vendored
//! `pdf-position-mapper` and SDT renderer): run entries correspond to the
//! text node's non-whitespace characters (only `' '`, `'\n'`, `'\t'` count as
//! whitespace there), offsets are UTF-16 code units, and a soft-hyphen run
//! flag drops that run's last character box because the hyphen is layout-only.

use bib_types::{AnnotationRect, AnnotationTextRange};
use serde_json::Value;

/// Per-character corpus record. Whitespace carries no geometry.
struct UnitRecord {
    text: String,
    /// Leaf-block ref path (indices into nested `content` arrays).
    block: u32,
    /// Text node index within the block's `content`.
    node: u32,
    /// UTF-16 code unit offset of this character within the text node.
    offset: u32,
    /// 0-based source page, when known.
    page: Option<u32>,
    /// PDF user-space box `[x1, y1, x2, y2]`, when known.
    rect: Option<[f64; 4]>,
}

/// Whitespace-insensitively searchable text index over one SDT document.
pub struct SdtTextIndex {
    units: Vec<UnitRecord>,
    block_paths: Vec<Vec<u32>>,
    /// Collapsed text: whitespace runs fold into single spaces, then trim.
    norm: String,
    /// `norm` byte offset of each remaining character (match conversion).
    norm_char_bytes: Vec<usize>,
    /// Corpus unit index backing each remaining `norm` character.
    norm_map: Vec<usize>,
    /// `catalog.pages[i].viewRect` — normalized-rect denominators, 0-based.
    page_views: Vec<Option<[f64; 4]>>,
    pub source_hash: Option<String>,
}

/// Where a quote was found and the geometry to store on the annotation.
#[derive(Debug, Clone)]
pub struct QuoteMatch {
    /// Normalized quote covering the whole match.
    pub quote: String,
    /// 1-based pages the match spans, in reading order.
    pub pages: Vec<u32>,
    /// One anchor per page — the reader stores one annotation per page.
    pub page_anchors: Vec<PageAnchor>,
}

#[derive(Debug, Clone)]
pub struct PageAnchor {
    /// 1-based page number stored on the annotation row.
    pub page: u32,
    /// `space: "pdf"` rectangles; the first carries `text_range`.
    pub rects: Vec<AnnotationRect>,
    /// Exact SDT selector for Reading Mode.
    pub start: Vec<u32>,
    pub end: Vec<u32>,
    /// Page-local normalized quote verified against the rendered text.
    pub quote: String,
}

#[derive(Debug, Clone)]
pub enum AnchorError {
    /// The quote does not appear in the document text.
    NotFound,
    /// The quote appears on several pages; a page hint is required.
    Ambiguous { pages: Vec<u32> },
    /// The quote exists but not on the hinted page.
    PageMismatch { hint: u32, actual: Vec<u32> },
}

impl std::fmt::Display for AnchorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let list = |pages: &[u32]| {
            pages
                .iter()
                .map(|page| page.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        match self {
            Self::NotFound => write!(
                f,
                "quote not found in the paper's text; a highlight `content` must be \
                 quoted verbatim from the paper (case-sensitive, whitespace-insensitive \
                 so line breaks are fine). Copy the sentence exactly from the full text."
            ),
            Self::Ambiguous { pages } => write!(
                f,
                "quote matches {} locations (pages: {}); call again with `page` set \
                 to the page you want to highlight",
                pages.len(),
                list(pages)
            ),
            Self::PageMismatch { hint, actual } => write!(
                f,
                "quote is not on page {hint}; it appears on page(s): {}",
                list(actual)
            ),
        }
    }
}

impl std::error::Error for AnchorError {}

impl SdtTextIndex {
    /// Walk the SDT content tree in renderer order and index every character.
    pub fn build(structure: &Value) -> Self {
        let mut units = Vec::new();
        let mut block_paths: Vec<Vec<u32>> = Vec::new();
        let page_views = structure
            .pointer("/catalog/pages")
            .and_then(Value::as_array)
            .map(|pages| {
                pages
                    .iter()
                    .map(|page| {
                        page.get("viewRect")
                            .and_then(Value::as_array)
                            .and_then(|values| rect4(values))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(content) = structure.get("content").and_then(Value::as_array) {
            for (index, node) in content.iter().enumerate() {
                let mut path = vec![index as u32];
                walk_block(node, &mut path, &mut units, &mut block_paths);
            }
        }
        let source_hash = structure
            .pointer("/metadata/source/hash")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let (norm, norm_char_bytes, norm_map) = collapse(&units);
        Self {
            units,
            block_paths,
            norm,
            norm_char_bytes,
            norm_map,
            page_views,
            source_hash,
        }
    }

    /// Locate `quote` and derive per-page anchors.
    ///
    /// `page_hint` (1-based) both disambiguates repeated quotes and is
    /// validated against where the quote actually lives.
    pub fn find_quote(
        &self,
        quote: &str,
        page_hint: Option<u32>,
    ) -> Result<QuoteMatch, AnchorError> {
        let needle = collapse_str(quote);
        if needle.is_empty() || self.norm.is_empty() {
            return Err(AnchorError::NotFound);
        }
        let candidates = self.match_ranges(&needle);
        if candidates.is_empty() {
            return Err(AnchorError::NotFound);
        }
        let mut selected = candidates.clone();
        if let Some(hint) = page_hint {
            let on_page: Vec<_> = candidates
                .iter()
                .copied()
                .filter(|range| self.pages_in(*range).contains(&hint))
                .collect();
            if on_page.is_empty() {
                let mut actual: Vec<u32> = candidates
                    .iter()
                    .flat_map(|range| self.pages_in(*range))
                    .collect();
                actual.sort_unstable();
                actual.dedup();
                return Err(AnchorError::PageMismatch { hint, actual });
            }
            selected = on_page;
        }
        if selected.len() > 1 {
            let mut pages: Vec<u32> = selected
                .iter()
                .filter_map(|range| self.pages_in(*range).first().copied())
                .collect();
            pages.sort_unstable();
            pages.dedup();
            return Err(AnchorError::Ambiguous { pages });
        }
        let range = selected[0];
        let pages = self.pages_in(range);
        let page_anchors = pages
            .iter()
            .filter_map(|page| self.page_anchor(range, *page))
            .collect();
        Ok(QuoteMatch {
            quote: needle,
            pages,
            page_anchors,
        })
    }

    /// Byte-domain substring search mapped back to corpus unit ranges.
    fn match_ranges(&self, needle: &str) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        let mut search_from = 0;
        while let Some(byte) = self.norm[search_from..].find(needle) {
            let start_byte = search_from + byte;
            let end_byte = start_byte + needle.len();
            let start_char = self
                .norm_char_bytes
                .partition_point(|offset| *offset < start_byte)
                - 1;
            let end_char = self
                .norm_char_bytes
                .partition_point(|offset| *offset < end_byte)
                - 1;
            let corpus_start = self.norm_map[start_char];
            let corpus_end = self.norm_map[end_char] + 1;
            ranges.push((corpus_start, corpus_end));
            search_from = end_byte;
        }
        ranges
    }

    /// Sorted unique 1-based pages of the geometry-bearing units in `range`.
    fn pages_in(&self, range: (usize, usize)) -> Vec<u32> {
        let mut pages: Vec<u32> = self.units[range.0..range.1]
            .iter()
            .filter_map(|unit| unit.page)
            .collect();
        pages.sort_unstable();
        pages.dedup();
        pages
    }

    fn unit_position(&self, index: usize) -> Vec<u32> {
        let unit = &self.units[index];
        let mut position = self.block_paths[unit.block as usize].clone();
        position.push(unit.node);
        position.push(unit.offset);
        position
    }

    fn page_anchor(&self, range: (usize, usize), page: u32) -> Option<PageAnchor> {
        let mut line_rects: Vec<[f64; 4]> = Vec::new();
        let mut first = range.0;
        let mut last = range.0;
        let mut seen_page_char = false;
        for index in range.0..range.1 {
            let unit = &self.units[index];
            if unit.page != Some(page - 1) {
                continue;
            }
            if let Some(rect) = unit.rect {
                line_rects.push(rect);
            }
            if !is_run_whitespace(&unit.text) {
                if !seen_page_char {
                    first = index;
                    seen_page_char = true;
                }
                last = index;
            }
        }
        if !seen_page_char {
            return None;
        }
        let quote = collapse_str(&self.span_text(first, last + 1));
        let rects = self.normalize_rects(page, &line_rects);
        if let (Some(hash), Some(first_rect)) = (self.source_hash.clone(), rects.first()) {
            // Only the first rectangle carries the Reading Mode selector, the
            // same convention the reader uses when saving a selection.
            if first_rect.text_range.is_none() {
                if let Some(rect) = rects.first() {
                    let _ = rect;
                }
            }
        }
        if let Some(hash) = self.source_hash.clone() {
            if let Some(rect) = rects.first() {
                if rect.text_range.is_none() {
                    // set below via rebuild to keep AnnotationRect fields owned
                }
            }
            let _ = hash;
        }
        let mut rects = rects;
        if let (Some(hash), Some(first_rect)) = (self.source_hash.clone(), rects.first_mut()) {
            first_rect.text_range = Some(AnnotationTextRange {
                source_hash: hash,
                start: self.unit_position(first),
                end: self.unit_position(last),
                quote: quote.clone(),
            });
        }
        Some(PageAnchor {
            page,
            start: self.unit_position(first),
            end: self.unit_position(last),
            quote,
            rects,
        })
    }

    fn span_text(&self, start: usize, end: usize) -> String {
        self.units[start..end]
            .iter()
            .map(|unit| unit.text.as_str())
            .collect()
    }

    /// Merge per-character boxes into per-line rectangles in normalized page
    /// space, using the reader's line heuristic (≥60% vertical overlap).
    fn normalize_rects(&self, page: u32, rects: &[[f64; 4]]) -> Vec<AnnotationRect> {
        let Some(view) = self.page_views.get(page as usize - 1).copied().flatten() else {
            return Vec::new();
        };
        let [vx0, vy0, vx1, vy1] = view;
        let (view_width, view_height) = (vx1 - vx0, vy1 - vy0);
        if view_width <= 0.0 || view_height <= 0.0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut lines: Vec<[f64; 4]> = Vec::new();
        for rect in rects {
            match lines.last_mut() {
                Some(current)
                    if same_line(current, rect)
                        && rect[0] - current[2]
                            <= 3.0 * (current[3] - current[1])
                                .max(rect[3] - rect[1])
                                .max(1.0) =>
                {
                    current[0] = current[0].min(rect[0]);
                    current[1] = current[1].min(rect[1]);
                    current[2] = current[2].max(rect[2]);
                    current[3] = current[3].max(rect[3]);
                }
                _ => lines.push(*rect),
            }
        }
        for rect in lines {
            push_rect(&mut out, rect, vx0, vy0, view_width, view_height);
        }
        out
    }
}

/// Normalize one merged PDF-space box into the reader's `space: "pdf"` rect.
fn push_rect(
    out: &mut Vec<AnnotationRect>,
    rect: [f64; 4],
    vx0: f64,
    vy0: f64,
    view_width: f64,
    view_height: f64,
) {
    let [x1, y1, x2, y2] = rect;
    let left = ((x1 - vx0) / view_width).clamp(0.0, 1.0);
    let right = ((x2 - vx0) / view_width).clamp(0.0, 1.0);
    // PDF space is y-up; the reader's `top` holds the normalized distance of
    // the box's bottom edge from the page bottom (see annotationDisplayBox).
    let bottom = ((y1 - vy0) / view_height).clamp(0.0, 1.0);
    let top_edge = ((y2 - vy0) / view_height).clamp(0.0, 1.0);
    let (width, height) = (right - left, top_edge - bottom);
    if width > 1e-6 && height > 1e-6 {
        out.push(AnnotationRect {
            left,
            top: bottom,
            width,
            height,
            space: Some("pdf".into()),
            text_range: None,
        });
    }
}

/// The reader's line test: vertical overlap ≥ 60% of the smaller height.
fn same_line(a: &[f64; 4], b: &[f64; 4]) -> bool {
    let overlap = a[3].min(b[3]) - a[1].max(b[1]);
    let min_height = (a[3] - a[1]).min(b[3] - b[1]).max(0.001);
    overlap / min_height >= 0.6
}

/// Run-alignment whitespace, exactly the mapper's set (`' '`, `'\n'`, `'\t'`).
fn is_run_whitespace(text: &str) -> bool {
    matches!(text, " " | "\n" | "\t")
}

/// Matching-time whitespace: same classes the GUI's `normalizeQuote` folds.
fn is_match_whitespace(ch: char) -> bool {
    ch.is_whitespace()
}

fn rect4(values: &[Value]) -> Option<[f64; 4]> {
    if values.len() < 4 {
        return None;
    }
    Some([
        values[0].as_f64()?,
        values[1].as_f64()?,
        values[2].as_f64()?,
        values[3].as_f64()?,
    ])
}

/// Depth-first walk mirroring the SDT renderer's ref-path assignment:
/// top-level blocks are `[i]`, nested blocks append their content index.
fn walk_block(
    node: &Value,
    path: &mut Vec<u32>,
    units: &mut Vec<UnitRecord>,
    block_paths: &mut Vec<Vec<u32>>,
) {
    if node.get("flowClass").and_then(Value::as_str) == Some("excluded") {
        return;
    }
    let Some(content) = node.get("content").and_then(Value::as_array) else {
        return;
    };
    let Some(first) = content.first() else {
        return;
    };
    if first.get("text").and_then(Value::as_str).is_some() {
        let block = block_paths.len() as u32;
        block_paths.push(path.clone());
        // Blocks are separated by one collapsible space in the corpus so
        // quotes copied across a paragraph break still match.
        if let Some(previous) = units.last() {
            if !is_match_whitespace(previous.text.chars().next().unwrap_or(' ')) {
                units.push(UnitRecord {
                    text: " ".into(),
                    block,
                    node: 0,
                    offset: 0,
                    page: None,
                    rect: None,
                });
            }
        }
        for (index, text_node) in content.iter().enumerate() {
            let Some(text) = text_node.get("text").and_then(Value::as_str) else {
                continue;
            };
            let runs = text_node
                .get("anchor")
                .and_then(|anchor| anchor.get("textMap"))
                .and_then(Value::as_str)
                .map(parse_runs);
            let fallback = text_node
                .get("anchor")
                .and_then(|anchor| anchor.get("pageRects"))
                .and_then(Value::as_array)
                .or_else(|| {
                    node.get("anchor")
                        .and_then(|anchor| anchor.get("pageRects"))
                        .and_then(Value::as_array)
                })
                .and_then(|rects| rects.first())
                .and_then(|rect| rect.as_array())
                .and_then(|rect| {
                    if rect.len() < 5 {
                        return None;
                    }
                    Some([
                        rect[0].as_f64()? as u32,
                        rect[1].as_f64()?,
                        rect[2].as_f64()?,
                        rect[3].as_f64()?,
                        rect[4].as_f64()?,
                    ])
                });
            push_text_node(units, block, index as u32, text, runs, fallback);
        }
    } else {
        for (index, child) in content.iter().enumerate() {
            path.push(index as u32);
            walk_block(child, path, units, block_paths);
            path.pop();
        }
    }
}

/// Index one SDT text node: align `textMap` runs with the node's non-run-
/// whitespace characters (UTF-16-unit-exact, mirroring the JS mapper).
fn push_text_node(
    units: &mut Vec<UnitRecord>,
    block: u32,
    node: u32,
    text: &str,
    runs: Option<Vec<(u32, [f64; 4])>>,
    fallback: Option<[f64; 5]>,
) {
    let mut run_boxes = runs.unwrap_or_default().into_iter();
    let use_runs = runs.is_some();
    let mut utf16_offset = 0u32;
    for ch in text.chars() {
        let offset = utf16_offset;
        utf16_offset += ch.len_utf16() as u32;
        let (mut page, mut rect) = (None, None);
        if !is_run_whitespace(&ch.to_string()) {
            // JS indexes UTF-16 units, so one astral character consumes as
            // many run entries as its code-unit width.
            for _ in 0..ch.len_utf16() {
                if let Some((run_page, run_rect)) = run_boxes.next() {
                    page = Some(run_page);
                    rect = Some(run_rect);
                }
            }
            if !use_runs && rect.is_none() && !text.trim().is_empty() {
                if let Some([fallback_page, x1, y1, x2, y2]) = fallback {
                    page = Some(fallback_page);
                    rect = Some([x1, y1, x2, y2]);
                }
            }
        }
        units.push(UnitRecord {
            text: ch.to_string(),
            block,
            node,
            offset,
            page,
            rect,
        });
    }
}

/// Decode packed textMap runs into per-character boxes in document order.
///
/// Each run is `[header, pageIndex, minX, minY, maxX, maxY, …widths]`; width
/// entries are either numbers or `[delta, width]` pairs along the run's
/// primary axis (see `structured-document-text/src/pdf/decode.js`).
fn parse_runs(text_map: &str) -> Vec<(u32, [f64; 4])> {
    let Ok(parsed) = serde_json::from_str::<Vec<Value>>(text_map) else {
        return Vec::new();
    };
    let mut boxes = Vec::new();
    for run in parsed {
        let Some(values) = run.as_array() else {
            continue;
        };
        if values.len() < 6 {
            continue;
        }
        let (Some(header), Some(page)) = (values[0].as_u64(), values[1].as_u64()) else {
            continue;
        };
        let Some(bounds) = rect4(&values[2..]) else {
            continue;
        };
        let [min_x, min_y, max_x, max_y] = bounds;
        let axis_dir = (header >> 1) & 0b11;
        let vertical = axis_dir == 1 || axis_dir == 3;
        let (start, end) = if vertical {
            (min_y, max_y)
        } else {
            (min_x, max_x)
        };
        let mut positions: Vec<(f64, f64)> = Vec::new();
        let widths = &values[6..];
        if widths.is_empty() {
            positions.push((start, end));
        } else {
            let mut cursor = start;
            for width in widths {
                if let Some(pair) = width.as_array() {
                    if pair.len() < 2 {
                        continue;
                    }
                    let (Some(delta), Some(w)) = (pair[0].as_f64(), pair[1].as_f64()) else {
                        continue;
                    };
                    cursor += delta;
                    positions.push((cursor, cursor + w));
                    cursor += w;
                } else if let Some(w) = width.as_f64() {
                    positions.push((cursor, cursor + w));
                    cursor += w;
                }
            }
        }
        // Header bit 0: the run's trailing soft hyphen is layout-only, so its
        // box is dropped to stay aligned with the text node's characters.
        if header & 1 == 1 {
            positions.pop();
        }
        for (x1, x2) in positions {
            if !(x1.is_finite() && x2.is_finite()) {
                continue;
            }
            let rect = if vertical {
                [min_x, x1, max_x, x2]
            } else {
                [x1, min_y, x2, max_y]
            };
            boxes.push((page as u32, rect));
        }
    }
    boxes
}

/// Collapse whitespace runs to single spaces and record, per remaining
/// character, its byte offset and backing corpus unit.
fn collapse(units: &[UnitRecord]) -> (String, Vec<usize>, Vec<usize>) {
    let mut norm = String::new();
    let mut char_bytes: Vec<usize> = Vec::new();
    let mut map: Vec<usize> = Vec::new();
    for (index, unit) in units.iter().enumerate() {
        let ch = unit.text.chars().next().unwrap_or(' ');
        let whitespace = is_match_whitespace(ch);
        if whitespace && norm.ends_with(' ') {
            continue;
        }
        char_bytes.push(norm.len());
        map.push(index);
        norm.push(if whitespace { ' ' } else { ch });
    }
    let mut start = 0;
    let mut end = map.len();
    while start < end && norm.as_bytes()[char_bytes[start]] == b' ' {
        start += 1;
    }
    while end > start && norm.as_bytes()[char_bytes[end - 1]] == b' ' {
        end -= 1;
    }
    let tail = if end < map.len() {
        char_bytes[end]
    } else {
        norm.len()
    };
    let norm = norm[char_bytes[start]..tail].to_owned();
    (
        norm,
        char_bytes[start..end].to_vec(),
        map[start..end].to_vec(),
    )
}

fn collapse_str(text: &str) -> String {
    let mut norm = String::new();
    for ch in text.chars() {
        if is_match_whitespace(ch) {
            if !norm.ends_with(' ') {
                norm.push(' ');
            }
        } else {
            norm.push(ch);
        }
    }
    norm.trim_matches(' ').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const VIEW: [f64; 4] = [0.0, 0.0, 612.0, 792.0];

    fn structure(content: Value) -> Value {
        json!({
            "schemaVersion": "1.2.0",
            "metadata": { "source": { "hash": "abc123" }, "processor": { "type": "pdf", "version": 15 } },
            "content": content,
            "catalog": { "pages": [ { "viewRect": VIEW }, { "viewRect": VIEW } ] },
        })
    }

    /// A single horizontal run starting at x=50 on a 612×792 page; every
    /// character gets a 10×12 box advancing 10 units, line box y 700..712.
    fn run_for(text: &str, page: u32, y: f64) -> String {
        let widths: Vec<f64> = text
            .chars()
            .filter(|ch| !matches!(ch, ' ' | '\n' | '\t'))
            .map(|_| 10.0)
            .collect();
        let mut run = vec![
            json!(0u64),
            json!(page),
            json!(50.0),
            json!(y),
            json!(50.0),
            json!(y + 12.0),
        ];
        run.extend(widths.into_iter().map(json));
        serde_json::to_string(&vec![Value::Array(run)]).unwrap()
    }

    fn paragraph(text: &str, page: u32, y: f64) -> Value {
        json!({
            "type": "paragraph",
            "content": [ { "text": text, "anchor": { "textMap": run_for(text, page, y) } } ],
        })
    }

    #[test]
    fn quote_resolves_selector_and_normalized_rects() {
        let text = "The mitochondrion is the powerhouse of the cell";
        let index = SdtTextIndex::build(&structure(json!([paragraph(text, 0, 700.0)])));
        let m = index.find_quote("mitochondrion is the", None).unwrap();
        assert_eq!(m.pages, vec![1]);
        let anchor = &m.page_anchors[0];
        assert_eq!(anchor.page, 1);
        // "The " is 4 UTF-16 units; the match starts at 'm'.
        assert_eq!(anchor.start, vec![0, 0, 4]);
        assert_eq!(anchor.end, vec![0, 0, 4 + 20]);
        // One visual line → one rect covering 21 chars × 10 units.
        assert_eq!(anchor.rects.len(), 1);
        let rect = &anchor.rects[0];
        assert_eq!(rect.space.as_deref(), Some("pdf"));
        assert!((rect.left - 60.0 / 612.0).abs() < 1e-9, "{rect:?}");
        // PDF y-up: `top` is the box's bottom edge over the page height.
        assert!((rect.top - 700.0 / 792.0).abs() < 1e-9, "{rect:?}");
        assert!((rect.width - 210.0 / 612.0).abs() < 1e-9);
        assert!((rect.height - 12.0 / 792.0).abs() < 1e-9);
        let range = rect.text_range.as_ref().unwrap();
        assert_eq!(range.source_hash, "abc123");
        assert_eq!(range.quote, "mitochondrion is the");
    }

    #[test]
    fn whitespace_collapsed_quotes_match_line_breaks() {
        let text = "Tumor suppressors regulate\nthe cell cycle tightly";
        let index = SdtTextIndex::build(&structure(json!([paragraph(text, 0, 700.0)])));
        let m = index
            .find_quote("regulate the cell\n  cycle", None)
            .unwrap();
        assert_eq!(m.quote, "regulate the cell cycle");
    }

    #[test]
    fn quote_across_blocks_matches_via_separator() {
        let index = SdtTextIndex::build(&structure(json!([
            paragraph("End of the introduction.", 0, 700.0),
            paragraph("Start of the methods section.", 0, 660.0),
        ])));
        let m = index.find_quote("introduction. Start of the methods", None).unwrap();
        let anchor = &m.page_anchors[0];
        // Selector starts in block 0 and ends in block 1.
        assert_eq!(anchor.start[..1], vec![0]);
        assert_eq!(anchor.end[..1], vec![1]);
        // Two lines → two merged rectangles.
        assert_eq!(anchor.rects.len(), 2);
    }

    #[test]
    fn repeated_quote_is_ambiguous_until_hinted() {
        let index = SdtTextIndex::build(&structure(json!([
            paragraph("Significant result reported in the study.", 0, 700.0),
            paragraph("Significant result reported in the study.", 1, 700.0),
        ])));
        let error = index
            .find_quote("Significant result", None)
            .unwrap_err();
        assert!(
            matches!(error, AnchorError::Ambiguous { ref pages } if pages == &vec![1, 2]),
            "{error}"
        );
        let m = index.find_quote("Significant result", Some(2)).unwrap();
        assert_eq!(m.pages, vec![2]);
        let error = index.find_quote("Significant result", Some(3)).unwrap_err();
        assert!(matches!(error, AnchorError::PageMismatch { hint: 3, .. }));
    }

    #[test]
    fn missing_quote_and_empty_index_report_not_found() {
        let index = SdtTextIndex::build(&structure(json!([paragraph("Known text only.", 0, 700.0)])));
        assert!(matches!(
            index.find_quote("totally absent phrase", None),
            Err(AnchorError::NotFound)
        ));
        assert!(matches!(
            index.find_quote("   \n  ", None),
            Err(AnchorError::NotFound)
        ));
    }

    #[test]
    fn missing_textmap_falls_back_to_block_rect() {
        let content = json!([{
            "type": "paragraph",
            "anchor": { "pageRects": [[0, 40.0, 300.0, 500.0, 340.0]] },
            "content": [ { "text": "Fallback paragraph without character geometry." } ],
        }]);
        let index = SdtTextIndex::build(&structure(content));
        let m = index.find_quote("Fallback paragraph", None).unwrap();
        let rect = &m.page_anchors[0].rects[0];
        assert!((rect.left - 40.0 / 612.0).abs() < 1e-9);
        assert!((rect.top - 300.0 / 792.0).abs() < 1e-9);
        assert!((rect.width - 460.0 / 612.0).abs() < 1e-9);
        assert!((rect.height - 40.0 / 792.0).abs() < 1e-9);
    }

    #[test]
    fn cross_page_quote_splits_into_one_anchor_per_page() {
        let index = SdtTextIndex::build(&structure(json!([
            paragraph("Continued from the previous page into", 0, 700.0),
            paragraph("the following page without losing text.", 1, 700.0),
        ])));
        let m = index
            .find_quote("previous page into the following page", None)
            .unwrap();
        assert_eq!(m.pages, vec![1, 2]);
        assert_eq!(m.page_anchors.len(), 2);
        let first = &m.page_anchors[0];
        let second = &m.page_anchors[1];
        assert_eq!(first.page, 1);
        assert_eq!(second.page, 2);
        assert_eq!(first.quote, "previous page into");
        assert_eq!(second.quote, "the following page");
        assert_eq!(
            first.rects[0].text_range.as_ref().unwrap().quote,
            "previous page into"
        );
        assert_eq!(
            second.rects[0].text_range.as_ref().unwrap().quote,
            "the following page"
        );
    }

    #[test]
    fn vertical_runs_and_soft_hyphen_flags_decode() {
        // Vertical run: header bits 1-2 = 1 (vertical); chars stack downward.
        let run = json!([ [ (2u64 << 1), 0, 100.0, 700.0, 112.0, 760.0, 10.0, 10.0, 10.0 ] ]);
        let content = json!([{
            "type": "paragraph",
            "content": [ { "text": "abc", "anchor": { "textMap": serde_json::to_string(&run).unwrap() } } ],
        }]);
        let index = SdtTextIndex::build(&structure(content));
        let m = index.find_quote("abc", None).unwrap();
        let rect = &m.page_anchors[0].rects[0];
        assert!((rect.left - 100.0 / 612.0).abs() < 1e-9);
        assert!((rect.width - 12.0 / 612.0).abs() < 1e-9);
        assert!((rect.top - 700.0 / 792.0).abs() < 1e-9);
        assert!((rect.height - 30.0 / 792.0).abs() < 1e-9);

        // Soft-hyphen flag: a trailing layout-only box is dropped so a 3-char
        // text with 4 encoded boxes stays aligned.
        let shy = json!([ [ 1u64, 0, 50.0, 700.0, 90.0, 712.0, 10.0, 10.0, 10.0 ] ]);
        let content = json!([{
            "type": "paragraph",
            "content": [ { "text": "abc", "anchor": { "textMap": serde_json::to_string(&shy).unwrap() } } ],
        }]);
        let index = SdtTextIndex::build(&structure(content));
        let m = index.find_quote("abc", None).unwrap();
        let rect = &m.page_anchors[0].rects[0];
        // Only two boxes remain → width covers 20 units, not 30.
        assert!((rect.width - 20.0 / 612.0).abs() < 1e-9);
    }

    #[test]
    fn nested_list_and_table_refs_follow_renderer_paths() {
        let content = json!([
            { "type": "paragraph", "content": [ { "text": "Intro line." } ] },
            { "type": "list", "ordered": false, "content": [
                { "content": [ { "text": "First item body" } ] },
                { "content": [
                    { "type": "paragraph", "content": [ { "text": "Nested block in item" } ] },
                ] },
            ] },
            { "type": "table", "content": [
                { "content": [
                    { "header": true, "content": [
                        { "type": "paragraph", "content": [ { "text": "Header cell text" } ] },
                    ] },
                ] },
            ] },
        ]);
        let index = SdtTextIndex::build(&structure(content));
        // Inline list item text lives at ref [1, 0].
        let m = index.find_quote("First item body", None).unwrap();
        assert_eq!(m.page_anchors[0].start[..2], vec![1, 0]);
        // Block child inside an item: [1, 1, 0].
        let m = index.find_quote("Nested block in item", None).unwrap();
        assert_eq!(m.page_anchors[0].start[..2], vec![1, 1, 0]);
        // Table cell child block: [2, 0, 0, 0].
        let m = index.find_quote("Header cell text", None).unwrap();
        assert_eq!(m.page_anchors[0].start[..2], vec![2, 0, 0, 0]);
    }
}
