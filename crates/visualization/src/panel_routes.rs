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
    obstacles: Vec<Rect>,
    caption_borders: Vec<Rect>,
    xs: Vec<f64>,
    ys: Vec<f64>,
    horizontal: Vec<bool>,
    vertical: Vec<bool>,
}
fn at(values: &[f64], v: f64) -> usize {
    values.binary_search_by(|x| x.total_cmp(&v)).unwrap()
}
fn simplify_route(path: impl IntoIterator<Item = [f64; 2]>) -> Vec<[f64; 2]> {
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
    simple
}
impl Grid {
    fn new(rects: &[Rect], lanes: &[f64], left: f64, right: f64) -> Self {
        Self::with_points(rects, lanes, left, right, &[])
    }
    fn with_points(
        rects: &[Rect],
        lanes: &[f64],
        left: f64,
        right: f64,
        points: &[[f64; 2]],
    ) -> Self {
        let mut xs = vec![left, right];
        let mut ys = lanes.to_vec();
        xs.extend(points.iter().map(|p| p[0]));
        ys.extend(points.iter().map(|p| p[1]));
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
            obstacles: rects.to_vec(),
            caption_borders: Vec::new(),
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
        // Prefer enough straight approach for a visible arrowhead. In a genuinely
        // confined gap retain the original legal route rather than failing it.
        // Add only this edge's port stubs. Without these grid coordinates,
        // forbidding short approaches can force a detour to a distant obstacle
        // edge, then a U-turn back to the target. Retain custom caption lanes.
        let mut points: Vec<_> = self.xs.iter().map(|&x| [x, self.ys[0]]).collect();
        for (ports, allowed) in [(starts, source_ports), (ends, target_ports)] {
            if let Some(allowed) = allowed {
                for (side, port) in ports.iter().enumerate() {
                    if !allowed[side] {
                        continue;
                    }
                    let mut stub = *port;
                    stub[usize::from(side >= 2)] += if side % 2 == 0 { -26. } else { 26. };
                    if stub[0] >= self.xs[0]
                        && stub[0] <= *self.xs.last().unwrap()
                        && stub[1] >= self.ys[0]
                        && stub[1] <= *self.ys.last().unwrap()
                    {
                        points.push(stub);
                    }
                }
            }
        }
        let expanded = Self::with_points(
            &self.obstacles,
            &self.ys,
            self.xs[0],
            *self.xs.last().unwrap(),
            &points,
        );
        expanded
            .route_with_clearance(starts, ends, source_ports, target_ports, 26.)
            .or_else(|_| self.route_with_clearance(starts, ends, source_ports, target_ports, 0.))
    }
    fn route_with_clearance(
        &self,
        starts: &[[f64; 2]],
        ends: &[[f64; 2]],
        source_ports: Option<[bool; 4]>,
        target_ports: Option<[bool; 4]>,
        clearance: f64,
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
                return Ok(simplify_route(path));
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
                if dir != state % 2 && clearance > 0. {
                    let near_port = |points: &[[f64; 2]], allowed: Option<[bool; 4]>| {
                        allowed.is_some_and(|allowed| {
                            points.iter().enumerate().any(|(side, port)| {
                                allowed[side]
                                    && port_outward(side, *port, p)
                                    && (p[0] - port[0]).abs() + (p[1] - port[1]).abs() < clearance
                            })
                        })
                    };
                    if near_port(starts, source_ports) || near_port(ends, target_ports) {
                        continue;
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
fn label_clear(center: [f64; 2], width: f64, height: f64, obstacles: &[Rect]) -> bool {
    let label = Rect {
        l: center[0] - width / 2. - 6.,
        r: center[0] + width / 2. + 6.,
        t: center[1] - height / 2. - 6.,
        b: center[1] + height / 2. + 6.,
    };
    !obstacles.iter().any(|r| label.overlaps(*r))
}
// Slide along a straight segment when its midpoint would cover an obstacle.
// Candidate interval boundaries suffice because the exclusions are rectangles.
fn label_position(
    route: &[[f64; 2]],
    width: f64,
    height: f64,
    obstacles: &[Rect],
) -> Option<[f64; 2]> {
    let mut best = None;
    let mut best_length = 0.;
    for segment in route.windows(2) {
        let [a, b] = [segment[0], segment[1]];
        let axis = usize::from(a[0] == b[0]);
        let length = (b[axis] - a[axis]).abs();
        if length <= 24. || length <= best_length {
            continue;
        }
        let center = [(a[0] + b[0]) / 2., (a[1] + b[1]) / 2.];
        let mut positions = vec![center[axis]];
        let half = if axis == 0 { width / 2. } else { height / 2. };
        for rect in obstacles {
            let (low, high) = if axis == 0 {
                (rect.l, rect.r)
            } else {
                (rect.t, rect.b)
            };
            positions.extend([low - half - 6.001, high + half + 6.001]);
        }
        positions.sort_by(|x, y| {
            (x - center[axis])
                .abs()
                .total_cmp(&(y - center[axis]).abs())
        });
        for position in positions {
            if position <= a[axis].min(b[axis]) + 12. || position >= a[axis].max(b[axis]) - 12. {
                continue;
            }
            let mut candidate = center;
            candidate[axis] = position;
            if label_clear(candidate, width, height, obstacles) {
                best = Some(candidate);
                best_length = length;
                break;
            }
        }
    }
    best
}
fn frame_borders(elements: &[Value]) -> Vec<Rect> {
    let mut borders = Vec::new();
    for frame in elements
        .iter()
        .filter(|e| e["type"] == "frame" && e["isDeleted"] != true && valid_box(e))
    {
        let r = Rect::element(frame, 0.);
        borders.extend([
            Rect {
                l: r.l - 2.,
                r: r.r + 2.,
                t: r.t - 2.,
                b: r.t + 2.,
            },
            Rect {
                l: r.l - 2.,
                r: r.r + 2.,
                t: r.b - 2.,
                b: r.b + 2.,
            },
            Rect {
                l: r.l - 2.,
                r: r.l + 2.,
                t: r.t,
                b: r.b,
            },
            Rect {
                l: r.r - 2.,
                r: r.r + 2.,
                t: r.t,
                b: r.b,
            },
        ]);
    }
    borders
}
// These obstacles constrain caption placement only: other connectors remain
// traversable by the route, but their straight segments must not run through text.
fn connector_caption_obstacles(elements: &[Value], excluded: &str) -> Vec<Rect> {
    let mut barriers = Vec::new();
    for arrow in elements.iter().filter(|e| {
        e["type"] == "arrow"
            && e["isDeleted"] != true
            && e["id"] != excluded
            && e["angle"].as_f64().unwrap_or(0.) == 0.
            && e["elbowed"] != true
            && e["roundness"].is_null()
    }) {
        let (Some(x), Some(y), Some(points)) = (
            arrow["x"].as_f64(),
            arrow["y"].as_f64(),
            arrow["points"].as_array(),
        ) else {
            continue;
        };
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        for pair in points.windows(2) {
            let read = |v: &Value| -> Option<[f64; 2]> {
                let p = v.as_array().filter(|p| p.len() == 2)?;
                let p = [x + p[0].as_f64()?, y + p[1].as_f64()?];
                p.iter().all(|v| v.is_finite()).then_some(p)
            };
            let (Some(a), Some(b)) = (read(&pair[0]), read(&pair[1])) else {
                continue;
            };
            // A diagonal segment's enclosing box would falsely block empty space.
            // Curves/diagonals are outside this rectilinear placement check.
            if (a[0] != b[0] && a[1] != b[1]) || a == b {
                continue;
            }
            barriers.push(Rect {
                l: a[0].min(b[0]) - 2.,
                r: a[0].max(b[0]) + 2.,
                t: a[1].min(b[1]) - 2.,
                b: a[1].max(b[1]) + 2.,
            });
        }
    }
    barriers
}
// An edge-only visibility grid can find a short path hugging an obstacle but
// miss a nearby channel wide enough for its caption. Try at most 16 clear
// endpoint-pair midpoints before escaping to the outer figure lane.
fn caption_channel(
    source: Rect,
    target: Rect,
    grid: &Grid,
    obstacles: &[Rect],
    size: [f64; 2],
    ports: ([bool; 4], [bool; 4]),
) -> Option<(Vec<[f64; 2]>, [f64; 2])> {
    let mut label_obstacles = obstacles.to_vec();
    label_obstacles.extend_from_slice(&grid.caption_borders);
    let mut centers = Vec::new();
    for (a, p) in source.ports().iter().enumerate() {
        for (b, q) in target.ports().iter().enumerate() {
            let center = [(p[0] + q[0]) / 2., (p[1] + q[1]) / 2.];
            if ports.0[a]
                && ports.1[b]
                && label_clear(center, size[0], size[1], &label_obstacles)
                && !centers.contains(&center)
            {
                centers.push(center);
            }
        }
    }
    if centers.is_empty() {
        return None;
    }
    let expanded = Grid::with_points(
        obstacles,
        &grid.ys,
        grid.xs[0],
        *grid.xs.last().unwrap(),
        &centers,
    );
    let mut best = None;
    let mut best_cost = f64::INFINITY;
    for center in centers {
        let Ok(mut first) = expanded.route(&source.ports(), &[center], Some(ports.0), None) else {
            continue;
        };
        let Ok(second) = expanded.route(&[center], &target.ports(), None, Some(ports.1)) else {
            continue;
        };
        first.extend(second.into_iter().skip(1));
        let route = simplify_route(first);
        let Some(segment) = label_position(&route, size[0], size[1], &label_obstacles) else {
            continue;
        };
        let cost = route
            .windows(2)
            .map(|p| (p[0][0] - p[1][0]).abs() + (p[0][1] - p[1][1]).abs())
            .sum::<f64>()
            + 24. * route.len().saturating_sub(2) as f64;
        if cost < best_cost {
            best_cost = cost;
            best = Some((route, segment));
        }
    }
    best
}
// Excalidraw anchors arrow labels at the middle vertex/segment. Add collinear
// vertices on the shorter side so the chosen clear label position is native,
// rather than relying on metadata the editor would ignore during restore.
fn center_label(route: &mut Vec<[f64; 2]>, center: [f64; 2]) {
    let a = route[0];
    let b = *route.last().unwrap();
    if route.len() == 2 && center == [(a[0] + b[0]) / 2., (a[1] + b[1]) / 2.] {
        return;
    }
    crate::connector_path::preserve_center(route, center);
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
    let mut label_obstacles = obstacles.to_vec();
    label_obstacles.extend_from_slice(&grid.caption_borders);
    let center = if let Some([width, height]) = label_size {
        if let Some(index) = label_position(&route, width, height, &label_obstacles) {
            index
        } else if let Some((local, index)) =
            caption_channel(source, target, grid, obstacles, [width, height], ports)
        {
            route = local;
            index
        } else {
            let left = grid.xs[0];
            let right = *grid.xs.last().unwrap();
            let mut best = None;
            let mut best_cost = f64::INFINITY;
            let middle = (source.l + source.r + target.l + target.r) / 4.;
            let expanded = Grid::with_points(obstacles, &grid.ys, left, right, &[[middle, lane]]);
            for x in [left, right, middle] {
                let waypoint = [[x, lane]];
                let Ok(mut first) = expanded.route(&source.ports(), &waypoint, Some(ports.0), None)
                else {
                    continue;
                };
                let Ok(second) = expanded.route(&waypoint, &target.ports(), None, Some(ports.1))
                else {
                    continue;
                };
                first.extend(second.into_iter().skip(1));
                let candidate = simplify_route(first);
                let Some(center) = label_position(&candidate, width, height, &label_obstacles)
                else {
                    continue;
                };
                let cost = candidate
                    .windows(2)
                    .map(|p| (p[0][0] - p[1][0]).abs() + (p[0][1] - p[1][1]).abs())
                    .sum::<f64>();
                if cost < best_cost {
                    best_cost = cost;
                    best = Some((candidate, center));
                }
            }
            let (candidate, center) = best.ok_or_else(|| InvalidPlan("no clear caption position; reposition the panels or shorten the caption explicitly".into()))?;
            route = candidate;
            center
        }
    } else {
        [
            (route[0][0] + route[1][0]) / 2.,
            (route[0][1] + route[1][1]) / 2.,
        ]
    };
    center_label(&mut route, center);
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
    if image["type"] != "image" || !valid_box(image) {
        return false;
    }
    nodes.iter().any(|node| {
        let primary = node["customData"]["visualAssetElementId"] == image["id"]
            && image["customData"]["semanticAnchorId"] == node["id"];
        let accessory = image["customData"]["semanticBadgeAnchorId"] == node["id"]
            && node["customData"]["visualBadgeIds"]
                .as_array()
                .is_some_and(|ids| ids.contains(&image["id"]));
        if (primary || accessory)
            && image["frameId"] == node["frameId"]
            && image["angle"].as_f64() == Some(0.)
            && valid_box(node)
        {
            let i = Rect::element(image, 0.);
            let n = Rect::element(node, 0.);
            if i.l >= n.l && i.r <= n.r && i.t >= n.t && i.b <= n.b {
                return true;
            }
        }
        let root = &node["customData"]["visualAnchorFit"];
        let grouped = root["scope"] == "illustration_group";
        if !(primary || accessory) || image["frameId"] != node["frameId"] {
            return false;
        }
        let fit = if grouped {
            let Some(members) = root["members"].as_array() else {
                return false;
            };
            let mut matches = members.iter().filter(|m| m["elementId"] == image["id"]);
            let Some(member) = matches.next() else {
                return false;
            };
            if matches.next().is_some() {
                return false;
            }
            let Some(bounds) = member["imageBounds"].as_array().filter(|b| b.len() == 4) else {
                return false;
            };
            if !["x", "y", "width", "height"]
                .iter()
                .zip(bounds)
                .all(|(key, b)| {
                    image[*key]
                        .as_f64()
                        .zip(b.as_f64())
                        .is_some_and(|(a, b)| (a - b).abs() < 1e-6)
                })
                || member["imageTransform"]["angle"] != image["angle"]
                || member["imageTransform"]["scale"] != image["scale"]
                || member["imageTransform"]["crop"] != image["crop"]
            {
                return false;
            }
            member
        } else {
            if !primary {
                return false;
            }
            root
        };
        if fit["fileId"] != image["fileId"] {
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
        let Ok(expected) = crate::illustration_fit::artwork_bounds(image, [b[0], b[1], b[2], b[3]])
        else {
            return false;
        };
        if grouped {
            if !valid_box(node) {
                return false;
            }
            let n = Rect::element(node, 0.);
            return expected[0] >= n.l - 1e-6
                && expected[1] >= n.t - 1e-6
                && expected[0] + expected[2] <= n.r + 1e-6
                && expected[1] + expected[3] <= n.b + 1e-6;
        }
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
    if elements
        .iter()
        .any(|e| e["id"] == id && e["customData"]["crossPanel"] != true)
    {
        return reroute_local(elements, id);
    }

    let fail = |message: &str| InvalidPlan(message.into());
    let index = elements
        .iter()
        .position(|e| e["id"] == id)
        .ok_or_else(|| fail("connector does not exist"))?;
    let arrow = &elements[index];
    if arrow["type"] != "arrow"
        || arrow["locked"] == true
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
        if elements[i]["locked"] == true
            || !valid_box(&elements[i])
            || elements[i]["angle"].as_f64().unwrap_or(0.) != 0.
        {
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
    let mut grid = Grid::new(&obstacles, &[lane], left, right);
    grid.caption_borders = frame_borders(elements);
    grid.caption_borders
        .extend(connector_caption_obstacles(elements, id));
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
            next_lane -= layout::wrap(&e.label, layout::edge_label_width(&e.label), 16.)
                .lines()
                .count()
                .max(1) as f64
                * 20.
                + 100.;
            y
        })
        .collect::<Vec<_>>();
    let mut grid = Grid::new(&obstacles, &lanes, left, right);
    grid.caption_borders = frame_borders(elements);
    for ((index, edge), lane) in cross.into_iter().zip(lanes) {
        grid.caption_borders = frame_borders(elements);
        grid.caption_borders.extend(connector_caption_obstacles(
            elements,
            &format!("edge-{index}"),
        ));
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
        let limit = layout::edge_label_width(&edge.label);
        let wrapped = layout::wrap(&edge.label, limit, 16.);
        let width = layout::text_width(&wrapped, 16.).clamp(1., limit);
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
        arrow.as_object_mut().unwrap().extend(json!({"points":route.iter().map(|p|[p[0]-start[0],p[1]-start[1]]).collect::<Vec<_>>(),"startBinding":{"elementId":format!("node-{a}"),"focus":0,"gap":12},"endBinding":{"elementId":format!("node-{b}"),"focus":0,"gap":12},"strokeStyle":edge.line_style.native(),"startArrowhead":edge.arrowheads().0,"endArrowhead":edge.arrowheads().1,"elbowed":false,"customData":{"from":edge.from,"to":edge.to,"evidence":edge.evidence,"direction":edge.direction,"inhibitory":edge.inhibitory,"crossPanel":true}}).as_object().unwrap().clone());
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

/// Reroute a sharp local connector around current objects, keeping its endpoints
/// and caption fixed. This deliberately does not infer new ports or semantics.
fn reroute_local(elements: &mut [Value], id: &str) -> Result<(), InvalidPlan> {
    reroute_local_impl(elements, id, None)
}
pub(crate) fn rebind_local_ports(
    elements: &mut [Value],
    id: &str,
    sides: [usize; 2],
) -> Result<(), InvalidPlan> {
    reroute_local_impl(elements, id, Some(sides))
}
fn reroute_local_impl(
    elements: &mut [Value],
    id: &str,
    sides: Option<[usize; 2]>,
) -> Result<(), InvalidPlan> {
    let fail = |s: &str| InvalidPlan(format!("local reroute: {s}"));
    let index = elements
        .iter()
        .position(|e| e["id"] == id)
        .ok_or_else(|| fail("missing arrow"))?;
    let arrow = &elements[index];
    if arrow["type"] != "arrow"
        || arrow["isDeleted"] == true
        || arrow["locked"] == true
        || arrow["angle"].as_f64() != Some(0.)
        || arrow["elbowed"] == true
        || !arrow["roundness"].is_null()
        || arrow["roughness"].as_f64() != Some(0.)
    {
        return Err(fail("requires a live unlocked sharp unrotated arrow"));
    }
    let points = arrow["points"]
        .as_array()
        .filter(|p| (2..=512).contains(&p.len()))
        .ok_or_else(|| fail("invalid points"))?;
    let x = arrow["x"].as_f64().ok_or_else(|| fail("invalid origin"))?;
    let y = arrow["y"].as_f64().ok_or_else(|| fail("invalid origin"))?;
    let points = points
        .iter()
        .map(|p| Some([x + p[0].as_f64()?, y + p[1].as_f64()?]))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| fail("invalid points"))?;
    if !points
        .iter()
        .flatten()
        .all(|n| n.is_finite() && n.abs() <= 1e6)
        || (sides.is_none()
            && !points
                .windows(2)
                .all(|p| p[0][0] == p[1][0] || p[0][1] == p[1][1]))
    {
        return Err(fail("requires finite orthogonal geometry"));
    }
    let node = |field: &str| {
        elements.iter().find(|e| {
            e["id"] == arrow[field]["elementId"]
                && e["isDeleted"] != true
                && valid_box(e)
                && e["angle"].as_f64() == Some(0.)
        })
    };
    let source = node("startBinding").ok_or_else(|| fail("missing start component"))?;
    let target = node("endBinding").ok_or_else(|| fail("missing end component"))?;
    let frame_id = arrow["frameId"]
        .as_str()
        .ok_or_else(|| fail("requires a local frame"))?;
    if source["frameId"] != frame_id
        || target["frameId"] != frame_id
        || source["id"] == target["id"]
    {
        return Err(fail("endpoints must share the arrow frame"));
    }
    let frame = elements
        .iter()
        .find(|e| {
            e["id"] == frame_id
                && e["type"] == "frame"
                && e["isDeleted"] != true
                && valid_box(e)
                && e["angle"].as_f64() == Some(0.)
        })
        .ok_or_else(|| fail("invalid frame"))?;
    let bounds = Rect::element(frame, 0.);
    let labels = elements
        .iter()
        .filter(|e| e["type"] == "text" && e["containerId"] == id && e["isDeleted"] != true)
        .collect::<Vec<_>>();
    if labels.len() > 1 {
        return Err(fail("ambiguous caption"));
    }
    let caption = labels.first().copied();
    if caption
        .is_some_and(|e| e["locked"] == true || !valid_box(e) || e["angle"].as_f64() != Some(0.))
    {
        return Err(fail("caption must be unlocked and unrotated"));
    }
    let mut obstacles = Vec::new();
    for e in elements.iter().filter(|e| {
        e["frameId"] == frame_id
            && obstacle(e)
            && !caption.is_some_and(|c| c["id"] == e["id"])
            && !fitted_endpoint_visual(e, [source, target])
    }) {
        if !valid_box(e) || e["angle"].as_f64() != Some(0.) {
            return Err(fail("unsupported obstacle geometry"));
        }
        let endpoint = e["id"] == source["id"] || e["id"] == target["id"];
        let mut r = Rect::element(e, if endpoint { 0. } else { 1. });
        r.l = r.l.max(bounds.l);
        r.r = r.r.min(bounds.r);
        r.t = r.t.max(bounds.t);
        r.b = r.b.min(bounds.b);
        if r.l < r.r && r.t < r.b {
            obstacles.push(r);
        }
    }
    if obstacles.len() > 128 {
        return Err(fail("too many local obstacles"));
    }
    let (starts, ends, start_mask, end_mask) = if let Some([start_side, end_side]) = sides {
        let port_rect = |node: &Value, field: &str| -> Result<Rect, InvalidPlan> {
            let binding = &arrow[field];
            let gap = binding["gap"]
                .as_f64()
                .filter(|v| v.is_finite() && (0. ..=100.).contains(v))
                .ok_or_else(|| fail("requires a finite gap within 0..100"))?;
            if binding["focus"].as_f64() != Some(0.)
                || !binding["fixedPoint"].is_null()
                || !matches!(
                    node["type"].as_str(),
                    Some("rectangle" | "ellipse" | "diamond")
                )
            {
                return Err(fail(
                    "requires centered native shape bindings without fixedPoint",
                ));
            }
            Ok(Rect::element(node, gap))
        };
        let starts = port_rect(source, "startBinding")?.ports().to_vec();
        let ends = port_rect(target, "endBinding")?.ports().to_vec();
        let mut a = [false; 4];
        let mut b = [false; 4];
        a[start_side] = true;
        b[end_side] = true;
        (starts, ends, Some(a), Some(b))
    } else {
        (vec![points[0]], vec![*points.last().unwrap()], None, None)
    };
    let center = caption.map(|e| {
        [
            e["x"].as_f64().unwrap() + e["width"].as_f64().unwrap() / 2.,
            e["y"].as_f64().unwrap() + e["height"].as_f64().unwrap() / 2.,
        ]
    });
    let mut controls = starts.clone();
    controls.extend_from_slice(&ends);
    if let Some(c) = center {
        controls.push(c);
    }
    let grid = Grid::with_points(
        &obstacles,
        &[bounds.t, bounds.b],
        bounds.l,
        bounds.r,
        &controls,
    );
    let mut route = if let Some(c) = center {
        let mut first = grid.route(&starts, &[c], start_mask, None)?;
        let second = grid.route(&[c], &ends, None, end_mask)?;
        first.extend(second.into_iter().skip(1));
        simplify_route(first)
    } else {
        grid.route(&starts, &ends, start_mask, end_mask)?
    };
    if let Some(c) = center {
        center_label(&mut route, c);
    }
    if route.len() > 512
        || route
            .iter()
            .any(|p| p[0] < bounds.l || p[0] > bounds.r || p[1] < bounds.t || p[1] > bounds.b)
    {
        return Err(fail("route leaves the frame or exceeds point limit"));
    }
    let start = route[0];
    let min_x = route.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
    let max_x = route.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);
    let min_y = route.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
    let max_y = route.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max);
    let next = &mut elements[index];
    next["x"] = json!(start[0]);
    next["y"] = json!(start[1]);
    next["width"] = json!(max_x - min_x);
    next["height"] = json!(max_y - min_y);
    next["points"] = json!(
        route
            .iter()
            .map(|p| [p[0] - start[0], p[1] - start[1]])
            .collect::<Vec<_>>()
    );
    crate::scene::touch(next);
    Ok(())
}

#[cfg(test)]
mod terminal_clearance_tests {
    #[test]
    fn group_viewport_exclusion_requires_current_member_geometry_and_file() {
        let mut image = serde_json::json!({"id":"image","type":"image","x":0,"y":0,"width":100,"height":100,"angle":0,"scale":[1,1],"fileId":"f","customData":{"semanticBadgeAnchorId":"anchor"}});
        let anchor = serde_json::json!({"id":"anchor","x":20,"y":20,"width":60,"height":60,"angle":0,"customData":{"visualAssetElementId":"primary","visualBadgeIds":["image"],"visualAnchorFit":{"scope":"illustration_group","members":[{"elementId":"image","fileId":"f","normalizedBounds":[0.2,0.2,0.6,0.6],"imageBounds":[0,0,100,100],"imageTransform":{"angle":0,"scale":[1,1],"crop":null}}]}}});
        assert!(super::fitted_endpoint_visual(&image, [&anchor, &anchor]));
        image["fileId"] = serde_json::json!("changed");
        assert!(!super::fitted_endpoint_visual(&image, [&anchor, &anchor]));
        image["fileId"] = serde_json::json!("f");
        image["width"] = serde_json::json!(99);
        assert!(!super::fitted_endpoint_visual(&image, [&anchor, &anchor]));
    }

    use super::*;

    #[test]
    fn outer_caption_lane_does_not_force_a_full_figure_width_crossing() {
        let source = Rect {
            l: 0.,
            r: 100.,
            t: 200.,
            b: 300.,
        };
        let target = Rect {
            l: 400.,
            r: 500.,
            t: 200.,
            b: 300.,
        };
        let obstacles = [source, target];
        let mut grid = Grid::new(&obstacles, &[-100.], -200., 700.);
        // Keep the caption outside the central region while leaving routing
        // legal there. This exercises the outer-lane fallback specifically.
        grid.caption_borders.push(Rect {
            l: -1000.,
            r: 1000.,
            t: 0.,
            b: 1000.,
        });
        let (route, center) = route_between_ports(
            source,
            target,
            &grid,
            &obstacles,
            -100.,
            Some([150., 40.]),
            ([true; 4], [true; 4]),
        )
        .unwrap();
        let length = route
            .windows(2)
            .map(|p| (p[0][0] - p[1][0]).abs() + (p[0][1] - p[1][1]).abs())
            .sum::<f64>();
        assert!(length < 1500., "unnecessary full-width detour: {length}");
        assert!(center[1] < 0.);
    }

    #[test]
    fn a_short_direct_connection_does_not_require_an_artificial_detour() {
        let grid = Grid::with_points(&[], &[0.], 0., 10., &[]);
        let path = grid
            .route(&[[0., 0.]], &[[10., 0.]], None, Some([true; 4]))
            .unwrap();
        assert_eq!(path, vec![[0., 0.], [10., 0.]]);
    }

    #[test]
    fn a_confined_turn_falls_back_to_the_legal_short_approach() {
        let grid = Grid::with_points(&[], &[0., 10.], 0., 10., &[]);
        let target_ports = Some([true; 4]);
        assert!(
            grid.route_with_clearance(&[[0., 0.]], &[[10., 10.]], None, target_ports, 26.)
                .is_err()
        );
        let path = grid
            .route(&[[0., 0.]], &[[10., 10.]], None, target_ports)
            .unwrap();
        assert_eq!(path.first(), Some(&[0., 0.]));
        assert_eq!(path.last(), Some(&[10., 10.]));
        assert_eq!(path.len(), 3);
    }
}
