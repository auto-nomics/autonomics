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
    font_size
        * if c == ' ' {
            0.35
        } else if "MW@mw".contains(c) {
            1.1
        } else if c.is_ascii_uppercase() {
            0.9
        } else if c.is_ascii() {
            0.65
        } else {
            1.1
        }
}
pub(crate) fn text_width(label: &str, font_size: f64) -> f64 {
    label
        .lines()
        .map(|line| line.chars().map(|c| glyph_width(c, font_size)).sum::<f64>())
        .fold(0., f64::max)
}
/// Conservative Arial/fallback widths, with word boundaries and long-token wrapping.
pub(crate) fn wrap(label: &str, width: f64, font_size: f64) -> String {
    let advance = |c| glyph_width(c, font_size);
    let mut lines = Vec::new();
    for paragraph in label.split('\n') {
        let mut line = String::new();
        let mut used = 0.;
        for word in paragraph.split_whitespace() {
            let word_width = word.chars().map(advance).sum::<f64>();
            if !line.is_empty() && used + advance(' ') + word_width > width {
                lines.push(std::mem::take(&mut line));
                used = 0.;
            }
            if !line.is_empty() {
                line.push(' ');
                used += advance(' ');
            }
            for c in word.chars() {
                if used > 0. && used + advance(c) > width {
                    lines.push(std::mem::take(&mut line));
                    used = 0.;
                }
                line.push(c);
                used += advance(c);
            }
        }
        lines.push(line);
    }
    lines.join("\n")
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
            let label = if plan.layout == Layout::Grid {
                node.label.clone()
            } else {
                wrap(&node.label, label_width, 20.)
            };
            let label_height = label.lines().count().max(1) as f64 * 25.;
            let height = if plan.layout == Layout::Grid {
                100.
            } else if matches!(node.shape, Shape::Ellipse) {
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
                label_width: if plan.layout == Layout::Grid {
                    200.
                } else {
                    label_width
                },
                label_height: if plan.layout == Layout::Grid {
                    50.
                } else {
                    label_height
                },
            }
        })
        .collect();
    let ranks = if plan.layout == Layout::Grid {
        vec![]
    } else {
        ranks(plan, &indices)
    };
    if plan.layout == Layout::Grid {
        for (node, rect) in plan.nodes.iter().zip(&mut boxes) {
            rect.x = f64::from(node.column.unwrap()) * 400.;
            rect.y = f64::from(node.row.unwrap()) * 200.;
        }
    } else {
        let layers = order_layers(plan, &ranks, &indices);
        let max_height = boxes.iter().map(|b| b.height).fold(100., f64::max);
        let edge_height = plan
            .edges
            .iter()
            .map(|edge| wrap(&edge.label, 140., 16.).lines().count().max(1) as f64 * 20.)
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
        let largest = layers.iter().map(Vec::len).max().unwrap_or(1);
        for (rank, layer) in layers.iter().enumerate() {
            let offset = (largest - layer.len()) as f64 * cross_step / 2.;
            for (slot, &node) in layer.iter().enumerate() {
                let cross = offset + slot as f64 * cross_step;
                if plan.layout == Layout::Horizontal {
                    boxes[node].x = rank as f64 * main_step;
                    boxes[node].y = cross + (max_height - boxes[node].height) / 2.;
                } else {
                    boxes[node].x = cross;
                    boxes[node].y =
                        rank as f64 * main_step + (max_height - boxes[node].height) / 2.;
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
                        (source.x + 230., cy1, target.x - 10., cy2)
                    } else {
                        (source.x - 10., cy1, target.x + 230., cy2)
                    }
                } else if target.y > source.y {
                    (cx1, source.y + 110., cx2, target.y - 10.)
                } else {
                    (cx1, source.y - 10., cx2, target.y + 110.)
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
                let label_height = wrap(&edge.label, 160., 16.).lines().count().max(1) as f64 * 20.;
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
