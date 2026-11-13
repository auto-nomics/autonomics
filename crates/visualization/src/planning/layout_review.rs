//! Read-only geometry of the compiled plan, not a browser render or scientific review.
use super::Brief;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelLayout {
    pub id: String,
    pub bounds: Bounds,
}
/// Declared plan topology, not inferred geometry or biological causality.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LabelContext {
    Node {
        node_id: String,
        panel_id: Option<String>,
    },
    Relation {
        from_node_id: String,
        to_node_id: String,
        from_panel_id: Option<String>,
        to_panel_id: Option<String>,
        cross_panel: Option<bool>,
    },
    Panel {
        panel_id: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongLabel {
    #[serde(default)]
    pub plan_field: Option<String>,
    #[serde(default)]
    pub context: Option<LabelContext>,
    pub preview: String,
    pub lines: usize,
    pub width: f64,
    pub height: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutReview {
    pub status: String,
    pub scope: String,
    pub bounds: Option<Bounds>,
    pub aspect_ratio: Option<f64>,
    pub panels: Vec<PanelLayout>,
    pub error: Option<String>,
    #[serde(default)]
    pub long_labels: Vec<LongLabel>,
    #[serde(default)]
    pub long_label_count: usize,
    #[serde(default)]
    pub long_labels_omitted: usize,
}
fn rect(e: &Value) -> Bounds {
    Bounds {
        x: e["x"].as_f64().unwrap(),
        y: e["y"].as_f64().unwrap(),
        width: e["width"].as_f64().unwrap(),
        height: e["height"].as_f64().unwrap(),
    }
}
fn bounds(elements: &[Value]) -> Bounds {
    let (mut left, mut top, mut right, mut bottom) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for e in elements {
        let r = rect(e);
        let points = if let Some(points) = e["points"].as_array() {
            points
                .iter()
                .map(|p| [r.x + p[0].as_f64().unwrap(), r.y + p[1].as_f64().unwrap()])
                .collect::<Vec<_>>()
        } else {
            vec![[r.x, r.y], [r.x + r.width, r.y + r.height]]
        };
        for [x, y] in points {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
        }
    }
    Bounds {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}
impl Brief {
    fn label_field(&self, element: &Value) -> Option<String> {
        let id = element["id"].as_str()?;
        let original = element["originalText"].as_str()?;
        for (prefix, collection) in [
            ("edge-label-", "edges"),
            ("label-", "nodes"),
            ("panel-title-", "panels"),
        ] {
            let Some(index) = id
                .strip_prefix(prefix)
                .and_then(|v| v.parse::<usize>().ok())
            else {
                continue;
            };
            let source = match collection {
                "edges" => self.edges.get(index).map(|v| v.label.as_str()),
                "nodes" => self.nodes.get(index).map(|v| v.label.as_str()),
                _ => self.panels.get(index).map(|v| v.title.as_str()),
            };
            if source == Some(original) {
                return Some(format!(
                    "{collection}[{index}].{}",
                    if collection == "panels" {
                        "title"
                    } else {
                        "label"
                    }
                ));
            }
        }
        None
    }
    fn label_context(&self, field: &str) -> Option<LabelContext> {
        let (collection, rest) = field.split_once('[')?;
        let index = rest.split_once(']')?.0.parse::<usize>().ok()?;
        match collection {
            "nodes" => self.nodes.get(index).map(|node| LabelContext::Node {
                node_id: node.id.clone(),
                panel_id: node.panel.clone(),
            }),
            "panels" => self.panels.get(index).map(|panel| LabelContext::Panel {
                panel_id: panel.id.clone(),
            }),
            "edges" => {
                let edge = self.edges.get(index)?;
                let from = self.nodes.iter().find(|node| node.id == edge.from)?;
                let to = self.nodes.iter().find(|node| node.id == edge.to)?;
                Some(LabelContext::Relation {
                    from_node_id: from.id.clone(),
                    to_node_id: to.id.clone(),
                    from_panel_id: from.panel.clone(),
                    to_panel_id: to.panel.clone(),
                    cross_panel: from
                        .panel
                        .as_ref()
                        .zip(to.panel.as_ref())
                        .map(|(a, b)| a != b),
                })
            }
            _ => None,
        }
    }
    pub fn layout_review(&self) -> LayoutReview {
        let mut review = LayoutReview {
            status: "unavailable".into(),
            scope: "compiled_sketch_only_excludes_future_captions_assets_and_pixel_review".into(),
            bounds: None,
            aspect_ratio: None,
            panels: vec![],
            error: None,
            long_labels: vec![],
            long_label_count: 0,
            long_labels_omitted: 0,
        };
        match crate::sketch::compile(&self.plan()) {
            Err(error) => review.error = Some(error.to_string()),
            Ok(scene) => {
                let elements = scene["elements"]
                    .as_array()
                    .expect("compiler returns native elements");
                let b = bounds(elements);
                review.aspect_ratio = Some(b.width / b.height);
                review.bounds = Some(b);
                review.panels = elements
                    .iter()
                    .filter(|e| e["type"] == "frame")
                    .map(|e| PanelLayout {
                        id: e["customData"]["panelId"].as_str().unwrap().into(),
                        bounds: rect(e),
                    })
                    .collect();
                let mut labels = elements
                    .iter()
                    .filter(|e| e["type"] == "text")
                    .filter_map(|e| {
                        let lines = e["text"].as_str()?.split('\n').count();
                        (lines >= 4).then(|| LongLabel {
                            context: self
                                .label_field(e)
                                .as_deref()
                                .and_then(|field| self.label_context(field)),
                            plan_field: self.label_field(e),
                            preview: e["originalText"]
                                .as_str()
                                .unwrap_or_default()
                                .chars()
                                .take(160)
                                .collect(),
                            lines,
                            width: e["width"].as_f64().unwrap(),
                            height: e["height"].as_f64().unwrap(),
                        })
                    })
                    .collect::<Vec<_>>();
                labels.sort_by_key(|label| std::cmp::Reverse(label.lines));
                review.long_label_count = labels.len();
                labels.truncate(20);
                review.long_labels_omitted = review.long_label_count - labels.len();
                review.long_labels = labels;
                review.status = "available".into();
            }
        }
        review
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn bounds_use_negative_arrow_points_instead_of_unsigned_dimensions() {
        let b =
            bounds(&[json!({"x":10,"y":20,"width":100,"height":30,"points":[[-50,-70],[100,30]]})]);
        assert_eq!([b.x, b.y, b.width, b.height], [-40., -50., 150., 100.]);
    }
}
