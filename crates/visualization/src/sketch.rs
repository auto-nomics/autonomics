//! Structured scientific plans compiled into editable Excalidraw scenes.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub message: String,
    pub audience: String,
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub assumptions: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub column: u16,
    pub row: u16,
    #[serde(default)]
    pub shape: Shape,
    #[serde(default)]
    pub evidence: Vec<String>,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    #[default]
    Rectangle,
    Ellipse,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub inhibitory: bool,
    #[serde(default)]
    pub evidence: Vec<String>,
}
#[derive(Debug, thiserror::Error)]
#[error("invalid scientific sketch: {0}")]
pub struct InvalidPlan(pub String);
fn base(id: &str, kind: &str, x: f64, y: f64, width: f64, height: f64) -> Value {
    json!({"id":id,"type":kind,"x":x,"y":y,"width":width,"height":height,"angle":0,"strokeColor":"#1e293b","backgroundColor":"transparent","fillStyle":"solid","strokeWidth":2,"strokeStyle":"solid","roughness":0,"opacity":100,"groupIds":[],"frameId":null,"roundness":null,"seed":1,"version":1,"versionNonce":1,"isDeleted":false,"boundElements":[],"updated":1,"link":null,"locked":false})
}
fn text(id: &str, label: &str, x: f64, y: f64, width: f64) -> Value {
    let mut value = base(id, "text", x, y, width, 50.0);
    value.as_object_mut().unwrap().extend(json!({"text":label,"originalText":label,"fontSize":20,"fontFamily":2,"textAlign":"center","verticalAlign":"middle","containerId":null,"autoResize":false,"lineHeight":1.25}).as_object().unwrap().clone());
    value
}
/// Grid coordinates express author intent; no scientific relationship is inferred.
/// Output retains the plan so an agent can inspect assumptions and evidence later.
pub fn compile(plan: &Plan) -> Result<Value, InvalidPlan> {
    let fail = |message: &str| InvalidPlan(message.into());
    if plan.message.trim().is_empty() || plan.audience.trim().is_empty() {
        return Err(fail("message and audience are required"));
    }
    if plan.nodes.is_empty() || plan.nodes.len() > 100 || plan.edges.len() > 200 {
        return Err(fail("requires 1..100 nodes and at most 200 edges"));
    }
    let mut positions = HashMap::new();
    let mut cells = std::collections::HashSet::new();
    for node in &plan.nodes {
        if node.id.trim().is_empty()
            || node.label.trim().is_empty()
            || node.label.chars().count() > 120
            || node.column > 30
            || node.row > 30
        {
            return Err(fail("invalid node id, label or grid position"));
        }
        if positions
            .insert(
                node.id.clone(),
                (f64::from(node.column) * 400.0, f64::from(node.row) * 200.0),
            )
            .is_some()
            || !cells.insert((node.column, node.row))
        {
            return Err(fail("node IDs and grid positions must be unique"));
        }
    }
    let mut elements = Vec::new();
    for (index, node) in plan.nodes.iter().enumerate() {
        let (x, y) = positions[&node.id];
        let id = format!("node-{index}");
        let label_id = format!("label-{index}");
        let mut shape = base(
            &id,
            match node.shape {
                Shape::Rectangle => "rectangle",
                Shape::Ellipse => "ellipse",
            },
            x,
            y,
            220.0,
            100.0,
        );
        shape["backgroundColor"] = json!("#e0f2fe");
        shape["boundElements"] = json!([{"id":label_id,"type":"text"}]);
        shape["customData"] = json!({"scientificId":node.id,"evidence":node.evidence});
        if index == 0 {
            shape["customData"]["scientificPlan"] = json!(plan);
        }
        let mut label = text(&label_id, &node.label, x + 10.0, y + 25.0, 200.0);
        label["containerId"] = json!(id);
        elements.extend([shape, label]);
    }
    for (index, edge) in plan.edges.iter().enumerate() {
        let &(x1, y1) = positions
            .get(&edge.from)
            .ok_or_else(|| fail("edge source does not exist"))?;
        let &(x2, y2) = positions
            .get(&edge.to)
            .ok_or_else(|| fail("edge target does not exist"))?;
        if edge.from == edge.to {
            return Err(fail("self edges require explicit manual layout"));
        }
        let (sx, sy, ex, ey) = if (x2 - x1).abs() >= (y2 - y1).abs() {
            if x2 > x1 {
                (x1 + 230.0, y1 + 50.0, x2 - 10.0, y2 + 50.0)
            } else {
                (x1 - 10.0, y1 + 50.0, x2 + 230.0, y2 + 50.0)
            }
        } else if y2 > y1 {
            (x1 + 110.0, y1 + 110.0, x2 + 110.0, y2 - 10.0)
        } else {
            (x1 + 110.0, y1 - 10.0, x2 + 110.0, y2 + 110.0)
        };
        let mut arrow = base(
            &format!("edge-{index}"),
            "arrow",
            sx,
            sy,
            (ex - sx).abs(),
            (ey - sy).abs(),
        );
        arrow.as_object_mut().unwrap().extend(json!({"points":[[0,0],[ex-sx,ey-sy]],"startBinding":null,"endBinding":null,"startArrowhead":null,"endArrowhead":if edge.inhibitory{"bar"}else{"arrow"},"elbowed":false,"customData":{"from":edge.from,"to":edge.to,"evidence":edge.evidence}}).as_object().unwrap().clone());
        let from_index = plan
            .nodes
            .iter()
            .position(|node| node.id == edge.from)
            .unwrap();
        let to_index = plan
            .nodes
            .iter()
            .position(|node| node.id == edge.to)
            .unwrap();
        let arrow_id = format!("edge-{index}");
        arrow["startBinding"] =
            json!({"elementId":format!("node-{from_index}"),"focus":0,"gap":10});
        arrow["endBinding"] = json!({"elementId":format!("node-{to_index}"),"focus":0,"gap":10});
        for node_index in [from_index, to_index] {
            elements[node_index * 2]["boundElements"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":arrow_id,"type":"arrow"}));
        }
        if !edge.label.is_empty() {
            arrow["boundElements"] = json!([{"id":format!("edge-label-{index}"),"type":"text"}]);
        }
        elements.push(arrow);
        if !edge.label.is_empty() {
            let mut label = text(
                &format!("edge-label-{index}"),
                &edge.label,
                (sx + ex) / 2.0 - 90.0,
                (sy + ey) / 2.0 - 55.0,
                180.0,
            );
            label["containerId"] = json!(arrow_id);
            let available = ((ex - sx).hypot(ey - sy) - 20.0).clamp(40.0, 180.0);
            let mut wrapped = String::new();
            let mut line_width = 0.0;
            for character in edge.label.chars() {
                let char_width = if character.is_ascii() { 8.5 } else { 16.0 };
                if character == '\n' {
                    wrapped.push(character);
                    line_width = 0.0;
                    continue;
                }
                if line_width + char_width > available && line_width > 0.0 {
                    wrapped.push('\n');
                    line_width = 0.0;
                }
                wrapped.push(character);
                line_width += char_width;
            }
            label["fontSize"] = json!(16);
            label["text"] = json!(wrapped);
            label["width"] = json!(
                available.min(
                    edge.label
                        .chars()
                        .map(|c| if c.is_ascii() { 8.5 } else { 16.0 })
                        .sum::<f64>()
                )
            );
            label["height"] = json!(wrapped.lines().count().max(1) as f64 * 20.0);
            elements.push(label);
        }
    }
    Ok(
        json!({"type":"excalidraw","version":2,"source":"autonomics","elements":elements,"files":{},"appState":{"viewBackgroundColor":"#ffffff","currentItemRoughness":0,"currentItemFontFamily":2},"scientificPlan":plan}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plan_compiles_to_separate_editable_objects() {
        let plan:Plan=serde_json::from_value(json!({"message":"Signal activates response","audience":"researchers","nodes":[{"id":"a","label":"Ligand","column":0,"row":0},{"id":"b","label":"Receptor","column":1,"row":0,"shape":"ellipse"}],"edges":[{"from":"a","to":"b","label":"binds"}],"assumptions":["schematic, not to scale"]})).unwrap();
        let scene = compile(&plan).unwrap();
        assert_eq!(scene["elements"].as_array().unwrap().len(), 6);
        assert_eq!(scene["elements"][1]["containerId"], "node-0");
        assert_eq!(
            scene["scientificPlan"]["assumptions"][0],
            "schematic, not to scale"
        );
        assert_eq!(scene["elements"][4]["startBinding"]["elementId"], "node-0");
        assert_eq!(scene["elements"][5]["containerId"], "edge-0");
        assert_eq!(scene["elements"][0]["boundElements"][1]["type"], "arrow");
        let mut invalid = plan.clone();
        invalid.edges[0].to = "missing".into();
        assert!(compile(&invalid).is_err());
        invalid = plan.clone();
        invalid.nodes[1].id = "a".into();
        assert!(compile(&invalid).is_err());
        invalid = plan;
        invalid.nodes[1].column = 0;
        assert!(compile(&invalid).is_err());
    }
}
