//! Independently laid out comparison panels with native editable frame contents.
use crate::{
    layout,
    sketch::{self, InvalidPlan, Plan},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};

fn bounds(elements: &[Value]) -> [f64; 4] {
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for element in elements {
        let x = element["x"].as_f64().unwrap();
        let y = element["y"].as_f64().unwrap();
        let points = element["points"]
            .as_array()
            .map(|points| {
                points
                    .iter()
                    .map(|p| [x + p[0].as_f64().unwrap(), y + p[1].as_f64().unwrap()])
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                vec![
                    [x, y],
                    [
                        x + element["width"].as_f64().unwrap(),
                        y + element["height"].as_f64().unwrap(),
                    ],
                ]
            });
        for [x, y] in points {
            bounds[0] = bounds[0].min(x);
            bounds[1] = bounds[1].min(y);
            bounds[2] = bounds[2].max(x);
            bounds[3] = bounds[3].max(y);
        }
    }
    bounds
}
fn reference(value: &mut Value, ids: &HashMap<String, String>) {
    if let Some(id) = value.as_str().and_then(|id| ids.get(id)) {
        *value = json!(id);
    }
}
pub(crate) fn compile(plan: &Plan) -> Result<Value, InvalidPlan> {
    let fail = |s: &str| InvalidPlan(s.into());
    if plan.panels.len() > 12 {
        return Err(fail("at most 12 panels"));
    }
    let mut panels = HashSet::new();
    let mut cells = HashSet::new();
    for panel in &plan.panels {
        if panel.id.trim().is_empty()
            || !panels.insert(panel.id.as_str())
            || panel.title.trim().is_empty()
            || panel.title.chars().count() > 120
            || panel.column > 10
            || panel.row > 10
            || !cells.insert((panel.column, panel.row))
        {
            return Err(fail(
                "panels require unique nonempty IDs, 1..120 character titles and unique column/row within 0..10",
            ));
        }
    }
    let mut nodes = HashMap::new();
    for node in &plan.nodes {
        if nodes.insert(node.id.as_str(), node).is_some() {
            return Err(fail("node IDs must be globally unique"));
        }
        if !node.panel.as_deref().is_some_and(|id| panels.contains(id)) {
            return Err(fail("each node must reference a declared panel"));
        }
    }
    for edge in &plan.edges {
        if !nodes.contains_key(edge.from.as_str()) || !nodes.contains_key(edge.to.as_str()) {
            return Err(fail("edge endpoint does not exist"));
        }
    }
    let mut drawings = Vec::new();
    let mut columns = BTreeMap::<u16, f64>::new();
    let mut rows = BTreeMap::<u16, f64>::new();
    for (index, panel) in plan.panels.iter().enumerate() {
        let members = plan
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.panel.as_deref() == Some(panel.id.as_str()))
            .collect::<Vec<_>>();
        if members.is_empty() {
            return Err(fail("each panel requires at least one node"));
        }
        let edges = plan
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                nodes[e.from.as_str()].panel.as_deref() == Some(panel.id.as_str())
                    && nodes[e.to.as_str()].panel == nodes[e.from.as_str()].panel
            })
            .collect::<Vec<_>>();
        let local = Plan {
            message: plan.message.clone(),
            audience: plan.audience.clone(),
            layout: plan.layout,
            panels: Vec::new(),
            nodes: members
                .iter()
                .map(|(_, n)| {
                    let mut n = (*n).clone();
                    n.panel = None;
                    n
                })
                .collect(),
            edges: edges.iter().map(|(_, e)| (*e).clone()).collect(),
            assumptions: plan.assumptions.clone(),
        };
        let mut scene = sketch::compile(&local)?;
        let elements = scene["elements"].as_array_mut().unwrap();
        let mut ids = HashMap::new();
        for (local, (global, _)) in members.iter().enumerate() {
            ids.insert(format!("node-{local}"), format!("node-{global}"));
            ids.insert(format!("label-{local}"), format!("label-{global}"));
        }
        for (local, (global, _)) in edges.iter().enumerate() {
            ids.insert(format!("edge-{local}"), format!("edge-{global}"));
            ids.insert(
                format!("edge-label-{local}"),
                format!("edge-label-{global}"),
            );
        }
        for element in elements.iter_mut() {
            reference(&mut element["id"], &ids);
            if element.get("containerId").is_some() {
                reference(&mut element["containerId"], &ids);
            }
            for binding in ["startBinding", "endBinding"] {
                if let Some(b) = element.get_mut(binding).and_then(Value::as_object_mut) {
                    reference(b.get_mut("elementId").unwrap(), &ids);
                }
            }
            for binding in element["boundElements"].as_array_mut().unwrap() {
                reference(&mut binding["id"], &ids);
            }
            if let Some(data) = element.get_mut("customData").and_then(Value::as_object_mut) {
                data.remove("scientificPlan");
            }
            element["frameId"] = json!(format!("panel-{index}"));
        }
        let b = bounds(elements);
        let width = (b[2] - b[0] + 80.).max(320.);
        let title = layout::balanced_heading(&panel.title, width - 80., 20.);
        let title_height = title.lines().count().max(1) as f64 * 25.;
        let height = b[3] - b[1] + title_height + 160.;
        columns
            .entry(panel.column)
            .and_modify(|v| *v = v.max(width))
            .or_insert(width);
        rows.entry(panel.row)
            .and_modify(|v| *v = v.max(height))
            .or_insert(height);
        drawings.push((elements.clone(), b, width, height, title, title_height));
    }
    // Cross-panel captions need actual space between logical panel tracks.
    // Reserve only the boundaries crossed by an explicit relation.
    let mut column_gaps = BTreeMap::<_, f64>::new();
    let mut row_gaps = BTreeMap::<_, f64>::new();
    let panel_by_id: HashMap<_, _> = plan.panels.iter().map(|p| (p.id.as_str(), p)).collect();
    for edge in &plan.edges {
        let from = panel_by_id[nodes[edge.from.as_str()].panel.as_deref().unwrap()];
        let to = panel_by_id[nodes[edge.to.as_str()].panel.as_deref().unwrap()];
        if from.id == to.id || edge.label.is_empty() {
            continue;
        }
        let wrapped = layout::wrap(&edge.label, layout::edge_label_width(&edge.label), 16.);
        let width = layout::text_width(&wrapped, 16.);
        let height = wrapped.lines().count().max(1) as f64 * 20.;
        for (&column, _) in columns.range(from.column.min(to.column)..from.column.max(to.column)) {
            column_gaps
                .entry(column)
                .and_modify(|gap| *gap = gap.max(width + 40.))
                .or_insert((width + 40.).max(100.));
        }
        for (&row, _) in rows.range(from.row.min(to.row)..from.row.max(to.row)) {
            row_gaps
                .entry(row)
                .and_modify(|gap| *gap = gap.max(height + 40.))
                .or_insert((height + 40.).max(100.));
        }
    }
    let mut elements = Vec::new();
    for (index, (panel, (mut children, b, width, _height, title, title_height))) in
        plan.panels.iter().zip(drawings).enumerate()
    {
        // Each grid row already reserves its tallest panel. Align frame bottoms
        // without moving children or changing scientific labels.
        let height = rows[&panel.row];
        let x = columns
            .range(..panel.column)
            .map(|(column, v)| v + column_gaps.get(column).copied().unwrap_or(100.))
            .sum::<f64>();
        let y = rows
            .range(..panel.row)
            .map(|(row, v)| v + row_gaps.get(row).copied().unwrap_or(100.))
            .sum::<f64>();
        for child in &mut children {
            child["x"] = json!(child["x"].as_f64().unwrap() + x + 40. - b[0]);
            child["y"] = json!(child["y"].as_f64().unwrap() + y + 80. + title_height - b[1]);
        }
        elements.extend(children);
        let id = format!("panel-{index}");
        let mut heading = sketch::text(
            &format!("panel-title-{index}"),
            &title,
            x + 40.,
            y + 30.,
            width - 80.,
        );
        heading["height"] = json!(title_height);
        heading["originalText"] = json!(panel.title);
        heading["textAlign"] = json!("left");
        heading["frameId"] = json!(id);
        heading["customData"] = json!({"panelId":panel.id});
        elements.push(heading);
        let mut frame = sketch::base(&id, "frame", x, y, width, height);
        // Excalidraw renders a null name as the placeholder "Frame", including
        // in exports. The editable scientific heading already lives inside it.
        frame["name"] = json!("");
        frame["customData"] = json!({"panelId":panel.id,"title":panel.title});
        elements.push(frame);
    }
    crate::panel_routes::connect(plan, &mut elements)?;
    elements[0]["customData"]["scientificPlan"] = json!(plan);
    Ok(
        json!({"type":"excalidraw","version":2,"source":"autonomics","elements":elements,"files":{},"appState":{"viewBackgroundColor":"#ffffff","currentItemRoughness":0,"currentItemFontFamily":9},"scientificPlan":plan}),
    )
}
