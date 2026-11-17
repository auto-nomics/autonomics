//! Deterministic layout of explicit relations; does not infer scientific mechanisms.
use super::sketch::{InvalidPlan, Layout, Plan, Shape};
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone)]
pub(crate) struct Box {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub label: String,
    pub label_width: f64,
    pub label_height: f64,
}
impl Box {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2., self.y + self.height / 2.)
    }
}
pub(crate) struct Drawing {
    pub boxes: Vec<Box>,
    pub routes: Vec<Vec<[f64; 2]>>,
    pub feedback_edges: usize,
}
fn glyph_width(c: char, font_size: f64) -> f64 {
    let advance = crate::font_metrics::ADVANCES
        .binary_search_by_key(&(c as u32), |(cp, _)| *cp)
        .ok()
        .map(|index| {
            f64::from(crate::font_metrics::ADVANCES[index].1) / crate::font_metrics::UNITS_PER_EM
        })
        .unwrap_or(1.1); // Conservative fallback for glyphs outside the shipped font.
    font_size * advance
}
fn kerning(a: char, b: char, font_size: f64) -> f64 {
    crate::font_metrics::KERNING
        .binary_search_by_key(&(a as u32, b as u32), |(a, b, _)| (*a, *b))
        .ok()
        .map_or(0., |i| {
            font_size * f64::from(crate::font_metrics::KERNING[i].2)
                / crate::font_metrics::UNITS_PER_EM
        })
}
pub(crate) fn text_width(label: &str, font_size: f64) -> f64 {
    label
        .lines()
        .map(|line| {
            let mut previous = None;
            line.chars()
                .map(|c| {
                    let width = glyph_width(c, font_size)
                        + previous.map_or(0., |p| kerning(p, c, font_size));
                    previous = Some(c);
                    width
                })
                .sum::<f64>()
        })
        .fold(0., f64::max)
}
/// Keep short relations readable on one line; longer captions retain a bounded
/// three-line width budget. Explicit newlines remain under the author's control.
pub(crate) fn edge_label_width(label: &str) -> f64 {
    let natural = text_width(label, 16.);
    if natural <= 180. {
        natural.max(40.)
    } else {
        (natural / 3.).clamp(104., 240.).min(natural.max(40.))
    }
}
/// Keep fitting Latin/Greek identifier runs intact at UAX hyphen opportunities.
/// CJK remains breakable; overlong runs retain the existing overflow fallback.
fn fitting_identifiers(text: &str, width: f64, font_size: f64) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = None;
    for (position, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if c.is_ascii_alphanumeric()
            || matches!(c, '\u{0370}'..='\u{03ff}' | '\u{1f00}'..='\u{1fff}' | '-' | '_')
        {
            start.get_or_insert(position);
        } else if let Some(begin) = start.take()
            && text_width(&text[begin..position], font_size) <= width
        {
            ranges.push((begin, position));
        }
    }
    ranges
}
/// UAX #14 opportunities keep CJK punctuation and ordinary words together.
/// Overlong complex/unit tokens first break at separators or before opening
/// parentheses, then by characters.
pub(crate) fn wrap(label: &str, width: f64, font_size: f64) -> String {
    let mut lines = Vec::new();
    let normalized = label
        .replace("\r\n", "\n")
        .replace(['\r', '\u{2028}', '\u{2029}'], "\n");
    for paragraph in normalized.split('\n') {
        let mut remaining = paragraph.trim_matches(' ');
        if remaining.is_empty() {
            lines.push(String::new());
            continue;
        }
        let protected = fitting_identifiers(remaining, width, font_size);
        let paragraph_length = remaining.len();
        while !remaining.is_empty() {
            let offset = paragraph_length - remaining.len();
            let mut end = 0;
            for (position, opportunity) in unicode_linebreak::linebreaks(remaining) {
                if protected
                    .iter()
                    .any(|(a, b)| *a < offset + position && offset + position < *b)
                {
                    continue;
                }
                if text_width(remaining[..position].trim_end_matches(' '), font_size) <= width {
                    end = position;
                    if opportunity == unicode_linebreak::BreakOpportunity::Mandatory {
                        break;
                    }
                } else {
                    break;
                }
            }
            // Preserve identifiers in overlong complex/unit strings before
            // resorting to character breaks. Ordinary UAX word opportunities
            // retain priority, so a compound that fits a fresh line stays whole.
            if end == 0 {
                let mut previous: Option<char> = None;
                let mut chars = remaining.char_indices().peekable();
                while let Some((index, c)) = chars.next() {
                    // UAX #14 can glue an identifier to a parenthetical formula.
                    // Move the opening bracket with its contents to a fresh line
                    // before splitting a fitting formula such as ADP→ATP.
                    if matches!(c, '(' | '（' | '[' | '［' | '{' | '｛')
                        && index > 0
                        && previous.is_some_and(char::is_alphanumeric)
                        && chars.peek().is_some_and(|(_, next)| next.is_alphanumeric())
                    {
                        if text_width(&remaining[..index], font_size) > width {
                            break;
                        }
                        end = index;
                    }
                    if matches!(c, ':' | '/')
                        && previous.is_some_and(char::is_alphanumeric)
                        && chars.peek().is_some_and(|(_, next)| next.is_alphanumeric())
                    {
                        let next = index + c.len_utf8();
                        if text_width(&remaining[..next], font_size) > width {
                            break;
                        }
                        end = next;
                    }
                    previous = Some(c);
                }
            }
            if end == 0 {
                for (index, c) in remaining.char_indices() {
                    let next = index + c.len_utf8();
                    if end != 0 && text_width(&remaining[..next], font_size) > width {
                        break;
                    }
                    end = next;
                }
            }
            lines.push(remaining[..end].trim_end_matches(' ').to_owned());
            remaining = remaining[end..].trim_start_matches(' ');
        }
    }
    lines.join("\n")
}

/// Balance automatically wrapped headings within the existing line count and box.
/// Keep explicit user line breaks and identifiers that fit in the original wrap.
pub(crate) fn balanced_heading(label: &str, width: f64, font_size: f64) -> String {
    let baseline = wrap(label, width, font_size);
    let lines = baseline.lines().count();
    if lines < 2 || label.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return baseline;
    }
    // Protect complete fitting identifier/word runs, including Chinese runs
    // separated by spaces or punctuation. Do not invent biological word breaks.
    let mut protected = Vec::new();
    let mut start = None;
    for (position, c) in label
        .char_indices()
        .chain(std::iter::once((label.len(), ' ')))
    {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            start.get_or_insert(position);
        } else if let Some(begin) = start.take()
            && text_width(&label[begin..position], font_size) <= width
        {
            protected.push((begin, position));
        }
    }
    let mut cuts = vec![0];
    cuts.extend(
        unicode_linebreak::linebreaks(label)
            .map(|(position, _)| position)
            .filter(|position| !protected.iter().any(|(a, b)| a < position && position < b)),
    );
    if cuts.last() != Some(&label.len()) {
        cuts.push(label.len());
    }
    let target = text_width(label, font_size) / lines as f64;
    let baseline_score = baseline
        .lines()
        .map(|line| (text_width(line, font_size) - target).powi(2))
        .sum::<f64>();
    // At most 120 source characters for panel titles; fixed line count and
    // existing box width bound this search. Store predecessors, not text copies.
    let mut best = vec![vec![None::<(f64, usize)>; cuts.len()]; lines + 1];
    best[0][0] = Some((0., 0));
    for line in 1..=lines {
        for end in 1..cuts.len() {
            for begin in 0..end {
                let Some((score, _)) = best[line - 1][begin] else {
                    continue;
                };
                let text = label[cuts[begin]..cuts[end]].trim_matches(' ');
                if text.is_empty() {
                    continue;
                }
                let measured = text_width(text, font_size);
                if measured > width {
                    continue;
                }
                let candidate = score + (measured - target).powi(2);
                if best[line][end].is_none_or(|(previous, _)| candidate < previous) {
                    best[line][end] = Some((candidate, begin));
                }
            }
        }
    }
    let Some((score, _)) = best[lines][cuts.len() - 1] else {
        return baseline;
    };
    if score + 1e-6 >= baseline_score {
        return baseline;
    }
    let mut end = cuts.len() - 1;
    let mut result = Vec::new();
    for line in (1..=lines).rev() {
        let begin = best[line][end].expect("reachable predecessor").1;
        result.push(label[cuts[begin]..cuts[end]].trim_matches(' '));
        end = begin;
    }
    result.reverse();
    result.join("\n")
}

fn ranks(plan: &Plan, indices: &HashMap<&str, usize>) -> Vec<usize> {
    let n = plan.nodes.len();
    let mut reach = vec![vec![false; n]; n];
    for (i, row) in reach.iter_mut().enumerate() {
        row[i] = true;
    }
    for edge in &plan.edges {
        reach[indices[edge.from.as_str()]][indices[edge.to.as_str()]] = true;
    }
    for via in 0..n {
        for from in 0..n {
            if reach[from][via] {
                for to in 0..n {
                    reach[from][to] |= reach[via][to];
                }
            }
        }
    }
    // Inside strongly connected components, input order breaks layout cycles.
    // All supplied relations remain in the output, including feedback edges.
    let mut component = vec![usize::MAX; n];
    let mut members: Vec<Vec<usize>> = Vec::new();
    for i in 0..n {
        if component[i] != usize::MAX {
            continue;
        }
        let id = members.len();
        let mut group = Vec::new();
        for j in i..n {
            if component[j] == usize::MAX && reach[i][j] && reach[j][i] {
                component[j] = id;
                group.push(j);
            }
        }
        members.push(group);
    }
    let mut local_rank = vec![0; n];
    let mut spans = vec![1; members.len()];
    for (id, group) in members.iter().enumerate() {
        for &node in group {
            for edge in &plan.edges {
                let (a, b) = (indices[edge.from.as_str()], indices[edge.to.as_str()]);
                if b == node && component[a] == id && a < b {
                    local_rank[b] = local_rank[b].max(local_rank[a] + 1);
                }
            }
            spans[id] = spans[id].max(local_rank[node] + 1);
        }
    }
    let mut adjacency = vec![BTreeSet::new(); members.len()];
    let mut incoming = vec![0; members.len()];
    for edge in &plan.edges {
        let (a, b) = (
            component[indices[edge.from.as_str()]],
            component[indices[edge.to.as_str()]],
        );
        if a != b && adjacency[a].insert(b) {
            incoming[b] += 1;
        }
    }
    let mut ready: BTreeSet<_> = (0..members.len()).filter(|i| incoming[*i] == 0).collect();
    let mut rank = vec![0; members.len()];
    while let Some(id) = ready.pop_first() {
        for &next in &adjacency[id] {
            rank[next] = rank[next].max(rank[id] + spans[id]);
            incoming[next] -= 1;
            if incoming[next] == 0 {
                ready.insert(next);
            }
        }
    }
    let mut result = vec![0; n];
    for (id, group) in members.iter().enumerate() {
        for &node in group {
            result[node] = rank[id] + local_rank[node];
        }
    }
    result
}
fn order_layers(plan: &Plan, ranks: &[usize], indices: &HashMap<&str, usize>) -> Vec<Vec<usize>> {
    let mut layers = vec![Vec::new(); ranks.iter().copied().max().unwrap_or(0) + 1];
    for (node, &rank) in ranks.iter().enumerate() {
        layers[rank].push(node);
    }
    // Forward/backward barycenter sweeps reduce avoidable crossings while
    // keeping deterministic tie order. Feedback edges do not drive ordering.
    for _ in 0..4 {
        for forward in [true, false] {
            let range: Vec<_> = if forward {
                (0..layers.len()).collect()
            } else {
                (0..layers.len()).rev().collect()
            };
            for rank in range {
                let mut slots = vec![0.; ranks.len()];
                for layer in &layers {
                    for (slot, &node) in layer.iter().enumerate() {
                        slots[node] = slot as f64;
                    }
                }
                let score = |node: usize| {
                    let peers: Vec<_> = plan
                        .edges
                        .iter()
                        .filter_map(|edge| {
                            let (a, b) = (indices[edge.from.as_str()], indices[edge.to.as_str()]);
                            if forward && b == node && ranks[a] < rank {
                                Some(slots[a])
                            } else if !forward && a == node && ranks[b] > rank {
                                Some(slots[b])
                            } else {
                                None
                            }
                        })
                        .collect();
                    if peers.is_empty() {
                        slots[node]
                    } else {
                        peers.iter().sum::<f64>() / peers.len() as f64
                    }
                };
                layers[rank].sort_by(|a, b| score(*a).total_cmp(&score(*b)).then(a.cmp(b)));
            }
        }
    }
    layers
}
pub(crate) fn arrange(plan: &Plan) -> Result<Drawing, InvalidPlan> {
    let fail = |message: &str| InvalidPlan(message.into());
    let mut indices = HashMap::new();
    let mut cells = BTreeSet::new();
    for (i, node) in plan.nodes.iter().enumerate() {
        if node.id.trim().is_empty()
            || node.label.trim().is_empty()
            || node.label.chars().count() > 120
            || indices.insert(node.id.as_str(), i).is_some()
        {
            return Err(fail(
                "node IDs must be unique and labels must contain 1..120 characters",
            ));
        }
        if plan.layout == Layout::Grid {
            let (Some(column), Some(row)) = (node.column, node.row) else {
                return Err(fail("grid layout requires each node's column and row"));
            };
            if column > 30 || row > 30 || !cells.insert((column, row)) {
                return Err(fail("grid positions must be unique and within 0..30"));
            }
        } else if node.column.is_some() || node.row.is_some() {
            return Err(fail(
                "automatic layouts do not accept grid positions; omit column/row or choose grid",
            ));
        }
    }
    for edge in &plan.edges {
        if !indices.contains_key(edge.from.as_str()) || !indices.contains_key(edge.to.as_str()) {
            return Err(fail("edge endpoint does not exist"));
        }
        if edge.from == edge.to {
            return Err(fail("self edges require explicit manual layout"));
        }
        if edge.label.chars().count() > 120 {
            return Err(fail("edge labels support at most 120 characters"));
        }
    }
    let mut boxes: Vec<_> = plan
        .nodes
        .iter()
        .map(|node| {
            let width = 220.;
            let label_width = if matches!(node.shape, Shape::Ellipse) {
                140.
            } else {
                200.
            };
            let label = wrap(&node.label, label_width, 20.);
            let label_height = label.lines().count().max(1) as f64 * 25.;
            let height = if matches!(node.shape, Shape::Ellipse) {
                ((label_height + 20.) * std::f64::consts::SQRT_2)
                    .ceil()
                    .max(100.)
            } else {
                (label_height + 20.).max(100.)
            };
            Box {
                x: 0.,
                y: 0.,
                width,
                height,
                label,
                label_width,
                label_height,
            }
        })
        .collect();
    let ranks = if plan.layout == Layout::Grid {
        vec![]
    } else {
        ranks(plan, &indices)
    };
    if plan.layout == Layout::Grid {
        let caption_height = plan
            .edges
            .iter()
            .map(|edge| {
                wrap(&edge.label, edge_label_width(&edge.label), 16.)
                    .lines()
                    .count()
                    .max(1) as f64
                    * 20.
            })
            .fold(0., f64::max);
        let row_step =
            boxes.iter().map(|b| b.height).fold(100., f64::max) + (caption_height + 40.).max(100.);
        let column_step = (220.
            + plan
                .edges
                .iter()
                .map(|edge| edge_label_width(&edge.label))
                .fold(0., f64::max)
            + 40.)
            .max(400.);
        for (node, rect) in plan.nodes.iter().zip(&mut boxes) {
            rect.x = f64::from(node.column.unwrap()) * column_step;
            rect.y = f64::from(node.row.unwrap()) * row_step;
        }
    } else {
        let layers = order_layers(plan, &ranks, &indices);
        let max_height = boxes.iter().map(|b| b.height).fold(100., f64::max);
        let edge_height = plan
            .edges
            .iter()
            .map(|edge| {
                wrap(&edge.label, edge_label_width(&edge.label), 16.)
                    .lines()
                    .count()
                    .max(1) as f64
                    * 20.
            })
            .fold(20., f64::max);
        let cross_step = if plan.layout == Layout::Horizontal {
            (max_height + 100.).max(edge_height * 2. + 80.)
        } else {
            400.
        };
        let main_step = if plan.layout == Layout::Horizontal {
            400.
        } else {
            max_height + edge_height.max(80.) + 100.
        };
        let mut main_positions = vec![0.; layers.len()];
        for rank in 1..layers.len() {
            let gap = if plan.layout == Layout::Horizontal {
                let width = plan
                    .edges
                    .iter()
                    .filter(|edge| {
                        ranks[indices[edge.from.as_str()]] == rank - 1
                            && ranks[indices[edge.to.as_str()]] == rank
                    })
                    .map(|edge| edge_label_width(&edge.label))
                    .fold(0., f64::max);
                main_step.max(240. + width / 0.65)
            } else {
                main_step
            };
            main_positions[rank] = main_positions[rank - 1] + gap;
        }
        let largest = layers.iter().map(Vec::len).max().unwrap_or(1);
        for (rank, layer) in layers.iter().enumerate() {
            let offset = (largest - layer.len()) as f64 * cross_step / 2.;
            for (slot, &node) in layer.iter().enumerate() {
                let cross = offset + slot as f64 * cross_step;
                if plan.layout == Layout::Horizontal {
                    boxes[node].x = main_positions[rank];
                    boxes[node].y = cross + (max_height - boxes[node].height) / 2.;
                } else {
                    boxes[node].x = cross;
                    boxes[node].y = main_positions[rank] + (max_height - boxes[node].height) / 2.;
                }
            }
        }
    }
    let top = boxes.iter().map(|b| b.y).fold(f64::INFINITY, f64::min);
    let left = boxes.iter().map(|b| b.x).fold(f64::INFINITY, f64::min);
    let mut feedback_edges = 0;
    let mut lane = 0;
    let mut routed = BTreeSet::new();
    let routes = plan
        .edges
        .iter()
        .map(|edge| {
            let a = indices[edge.from.as_str()];
            let b = indices[edge.to.as_str()];
            let (source, target) = (&boxes[a], &boxes[b]);
            let (cx1, cy1) = source.center();
            let (cx2, cy2) = target.center();
            if plan.layout == Layout::Grid {
                let (sx, sy, ex, ey) = if (source.x - target.x).abs() >= (source.y - target.y).abs()
                {
                    if target.x > source.x {
                        (source.x + source.width + 10., cy1, target.x - 10., cy2)
                    } else {
                        (source.x - 10., cy1, target.x + target.width + 10., cy2)
                    }
                } else if target.y > source.y {
                    (cx1, source.y + source.height + 10., cx2, target.y - 10.)
                } else {
                    (cx1, source.y - 10., cx2, target.y + target.height + 10.)
                };
                return vec![[sx, sy], [ex, ey]];
            }
            if ranks[b] <= ranks[a] {
                feedback_edges += 1;
            }
            if plan.layout == Layout::Horizontal {
                let start = [source.x + source.width + 10., cy1];
                let end = [target.x - 10., cy2];
                if ranks[b] == ranks[a] + 1 && routed.insert((a, b)) {
                    return vec![start, end];
                }
                let label_height = wrap(&edge.label, edge_label_width(&edge.label), 16.)
                    .lines()
                    .count()
                    .max(1) as f64
                    * 20.;
                let outside = top - 80. - lane as f64 * 200. - label_height / 2.;
                lane += 1;
                let (exit, entry) = (start[0] + 50., end[0] - 50.);
                vec![
                    start,
                    [exit, start[1]],
                    [exit, outside],
                    [entry, outside],
                    [entry, end[1]],
                    end,
                ]
            } else {
                let start = [cx1, source.y + source.height + 10.];
                let end = [cx2, target.y - 10.];
                if ranks[b] == ranks[a] + 1 && routed.insert((a, b)) {
                    return vec![start, end];
                }
                let outside = left - 150. - lane as f64 * 220.;
                lane += 1;
                let (exit, entry) = (start[1] + 50., end[1] - 50.);
                vec![
                    start,
                    [start[0], exit],
                    [outside, exit],
                    [outside, entry],
                    [end[0], entry],
                    end,
                ]
            }
        })
        .collect();
    Ok(Drawing {
        boxes,
        routes,
        feedback_edges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn font_metrics_keep_fitting_words_together_and_apply_kerning() {
        assert_eq!(wrap("Treatment input", 200., 20.), "Treatment input");
        assert_eq!(
            wrap("IL-6 receptor signaling", 140., 20.),
            "IL-6 receptor\nsignaling"
        );
        assert!(text_width("AV", 20.) < text_width("A", 20.) + text_width("V", 20.));
        assert_eq!(wrap("α β", 100., 20.), "α β");
        assert_eq!(text_width("细胞核", 20.), 60.);
        assert_eq!(text_width("（受体）", 20.), 80.);
        assert_eq!(wrap("细胞信号传导", 60., 20.), "细胞信\n号传导");
        assert_eq!(wrap("核内（受体）响应", 80., 20.), "核内（受\n体）响应");
        assert_eq!(
            wrap("Dose 10\u{a0}μM response", 130., 20.),
            "Dose 10\u{a0}μM\nresponse"
        );
    }
    #[test]
    fn fitting_hyphenated_identifiers_move_whole_to_the_next_line() {
        for token in ["p-IFNGR1", "IFN-γ", "p-STAT1", "IL-6", "procaspase-9"] {
            let width = text_width(token, 20.) + 2.;
            let source = format!("前缀 {token} 后续");
            let wrapped = wrap(&source, width, 20.);
            assert!(wrapped.contains(token), "split {token}: {wrapped}");
            assert!(wrapped.lines().all(|line| text_width(line, 20.) <= width));
            assert_eq!(wrapped.replace(['\n', ' '], ""), source.replace(' ', ""));
        }
        assert_eq!(
            wrap("IFN-\nγ", 200., 20.),
            "IFN-\nγ",
            "explicit line break retained"
        );
        let source = "p-IFNGR1-p-STAT1-superlong";
        let wrapped = wrap(source, 70., 20.);
        assert_eq!(wrapped.replace('\n', ""), source);
        assert!(wrapped.lines().all(|line| text_width(line, 20.) <= 70.));
    }
    #[test]
    fn scientific_parenthetical_keeps_fitting_identifiers_intact() {
        let label = "结合 APAF1（ADP→ATP 交换，组装七聚体凋亡小体）";
        let wrapped = wrap(label, 125.0703125, 16.);
        for token in ["APAF1", "ADP", "ATP"] {
            assert!(wrapped.contains(token), "{token} was split: {wrapped}");
        }
        assert_eq!(wrapped.replace(['\n', ' '], ""), label.replace(' ', ""));
        assert!(
            wrapped
                .lines()
                .all(|line| text_width(line, 16.) <= 125.0703125)
        );
        assert_eq!(wrap("APAF1（ATP）", 300., 16.), "APAF1（ATP）");
    }
    #[test]
    fn overlong_complex_labels_break_between_identifiers_without_losing_content() {
        let label = "IFNGR1:JAK1:IFNGR2:JAK2";
        assert!(text_width(label, 20.) > 200.);
        let wrapped = wrap(label, 200., 20.);
        for id in ["IFNGR1", "JAK1", "IFNGR2", "JAK2"] {
            assert!(wrapped.contains(id), "{id}: {wrapped}");
        }
        assert_eq!(wrapped.replace('\n', ""), label);
        assert!(wrapped.lines().all(|line| text_width(line, 20.) <= 200.));
        let relation = "IFNGR1:IFNGR2–JAK1/JAK2";
        let wrapped = wrap(relation, 110., 16.);
        for id in ["IFNGR1", "IFNGR2", "JAK1", "JAK2"] {
            assert!(wrapped.contains(id), "{id}: {wrapped}");
        }
        assert_eq!(wrapped.replace('\n', ""), relation);
        assert!(wrapped.lines().all(|line| text_width(line, 16.) <= 110.));
        assert!(text_width("IFNGR1:JAK1", 20.) <= 150.);
        assert!(text_width("Binding IFNGR1:JAK1", 20.) > 150.);
        assert_eq!(
            wrap("Binding IFNGR1:JAK1", 150., 20.),
            "Binding\nIFNGR1:JAK1"
        );
        assert_eq!(
            wrap("https://example.org", 300., 20.),
            "https://example.org"
        );
    }

    fn plan(layout: &str) -> Plan {
        serde_json::from_value(json!({"message":"Explicit conceptual relations","audience":"researchers","layout":layout,
            "nodes":[{"id":"a","label":"Ligand"},{"id":"b","label":"Receptor complex and its intracellular signaling components","shape":"ellipse"},{"id":"c","label":"Parallel response"},{"id":"d","label":"Outcome"},{"id":"e","label":"Independent control"}],
            "edges":[{"from":"a","to":"b","label":"binds"},{"from":"a","to":"c","label":"activates"},{"from":"b","to":"d","label":"stimulates"},{"from":"c","to":"d","label":"contributes"},{"from":"a","to":"d","label":"explicit cross-layer relation"},{"from":"d","to":"a","label":"negative feedback","inhibitory":true}]})).unwrap()
    }
    fn intersects(a: [f64; 2], b: [f64; 2], rect: &Box) -> bool {
        let mut lo: f64 = 0.;
        let mut hi: f64 = 1.;
        for (axis, min, max) in [
            (0, rect.x + 0.01, rect.x + rect.width - 0.01),
            (1, rect.y + 0.01, rect.y + rect.height - 0.01),
        ] {
            let delta = b[axis] - a[axis];
            if delta.abs() < 1e-9 {
                if a[axis] < min || a[axis] > max {
                    return false;
                }
            } else {
                let t1 = (min - a[axis]) / delta;
                let t2 = (max - a[axis]) / delta;
                lo = lo.max(t1.min(t2));
                hi = hi.min(t1.max(t2));
            }
        }
        lo < hi
    }
    #[test]
    fn branches_cycles_and_skipped_layers_have_clear_node_geometry() {
        for layout in ["horizontal", "vertical"] {
            let plan = plan(layout);
            let indices: HashMap<_, _> = plan
                .nodes
                .iter()
                .enumerate()
                .map(|(i, n)| (n.id.as_str(), i))
                .collect();
            let drawing = arrange(&plan).unwrap();
            assert_eq!(drawing.feedback_edges, 1);
            assert_eq!(drawing.routes.len(), plan.edges.len());
            let a = &drawing.boxes[0];
            let b = &drawing.boxes[1];
            let c = &drawing.boxes[2];
            let d = &drawing.boxes[3];
            if layout == "horizontal" {
                assert!(a.x < b.x && b.x == c.x && c.x < d.x);
            } else {
                assert!(a.y < b.y && b.y + b.height / 2. == c.y + c.height / 2. && c.y < d.y);
            }
            for (i, rect) in drawing.boxes.iter().enumerate() {
                for other in drawing.boxes.iter().skip(i + 1) {
                    assert!(
                        rect.x + rect.width <= other.x
                            || other.x + other.width <= rect.x
                            || rect.y + rect.height <= other.y
                            || other.y + other.height <= rect.y,
                        "node overlap in {layout}"
                    );
                }
                assert!(
                    rect.label_height
                        <= if matches!(plan.nodes[i].shape, Shape::Ellipse) {
                            rect.height / std::f64::consts::SQRT_2 - 10.
                        } else {
                            rect.height - 10.
                        }
                );
            }
            for (edge, route) in plan.edges.iter().zip(&drawing.routes) {
                for (node, rect) in drawing.boxes.iter().enumerate() {
                    if node == indices[edge.from.as_str()] || node == indices[edge.to.as_str()] {
                        continue;
                    }
                    for segment in route.windows(2) {
                        assert!(
                            !intersects(segment[0], segment[1], rect),
                            "{layout}: {}→{} crosses {}",
                            edge.from,
                            edge.to,
                            plan.nodes[node].id
                        );
                    }
                }
            }
            assert!(drawing.routes[4].len() > 2);
            assert!(drawing.routes[5].len() > 2);
            assert_eq!(
                crate::sketch::compile(&plan).unwrap(),
                crate::sketch::compile(&plan).unwrap(),
                "same plan gives reproducible editable output"
            );
        }
    }
    #[test]
    fn validation_respects_manual_positions_and_parallel_relations() {
        let mut plan = plan("horizontal");
        plan.nodes[0].column = Some(0);
        assert!(arrange(&plan).is_err());
        plan.nodes[0].column = None;
        plan.edges[0].to = "missing".into();
        assert!(arrange(&plan).is_err());
        let mut plan = self::plan("vertical");
        plan.edges.push(plan.edges[0].clone());
        let drawing = arrange(&plan).unwrap();
        assert_ne!(drawing.routes[0], *drawing.routes.last().unwrap());
        plan.layout = Layout::Grid;
        assert!(arrange(&plan).is_err());
    }
    #[test]
    fn long_multilingual_labels_expand_without_losing_original_text() {
        let plan: Plan = serde_json::from_value(json!({"message":"Labels","audience":"Research","layout":"vertical","nodes":[{"id":"a","label":"细胞膜受体复合物与下游转录调控因子之间的相互作用以及细胞内信号传导机制","shape":"ellipse"},{"id":"b","label":"Response"}],"edges":[{"from":"a","to":"b","label":"activates"}]})).unwrap();
        let scene = crate::sketch::compile(&plan).unwrap();
        assert!(scene["elements"][0]["height"].as_f64().unwrap() > 100.);
        assert_eq!(scene["elements"][1]["originalText"], plan.nodes[0].label);
        assert!(
            scene["elements"][1]["text"]
                .as_str()
                .unwrap()
                .contains('\n')
        );
        assert_eq!(scene["elements"][4]["startBinding"]["elementId"], "node-0");
    }
}

#[cfg(test)]
mod relation_caption_tests {
    use super::*;
    #[test]
    fn short_scientific_relations_keep_their_wording_on_one_line() {
        let label = "磷酸化 MEK 活化环";
        let natural = text_width(label, 16.);
        assert!(natural > 104. && natural <= 180.);
        let wrapped = wrap(label, edge_label_width(label), 16.);
        assert_eq!(wrapped, label);
        assert!(edge_label_width(label) <= 180.);
    }
    #[test]
    fn long_relations_retain_bounded_width_and_explicit_newlines() {
        let label = "A long source-qualified relation label that must retain its full scientific wording and wrap inside a bounded caption";
        assert!(edge_label_width(label) <= 240.);
        assert_eq!(
            wrap(label, edge_label_width(label), 16.)
                .split_whitespace()
                .collect::<Vec<_>>(),
            label.split_whitespace().collect::<Vec<_>>()
        );
        assert_eq!(
            wrap("激活\nMEK", edge_label_width("激活\nMEK"), 16.),
            "激活\nMEK"
        );
    }
}
