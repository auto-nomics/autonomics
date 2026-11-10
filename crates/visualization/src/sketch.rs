//! Structured scientific plans compiled into editable Excalidraw scenes.
use crate::layout;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub message: String,
    pub audience: String,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panels: Vec<Panel>,
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub assumptions: Vec<String>,
}
/// Explicit comparison/step panels. Each node belongs to one panel; no relation
/// is inferred between panels. Column/row place panels, not their child nodes.
#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Panel {
    pub id: String,
    pub title: String,
    pub column: u16,
    pub row: u16,
}
#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<u16>,
    #[serde(default)]
    pub shape: Shape,
    #[serde(default)]
    pub evidence: Vec<String>,
}
#[derive(
    schemars::JsonSchema, Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    #[default]
    Grid,
    Horizontal,
    Vertical,
}
#[derive(schemars::JsonSchema, Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    #[default]
    Rectangle,
    Ellipse,
}
#[derive(schemars::JsonSchema, Debug, Clone, Serialize, Deserialize)]
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
pub(crate) fn base(id: &str, kind: &str, x: f64, y: f64, width: f64, height: f64) -> Value {
    json!({"id":id,"type":kind,"x":x,"y":y,"width":width,"height":height,"angle":0,"strokeColor":"#1e293b","backgroundColor":"transparent","fillStyle":"solid","strokeWidth":2,"strokeStyle":"solid","roughness":0,"opacity":100,"groupIds":[],"frameId":null,"roundness":null,"seed":1,"version":1,"versionNonce":1,"isDeleted":false,"boundElements":[],"updated":1,"link":null,"locked":false})
}
pub(crate) fn text(id: &str, label: &str, x: f64, y: f64, width: f64) -> Value {
    let mut value = base(id, "text", x, y, width, 50.0);
    value.as_object_mut().unwrap().extend(json!({"text":label,"originalText":label,"fontSize":20,"fontFamily":9,"textAlign":"center","verticalAlign":"middle","containerId":null,"autoResize":false,"lineHeight":1.25}).as_object().unwrap().clone());
    value
}
/// Explicit grid or automatic layered layout; no scientific relationship is inferred.
/// Output retains the plan so an agent can inspect assumptions and evidence later.
pub fn compile(plan: &Plan) -> Result<Value, InvalidPlan> {
    let fail = |message: &str| InvalidPlan(message.into());
    if plan.message.trim().is_empty() || plan.audience.trim().is_empty() {
        return Err(fail("message and audience are required"));
    }
    if plan.nodes.is_empty() || plan.nodes.len() > 100 || plan.edges.len() > 200 {
        return Err(fail("requires 1..100 nodes and at most 200 edges"));
    }
    if !plan.panels.is_empty() {
        return crate::panels::compile(plan);
    }
    if plan.nodes.iter().any(|node| node.panel.is_some()) {
        return Err(fail("node panel requires a declared panel"));
    }
    let drawing = layout::arrange(plan)?;
    let mut elements = Vec::new();
    for (index, node) in plan.nodes.iter().enumerate() {
        let rect = &drawing.boxes[index];
        let (x, y) = (rect.x, rect.y);
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
            rect.width,
            rect.height,
        );
        shape["backgroundColor"] = json!("#e0f2fe");
        shape["boundElements"] = json!([{"id":label_id,"type":"text"}]);
        shape["customData"] = json!({"scientificId":node.id,"evidence":node.evidence});
        if index == 0 {
            shape["customData"]["scientificPlan"] = json!(plan);
            shape["customData"]["layoutInfo"] = json!({"kind":plan.layout,"feedback_edges":drawing.feedback_edges,"font_family":9,"font_name":crate::font_metrics::FONT_NAME,"font_sha256":crate::font_metrics::SHA256});
        }
        let mut label = text(
            &label_id,
            &rect.label,
            x + (rect.width - rect.label_width) / 2.,
            y + (rect.height - rect.label_height) / 2.,
            rect.label_width,
        );
        label["originalText"] = json!(node.label);
        label["height"] = json!(rect.label_height);
        label["containerId"] = json!(id);
        elements.extend([shape, label]);
    }
    for (index, edge) in plan.edges.iter().enumerate() {
        let route = &drawing.routes[index];
        let [sx, sy] = route[0];
        let [ex, ey] = *route.last().unwrap();
        let min_x = route.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
        let max_x = route.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);
        let min_y = route.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
        let max_y = route.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max);
        let mut arrow = base(
            &format!("edge-{index}"),
            "arrow",
            sx,
            sy,
            max_x - min_x,
            max_y - min_y,
        );
        arrow.as_object_mut().unwrap().extend(json!({"points":route.iter().map(|p|[p[0]-sx,p[1]-sy]).collect::<Vec<_>>(),"startBinding":null,"endBinding":null,"startArrowhead":null,"endArrowhead":if edge.inhibitory{"bar"}else{"arrow"},"elbowed":false,"customData":{"from":edge.from,"to":edge.to,"evidence":edge.evidence}}).as_object().unwrap().clone());
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
            let middle = route.len() / 2;
            let [lx, ly] = [
                (route[middle - 1][0] + route[middle][0]) / 2.,
                (route[middle - 1][1] + route[middle][1]) / 2.,
            ];
            let mut label = text(
                &format!("edge-label-{index}"),
                &edge.label,
                lx - 90.0,
                ly - 25.0,
                180.0,
            );
            label["containerId"] = json!(arrow_id);
            let available = ((ex - sx).hypot(ey - sy) * 0.65).clamp(40.0, 180.0);
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
            if plan.layout != Layout::Grid {
                wrapped = layout::wrap(&edge.label, available, 16.);
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
            if plan.layout != Layout::Grid {
                label["width"] = json!(layout::text_width(&wrapped, 16.).min(available).max(1.));
            }
            label["height"] = json!(wrapped.lines().count().max(1) as f64 * 20.0);
            label["x"] = json!(lx - label["width"].as_f64().unwrap() / 2.);
            label["y"] = json!(ly - label["height"].as_f64().unwrap() / 2.);
            elements.push(label);
        }
    }
    Ok(
        json!({"type":"excalidraw","version":2,"source":"autonomics","elements":elements,"files":{},"appState":{"viewBackgroundColor":"#ffffff","currentItemRoughness":0,"currentItemFontFamily":9},"scientificPlan":plan}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_relation_labels_leave_a_visible_arrow_body() {
        for direction in ["horizontal", "vertical"] {
            let plan:Plan=serde_json::from_value(json!({"message":"Explicit binding","audience":"Researchers","layout":direction,"nodes":[{"id":"a","label":"Ligand"},{"id":"b","label":"Receptor"}],"edges":[{"from":"a","to":"b","label":"binds"}]})).unwrap();
            let scene = compile(&plan).unwrap();
            let arrow = &scene["elements"][4];
            let label = &scene["elements"][5];
            let length = arrow["width"]
                .as_f64()
                .unwrap()
                .hypot(arrow["height"].as_f64().unwrap());
            let occupied = label[if direction == "horizontal" {
                "width"
            } else {
                "height"
            }]
            .as_f64()
            .unwrap();
            assert!(
                occupied < length / 2.,
                "short label must not mask most of the arrow"
            );
            assert_eq!(label["originalText"], "binds");
        }
    }
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
        invalid.nodes[1].column = Some(0);
        assert!(compile(&invalid).is_err());
    }
}
