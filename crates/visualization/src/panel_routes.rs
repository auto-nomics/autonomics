//! Rectilinear visibility-grid routing for explicitly declared cross-panel edges.
use crate::{
    layout,
    sketch::{self, InvalidPlan, Plan},
};
use serde_json::{Value, json};
use std::{cmp::Ordering, collections::BinaryHeap};
#[derive(Clone, Copy)]
struct Rect {
    l: f64,
    t: f64,
    r: f64,
    b: f64,
}
impl Rect {
    fn element(e: &Value, margin: f64) -> Self {
        let x = e["x"].as_f64().unwrap();
        let y = e["y"].as_f64().unwrap();
        let width = e["width"].as_f64().unwrap();
        let height = e["height"].as_f64().unwrap();
        let angle = e["angle"].as_f64().unwrap_or(0.);
        let extent_x = (width * angle.cos().abs() + height * angle.sin().abs()) / 2.;
        let extent_y = (width * angle.sin().abs() + height * angle.cos().abs()) / 2.;
        Self {
            l: x + width / 2. - extent_x - margin,
            t: y + height / 2. - extent_y - margin,
            r: x + width / 2. + extent_x + margin,
            b: y + height / 2. + extent_y + margin,
        }
    }
    fn ports(self) -> [[f64; 2]; 4] {
        let x = (self.l + self.r) / 2.;
        let y = (self.t + self.b) / 2.;
        [[self.l, y], [self.r, y], [x, self.t], [x, self.b]]
    }
    fn overlaps(self, other: Self) -> bool {
        self.l < other.r && self.r > other.l && self.t < other.b && self.b > other.t
    }
}
#[derive(Clone, Copy)]
struct Entry {
    score: f64,
    state: usize,
}
impl PartialEq for Entry {
    fn eq(&self, o: &Self) -> bool {
        self.score == o.score && self.state == o.state
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Entry {
    fn cmp(&self, o: &Self) -> Ordering {
        o.score
            .total_cmp(&self.score)
            .then_with(|| o.state.cmp(&self.state))
    }
}
// Side order matches Rect::ports: left, right, top, bottom.
fn port_outward(side: usize, port: [f64; 2], outside: [f64; 2]) -> bool {
    match side {
        0 => outside[0] < port[0] && outside[1] == port[1],
        1 => outside[0] > port[0] && outside[1] == port[1],
        2 => outside[1] < port[1] && outside[0] == port[0],
        _ => outside[1] > port[1] && outside[0] == port[0],
    }
}
struct Grid {
    xs: Vec<f64>,
    ys: Vec<f64>,
    horizontal: Vec<bool>,
    vertical: Vec<bool>,
}
fn at(values: &[f64], v: f64) -> usize {
    values.binary_search_by(|x| x.total_cmp(&v)).unwrap()
}
impl Grid {
    fn new(rects: &[Rect], lanes: &[f64], left: f64, right: f64) -> Self {
        let mut xs = vec![left, right];
        let mut ys = lanes.to_vec();
        for r in rects {
            xs.extend([r.l, (r.l + r.r) / 2., r.r]);
            ys.extend([r.t, (r.t + r.b) / 2., r.b]);
        }
        xs.sort_by(f64::total_cmp);
        xs.dedup();
        ys.sort_by(f64::total_cmp);
        ys.dedup();
        let n = xs.len() * ys.len();
        let mut horizontal = vec![false; n];
        let mut vertical = vec![false; n];
        for r in rects {
            let (l, t, rr, b) = (at(&xs, r.l), at(&ys, r.t), at(&xs, r.r), at(&ys, r.b));
            for y in t + 1..b {
                for x in l..rr {
                    horizontal[y * xs.len() + x] = true;
                }
            }
            for y in t..b {
                for x in l + 1..rr {
                    vertical[y * xs.len() + x] = true;
                }
            }
        }
        Self {
            xs,
            ys,
            horizontal,
            vertical,
        }
    }
    fn index(&self, p: [f64; 2]) -> usize {
        at(&self.ys, p[1]) * self.xs.len() + at(&self.xs, p[0])
    }
    fn point(&self, i: usize) -> [f64; 2] {
        [self.xs[i % self.xs.len()], self.ys[i / self.xs.len()]]
    }
    fn route(
        &self,
        starts: &[[f64; 2]],
        ends: &[[f64; 2]],
        source_ports: Option<[bool; 4]>,
        target_ports: Option<[bool; 4]>,
    ) -> Result<Vec<[f64; 2]>, InvalidPlan> {
        let count = self.xs.len() * self.ys.len();
        let mut dist = vec![f64::INFINITY; count * 2];
        let mut prev = vec![usize::MAX; count * 2];
        let targets = ends.iter().map(|p| self.index(*p)).collect::<Vec<_>>();
        let heuristic = |i| {
            let p = self.point(i);
            ends.iter()
                .map(|q| (p[0] - q[0]).abs() + (p[1] - q[1]).abs())
                .fold(f64::INFINITY, f64::min)
        };
        let mut queue = BinaryHeap::new();
        for (side, p) in starts.iter().enumerate() {
            for dir in 0..2 {
                if source_ports.is_some_and(|ports| !ports[side] || dir != usize::from(side >= 2)) {
                    continue;
                }
                let s = self.index(*p) * 2 + dir;
                dist[s] = 0.;
                queue.push(Entry {
                    score: heuristic(s / 2),
                    state: s,
                });
            }
        }
        while let Some(Entry { score, state }) = queue.pop() {
            let i = state / 2;
            if score > dist[state] + heuristic(i) + 0.001 {
                continue;
            }
            if targets.contains(&i) && prev[state] != usize::MAX {
                let mut path = vec![self.point(i)];
                let mut s = state;
                while prev[s] != usize::MAX {
                    s = prev[s];
                    path.push(self.point(s / 2));
                }
                path.reverse();
                let mut simple: Vec<[f64; 2]> = Vec::new();
                for p in path {
                    while simple.len() > 1 {
                        let a = simple[simple.len() - 2];
                        let b = simple[simple.len() - 1];
                        if (a[0] == b[0] && b[0] == p[0]) || (a[1] == b[1] && b[1] == p[1]) {
                            simple.pop();
                        } else {
                            break;
                        }
                    }
                    simple.push(p);
                }
                return Ok(simple);
            }
            let x = i % self.xs.len();
            let y = i / self.xs.len();
            let mut neighbors = Vec::with_capacity(4);
            if x > 0 && !self.horizontal[i - 1] {
                neighbors.push((i - 1, 0));
            }
            if x + 1 < self.xs.len() && !self.horizontal[i] {
                neighbors.push((i + 1, 0));
            }
            if y > 0 && !self.vertical[i - self.xs.len()] {
                neighbors.push((i - self.xs.len(), 1));
            }
            if y + 1 < self.ys.len() && !self.vertical[i] {
                neighbors.push((i + self.xs.len(), 1));
            }
            for (next, dir) in neighbors {
                let p = self.point(i);
                let q = self.point(next);
                if source_ports.is_some() && prev[state] == usize::MAX && dist[state] == 0. {
                    let side = starts.iter().position(|start| *start == p).unwrap();
                    if !port_outward(side, p, q) {
                        continue;
                    }
                }
                if let Some(allowed) = target_ports {
                    if let Some(side) = targets.iter().position(|target| *target == next) {
                        if !allowed[side] || !port_outward(side, q, p) {
                            continue;
                        }
                    }
                }
                let cost = dist[state]
                    + (p[0] - q[0]).abs()
                    + (p[1] - q[1]).abs()
                    + if dir == state % 2 { 0. } else { 24. };
                let s = next * 2 + dir;
                if cost < dist[s] {
                    dist[s] = cost;
                    prev[s] = state;
                    queue.push(Entry {
                        score: cost + heuristic(next),
                        state: s,
                    });
                }
            }
        }
        Err(InvalidPlan(
            "no unobstructed cross-panel route; reposition the panels or nodes".into(),
        ))
    }
}
fn label_position(
    route: &[[f64; 2]],
    width: f64,
    height: f64,
    obstacles: &[Rect],
) -> Option<usize> {
    (0..route.len() - 1)
        .filter(|&i| {
            let a = route[i];
            let b = route[i + 1];
            let x = (a[0] + b[0]) / 2.;
            let y = (a[1] + b[1]) / 2.;
            let label = Rect {
                l: x - width / 2. - 6.,
                r: x + width / 2. + 6.,
                t: y - height / 2. - 6.,
                b: y + height / 2. + 6.,
            };
            (a[0] - b[0]).abs() + (a[1] - b[1]).abs() > 24.
                && !obstacles.iter().any(|r| label.overlaps(*r))
        })
        .max_by(|&a, &b| {
            let length = |i: usize| {
                (route[i][0] - route[i + 1][0]).abs() + (route[i][1] - route[i + 1][1]).abs()
            };
            length(a).total_cmp(&length(b))
        })
}
// Excalidraw anchors arrow labels at the middle vertex/segment. Add collinear
// vertices on the shorter side so the chosen clear label position is native,
// rather than relying on metadata the editor would ignore during restore.
fn center_label(route: &mut Vec<[f64; 2]>, segment: usize) -> [f64; 2] {
    let a = route[segment];
    let b = route[segment + 1];
    let center = [(a[0] + b[0]) / 2., (a[1] + b[1]) / 2.];
    if route.len() == 2 {
        return center;
    }
    route.insert(segment + 1, center);
    let left = segment + 1;
    let right = route.len() - left - 1;
    let extra = left.abs_diff(right);
    if extra > 0 {
        let index = if left < right { 0 } else { route.len() - 2 };
        let a = route[index];
        let b = route[index + 1];
        for n in 1..=extra {
            let t = n as f64 / (extra + 1) as f64;
            route.insert(
                index + n,
                [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t],
            );
        }
    }
    center
}
fn preferred_ports(elements: &[Value], node: &Value, opposite: &str, excluded: &str) -> [bool; 4] {
    let mut available = [true; 4];
    for arrow in elements.iter().filter(|e| {
        e["type"] == "arrow"
            && e["isDeleted"] != true
            && e["id"] != excluded
            && e[opposite]["elementId"] == node["id"]
            && e["angle"].as_f64() == Some(0.)
    }) {
        let Some(points) = arrow["points"].as_array().filter(|p| p.len() >= 2) else {
            continue;
        };
        let point = if opposite == "endBinding" {
            points.last().unwrap()
        } else {
            &points[0]
        };
        let (Some(x), Some(y), Some(px), Some(py), Some(gap)) = (
            arrow["x"].as_f64(),
            arrow["y"].as_f64(),
            point[0].as_f64(),
            point[1].as_f64(),
            arrow[opposite]["gap"].as_f64(),
        ) else {
            continue;
        };
        if ![x, y, px, py, gap].iter().all(|v| v.is_finite()) || !(0. ..=100.).contains(&gap) {
            continue;
        }
        for (side, p) in Rect::element(node, gap).ports().iter().enumerate() {
            if (p[0] - x - px).abs() < 1e-6 && (p[1] - y - py).abs() < 1e-6 {
                available[side] = false;
            }
        }
    }
    if available.iter().any(|v| *v) {
        available
    } else {
        [true; 4]
    }
}
fn route_between(
    source: Rect,
    target: Rect,
    grid: &Grid,
    obstacles: &[Rect],
    lane: f64,
    label_size: Option<[f64; 2]>,
    ports: ([bool; 4], [bool; 4]),
) -> Result<(Vec<[f64; 2]>, [f64; 2]), InvalidPlan> {
    let result = route_between_ports(source, target, grid, obstacles, lane, label_size, ports);
    if result.is_ok() || ports == ([true; 4], [true; 4]) {
        result
    } else {
        route_between_ports(
            source,
            target,
            grid,
            obstacles,
            lane,
            label_size,
            ([true; 4], [true; 4]),
        )
    }
}
fn route_between_ports(
    source: Rect,
    target: Rect,
    grid: &Grid,
    obstacles: &[Rect],
    lane: f64,
    label_size: Option<[f64; 2]>,
    ports: ([bool; 4], [bool; 4]),
) -> Result<(Vec<[f64; 2]>, [f64; 2]), InvalidPlan> {
    let mut route = grid.route(
        &source.ports(),
        &target.ports(),
        Some(ports.0),
        Some(ports.1),
    )?;
    let segment = if let Some([width, height]) = label_size {
        if let Some(index) = label_position(&route, width, height, obstacles) {
            index
        } else {
            let left = grid.xs[0];
            let right = *grid.xs.last().unwrap();
            let mut first = grid.route(&source.ports(), &[[left, lane]], Some(ports.0), None)?;
            let second = grid.route(&[[right, lane]], &target.ports(), None, Some(ports.1))?;
            let index = first.len() - 1;
            first.extend(second);
            route = first;
            index
        }
    } else {
        0
    };
    let center = center_label(&mut route, segment);
    Ok((route, center))
}
fn obstacle(element: &Value) -> bool {
    element["isDeleted"] != true
        && matches!(
            element["type"].as_str(),
            Some("rectangle" | "ellipse" | "diamond" | "image" | "text")
        )
}
fn valid_box(element: &Value) -> bool {
    ["x", "y", "width", "height"].iter().all(|field| {
        element[*field]
            .as_f64()
            .is_some_and(|v| v.is_finite() && v.abs() <= 1e6)
    }) && element["width"].as_f64().unwrap_or(0.) > 0.
        && element["height"].as_f64().unwrap_or(0.) > 0.
}
// The explicit fitted anchor is the reviewed visual obstacle for its own
// endpoint. Its transparent image viewport must not seal all four ports.
fn fitted_endpoint_visual(image: &Value, nodes: [&Value; 2]) -> bool {
    if image["type"] != "image"
        || !valid_box(image)
        || image["angle"].as_f64() != Some(0.)
        || !image["crop"].is_null()
        || !image["scale"]
            .as_array()
            .is_some_and(|s| s.len() == 2 && s.iter().all(|v| v.as_f64() == Some(1.)))
    {
        return false;
    }
    nodes.iter().any(|node| {
        let fit = &node["customData"]["visualAnchorFit"];
        if node["customData"]["visualAssetElementId"] != image["id"]
            || image["customData"]["semanticAnchorId"] != node["id"]
            || fit["fileId"] != image["fileId"]
        {
            return false;
        }
        let Some(bounds) = fit["normalizedBounds"].as_array().filter(|b| b.len() == 4) else {
            return false;
        };
        let Some(b) = bounds.iter().map(Value::as_f64).collect::<Option<Vec<_>>>() else {
            return false;
        };
        if !b.iter().all(|v| v.is_finite())
            || b[0] < 0.
            || b[1] < 0.
            || b[2] <= 0.
            || b[3] <= 0.
            || b[0] + b[2] > 1. + 1e-9
            || b[1] + b[3] > 1. + 1e-9
        {
            return false;
        }
        let expected = [
            image["x"].as_f64().unwrap() + b[0] * image["width"].as_f64().unwrap(),
            image["y"].as_f64().unwrap() + b[1] * image["height"].as_f64().unwrap(),
            b[2] * image["width"].as_f64().unwrap(),
            b[3] * image["height"].as_f64().unwrap(),
        ];
        ["x", "y", "width", "height"]
            .iter()
            .zip(expected)
            .all(|(key, value)| {
                node[*key]
                    .as_f64()
                    .is_some_and(|n| (n - value).abs() < 1e-6)
            })
    })
}
/// Explicit geometry edit: reads current bindings and current label bounds,
/// never regenerates the figure from its potentially stale scientific plan.
pub(crate) fn reroute(elements: &mut [Value], id: &str) -> Result<(), InvalidPlan> {
    let fail = |message: &str| InvalidPlan(message.into());
    let index = elements
        .iter()
        .position(|e| e["id"] == id)
        .ok_or_else(|| fail("connector does not exist"))?;
    let arrow = &elements[index];
    if arrow["type"] != "arrow"
        || arrow["customData"]["crossPanel"] != true
        || arrow["isDeleted"] == true
        || arrow["angle"].as_f64().unwrap_or(0.) != 0.
        || arrow["elbowed"] == true
        || !arrow["frameId"].is_null()
    {
        return Err(fail(
            "requires an unrotated, scene-level generated cross-panel arrow",
        ));
    }
    let node = |binding: &str| -> Result<&Value, InvalidPlan> {
        let id = arrow[binding]["elementId"]
            .as_str()
            .ok_or_else(|| fail("connector has a detached endpoint"))?;
        let node = elements
            .iter()
            .find(|e| e["id"] == id && e["isDeleted"] != true)
            .ok_or_else(|| fail("bound component does not exist"))?;
        if !matches!(node["type"].as_str(), Some("rectangle" | "ellipse"))
            || !valid_box(node)
            || node["angle"].as_f64().unwrap_or(0.) != 0.
        {
            return Err(fail(
                "bound components must be unrotated rectangles or ellipses",
            ));
        }
        Ok(node)
    };
    let source_node = node("startBinding")?;
    let target_node = node("endBinding")?;
    let source = Rect::element(source_node, 12.);
    let target = Rect::element(target_node, 12.);
    let labels = elements
        .iter()
        .enumerate()
        .filter(|(_, e)| e["type"] == "text" && e["containerId"] == id && e["isDeleted"] != true)
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    if labels.len() > 1 {
        return Err(fail(
            "connector has multiple labels; resolve their ownership before rerouting",
        ));
    }
    let label_index = labels.first().copied();
    let label_size = if let Some(i) = label_index {
        if !valid_box(&elements[i]) || elements[i]["angle"].as_f64().unwrap_or(0.) != 0. {
            return Err(fail("label must have valid unrotated geometry"));
        }
        Some([
            elements[i]["width"].as_f64().unwrap(),
            elements[i]["height"].as_f64().unwrap(),
        ])
    } else {
        None
    };
    let mut obstacles = Vec::new();
    for (_, e) in elements.iter().enumerate().filter(|(i, e)| {
        Some(*i) != label_index
            && obstacle(e)
            && !fitted_endpoint_visual(e, [source_node, target_node])
    }) {
        if !valid_box(e) || !e["angle"].as_f64().unwrap_or(0.).is_finite() {
            return Err(fail("obstacles must have valid finite geometry"));
        }
        obstacles.push(Rect::element(e, 12.));
    }
    let left = obstacles.iter().map(|r| r.l).fold(f64::INFINITY, f64::min) - 200.;
    let right = obstacles
        .iter()
        .map(|r| r.r)
        .fold(f64::NEG_INFINITY, f64::max)
        + 200.;
    let top = obstacles.iter().map(|r| r.t).fold(f64::INFINITY, f64::min);
    let lane = top - label_size.map_or(160., |s| s[1] / 2. + 100.);
    let grid = Grid::new(&obstacles, &[lane], left, right);
    let (route, center) = route_between(
        source,
        target,
        &grid,
        &obstacles,
        lane,
        label_size,
        (
            preferred_ports(elements, source_node, "endBinding", id),
            preferred_ports(elements, target_node, "startBinding", id),
        ),
    )?;
    let start = route[0];
    let min_x = route.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
    let max_x = route.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);
    let min_y = route.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
    let max_y = route.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max);
    let arrow = &mut elements[index];
    arrow["x"] = json!(start[0]);
    arrow["y"] = json!(start[1]);
    arrow["width"] = json!(max_x - min_x);
    arrow["height"] = json!(max_y - min_y);
    arrow["points"] = json!(
        route
            .iter()
            .map(|p| [p[0] - start[0], p[1] - start[1]])
            .collect::<Vec<_>>()
    );
    for key in ["startBinding", "endBinding"] {
        arrow[key]["gap"] = json!(12);
        arrow[key]["focus"] = json!(0);
        if let Some(binding) = arrow[key].as_object_mut() {
            binding.remove("fixedPoint");
        }
    }
    crate::scene::touch(arrow);
    if let Some(i) = label_index {
        let label = &mut elements[i];
        let [width, height] = label_size.unwrap();
        label["x"] = json!(center[0] - width / 2.);
        label["y"] = json!(center[1] - height / 2.);
        crate::scene::touch(label);
    }
    Ok(())
}
pub(crate) fn connect(plan: &Plan, elements: &mut Vec<Value>) -> Result<(), InvalidPlan> {
    let cross = plan
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let a = plan.nodes.iter().find(|n| n.id == e.from).unwrap();
            let b = plan.nodes.iter().find(|n| n.id == e.to).unwrap();
            a.panel != b.panel
        })
        .collect::<Vec<_>>();
    if cross.is_empty() {
        return Ok(());
    }
    let mut obstacles = elements
        .iter()
        .filter(|e| {
            e["type"] == "rectangle"
                || e["type"] == "ellipse"
                || e["type"] == "text"
                || e["id"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("panel-title-"))
        })
        .map(|e| Rect::element(e, 12.))
        .collect::<Vec<_>>();
    let left = obstacles.iter().map(|r| r.l).fold(f64::INFINITY, f64::min) - 200.;
    let right = obstacles
        .iter()
        .map(|r| r.r)
        .fold(f64::NEG_INFINITY, f64::max)
        + 200.;
    let top = obstacles.iter().map(|r| r.t).fold(f64::INFINITY, f64::min);
    let mut next_lane = top - 160.;
    let lanes = cross
        .iter()
        .map(|(_, e)| {
            let y = next_lane;
            next_lane -=
                layout::wrap(&e.label, 140., 16.).lines().count().max(1) as f64 * 20. + 100.;
            y
        })
        .collect::<Vec<_>>();
    let grid = Grid::new(&obstacles, &lanes, left, right);
    for ((index, edge), lane) in cross.into_iter().zip(lanes) {
        if edge.label.chars().count() > 120 {
            return Err(InvalidPlan(
                "edge labels support at most 120 characters".into(),
            ));
        }
        let a = plan.nodes.iter().position(|n| n.id == edge.from).unwrap();
        let b = plan.nodes.iter().position(|n| n.id == edge.to).unwrap();
        let source = Rect::element(
            elements
                .iter()
                .find(|e| e["id"] == format!("node-{a}"))
                .unwrap(),
            12.,
        );
        let target = Rect::element(
            elements
                .iter()
                .find(|e| e["id"] == format!("node-{b}"))
                .unwrap(),
            12.,
        );
        let wrapped = layout::wrap(&edge.label, 140., 16.);
        let width = layout::text_width(&wrapped, 16.).clamp(1., 140.);
        let height = wrapped.lines().count().max(1) as f64 * 20.;
        let (route, label_center) = route_between(
            source,
            target,
            &grid,
            &obstacles,
            lane,
            if edge.label.is_empty() {
                None
            } else {
                Some([width, height])
            },
            (
                preferred_ports(
                    elements,
                    elements
                        .iter()
                        .find(|e| e["id"] == format!("node-{a}"))
                        .unwrap(),
                    "endBinding",
                    &format!("edge-{index}"),
                ),
                preferred_ports(
                    elements,
                    elements
                        .iter()
                        .find(|e| e["id"] == format!("node-{b}"))
                        .unwrap(),
                    "startBinding",
                    &format!("edge-{index}"),
                ),
            ),
        )?;
        let start = route[0];
        let bb = route.iter().fold(
            [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ],
            |mut b, p| {
                b[0] = b[0].min(p[0]);
                b[1] = b[1].min(p[1]);
                b[2] = b[2].max(p[0]);
                b[3] = b[3].max(p[1]);
                b
            },
        );
        let id = format!("edge-{index}");
        let label_id = format!("edge-label-{index}");
        let mut arrow = sketch::base(
            &id,
            "arrow",
            start[0],
            start[1],
            bb[2] - bb[0],
            bb[3] - bb[1],
        );
        arrow.as_object_mut().unwrap().extend(json!({"points":route.iter().map(|p|[p[0]-start[0],p[1]-start[1]]).collect::<Vec<_>>(),"startBinding":{"elementId":format!("node-{a}"),"focus":0,"gap":12},"endBinding":{"elementId":format!("node-{b}"),"focus":0,"gap":12},"startArrowhead":edge.arrowheads().0,"endArrowhead":edge.arrowheads().1,"elbowed":false,"customData":{"from":edge.from,"to":edge.to,"evidence":edge.evidence,"direction":edge.direction,"inhibitory":edge.inhibitory,"crossPanel":true}}).as_object().unwrap().clone());
        if !edge.label.is_empty() {
            arrow["boundElements"] = json!([{"id":label_id,"type":"text"}]);
        }
        for node in [format!("node-{a}"), format!("node-{b}")] {
            elements.iter_mut().find(|e| e["id"] == node).unwrap()["boundElements"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":id,"type":"arrow"}));
        }
        elements.push(arrow);
        if !edge.label.is_empty() {
            let [x, y] = label_center;
            let mut label =
                sketch::text(&label_id, &wrapped, x - width / 2., y - height / 2., width);
            label["fontSize"] = json!(16);
            label["height"] = json!(height);
            label["originalText"] = json!(edge.label);
            label["containerId"] = json!(id);
            obstacles.push(Rect::element(&label, 6.));
            elements.push(label);
        }
    }
    Ok(())
}
