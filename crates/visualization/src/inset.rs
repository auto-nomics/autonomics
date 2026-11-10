//! Editable, version-bound image detail snapshots. Shared source files stay immutable.
use crate::{
    image::{self, InvalidImage},
    sketch::{base, text},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub element_id: String,
    /// Fractional rectangle in the current visible image, after flips and before rotation.
    pub region: Region,
    pub x: f64,
    pub y: f64,
    pub max_size: f64,
    pub title: String,
}
fn number(e: &Value, key: &str) -> Result<f64, InvalidImage> {
    e[key]
        .as_f64()
        .filter(|v| v.is_finite())
        .ok_or(InvalidImage("invalid image geometry"))
}
pub fn crop(source: &Value, width: u32, height: u32) -> Result<[f64; 4], InvalidImage> {
    let result = if source["crop"].is_null() {
        [0., 0., f64::from(width), f64::from(height)]
    } else {
        let c = &source["crop"];
        if (number(c, "naturalWidth")? - f64::from(width)).abs() > 0.01
            || (number(c, "naturalHeight")? - f64::from(height)).abs() > 0.01
        {
            return Err(InvalidImage("saved crop does not match source dimensions"));
        }
        [
            number(c, "x")?,
            number(c, "y")?,
            number(c, "width")?,
            number(c, "height")?,
        ]
    };
    if result[0] < 0.
        || result[1] < 0.
        || result[2] <= 0.
        || result[3] <= 0.
        || result[0] + result[2] > f64::from(width) + 1e-6
        || result[1] + result[3] > f64::from(height) + 1e-6
    {
        return Err(InvalidImage("source crop is outside the image"));
    }
    Ok(result)
}
fn edge(center: [f64; 2], size: [f64; 2], angle: f64, toward: [f64; 2]) -> [f64; 2] {
    let d = [toward[0] - center[0], toward[1] - center[1]];
    let (s, c) = angle.sin_cos();
    let local = [c * d[0] + s * d[1], -s * d[0] + c * d[1]];
    let t = (size[0] / 2. / local[0].abs()).min(size[1] / 2. / local[1].abs());
    let length = d[0].hypot(d[1]);
    [
        center[0] + d[0] * (t + 8. / length),
        center[1] + d[1] * (t + 8. / length),
    ]
}
pub fn compile(
    document: &Value,
    spec: &Spec,
    width: u32,
    height: u32,
    revision: i64,
    sha256: &str,
) -> Result<Value, InvalidImage> {
    crate::scene::apply(document, &[]).map_err(|_| InvalidImage("invalid scene"))?;
    let source = image::element(document, &spec.element_id)?;
    let r = &spec.region;
    if [r.x, r.y, r.width, r.height, spec.x, spec.y, spec.max_size]
        .iter()
        .any(|v| !v.is_finite())
        || r.x < 0.
        || r.y < 0.
        || r.width <= 0.
        || r.height <= 0.
        || r.x + r.width > 1. + 1e-9
        || r.y + r.height > 1. + 1e-9
        || !(32. ..=2048.).contains(&spec.max_size)
        || spec.x.abs() > 1e6
        || spec.y.abs() > 1e6
        || spec.title.trim().is_empty()
        || spec.title.chars().count() > 80
        || width == 0
        || height == 0
    {
        return Err(InvalidImage(
            "invalid inset region, size, title or position",
        ));
    }
    let (x, y, w, h, angle) = (
        number(source, "x")?,
        number(source, "y")?,
        number(source, "width")?,
        number(source, "height")?,
        number(source, "angle")?,
    );
    if w <= 0. || h <= 0. {
        return Err(InvalidImage("invalid source dimensions"));
    }
    let scale = source["scale"]
        .as_array()
        .ok_or(InvalidImage("invalid image flip"))?;
    if scale.len() != 2 || scale.iter().any(|v| !matches!(v.as_i64(), Some(-1 | 1))) {
        return Err(InvalidImage("invalid image flip"));
    }
    let [cx, cy, cw, ch] = crop(source, width, height)?;
    let u = if scale[0] == -1 {
        1. - r.x - r.width
    } else {
        r.x
    };
    let v = if scale[1] == -1 {
        1. - r.y - r.height
    } else {
        r.y
    };
    let native_crop = json!({"x":cx+u*cw,"y":cy+v*ch,"width":r.width*cw,"height":r.height*ch,"naturalWidth":width,"naturalHeight":height});
    let roi_size = [r.width * w, r.height * h];
    let (s, c) = angle.sin_cos();
    let original_center = [x + w / 2., y + h / 2.];
    let d = [
        (r.x + r.width / 2. - 0.5) * w,
        (r.y + r.height / 2. - 0.5) * h,
    ];
    let roi_center = [
        original_center[0] + c * d[0] - s * d[1],
        original_center[1] + s * d[0] + c * d[1],
    ];
    let rotated = [
        c.abs() * roi_size[0] + s.abs() * roi_size[1],
        s.abs() * roi_size[0] + c.abs() * roi_size[1],
    ];
    let factor = spec.max_size / rotated[0].max(rotated[1]);
    let display = [roi_size[0] * factor, roi_size[1] * factor];
    let bounds = [rotated[0] * factor, rotated[1] * factor];
    let border_width = (bounds[0] + 24.).max(180.);
    let title = crate::layout::wrap(spec.title.trim(), border_width - 24., 18.);
    let title_height = title.lines().count().max(1) as f64 * 22.5;
    let border_height = 12. + title_height + 8. + bounds[1] + 12.;
    let center = [spec.x + border_width / 2., spec.y + border_height / 2.];
    if (center[0] - roi_center[0]).hypot(center[1] - roi_center[1]) < 1. {
        return Err(InvalidImage("place the inset away from the source region"));
    }
    let ids = (0..6)
        .map(|_| uuid::Uuid::new_v4().to_string())
        .collect::<Vec<_>>();
    let (anchor_id, image_id, title_id, roi_id, line_id, group) =
        (&ids[0], &ids[1], &ids[2], &ids[3], &ids[4], &ids[5]);
    let reference = json!({"kind":"image_inset_snapshot","source_element_id":source["id"],"source_file_id":source["fileId"],"source_revision":revision,"source_sha256":sha256,"region":r,"crop":native_crop,"anchor_id":anchor_id,"image_id":image_id,"region_id":roi_id,"callout_id":line_id});
    let mut anchor = base(
        anchor_id,
        "rectangle",
        spec.x,
        spec.y,
        border_width,
        border_height,
    );
    anchor["strokeColor"] = json!("#64748b");
    anchor["groupIds"] = json!([group]);
    anchor["boundElements"] = json!([{"id":line_id,"type":"arrow"}]);
    anchor["customData"] = json!({"inset":reference,"visualAssetElementId":image_id,"visualAssetGroupId":group,"visualLabelIds":[title_id]});
    let mut image = source.clone();
    image["id"] = json!(image_id);
    image["x"] = json!(spec.x + border_width / 2. - display[0] / 2.);
    image["y"] = json!(spec.y + 12. + title_height + 8. + bounds[1] / 2. - display[1] / 2.);
    image["width"] = json!(display[0]);
    image["height"] = json!(display[1]);
    image["crop"] = native_crop.clone();
    image["groupIds"] = json!([group]);
    image["frameId"] = Value::Null;
    image["boundElements"] = json!([]);
    image["roundness"] = Value::Null;
    image["locked"] = json!(false);
    image["isDeleted"] = json!(false);
    image["status"] = json!("saved");
    if !image["customData"].is_object() {
        image["customData"] = json!({});
    }
    image["customData"]["semanticAnchorId"] = json!(anchor_id);
    image["customData"]["inset"] = reference.clone();
    image["customData"]["sha256"] = json!(sha256);
    if image["customData"].get("modifications").is_none() {
        image["customData"]["modifications"] = json!([]);
    }
    image["customData"]["modifications"].as_array_mut().ok_or(InvalidImage("image modifications must be a list"))?.push(json!({"kind":"inset","source_element_id":source["id"],"source_revision":revision,"region":r,"crop":native_crop,"based_on_file_id":source["fileId"],"based_on_sha256":sha256,"result_sha256":sha256,"created_at":chrono::Utc::now().to_rfc3339()}));
    crate::scene::touch(&mut image);
    let mut label = text(
        title_id,
        &title,
        spec.x + 12.,
        spec.y + 12.,
        border_width - 24.,
    );
    label["originalText"] = json!(spec.title.trim());
    label["fontSize"] = json!(18);
    label["height"] = json!(title_height);
    label["groupIds"] = json!([group]);
    let mut roi = base(
        roi_id,
        "rectangle",
        roi_center[0] - roi_size[0] / 2.,
        roi_center[1] - roi_size[1] / 2.,
        roi_size[0],
        roi_size[1],
    );
    roi["angle"] = json!(angle);
    roi["strokeColor"] = json!("#64748b");
    roi["strokeStyle"] = json!("dashed");
    roi["frameId"] = source["frameId"].clone();
    roi["boundElements"] = json!([{"id":line_id,"type":"arrow"}]);
    roi["customData"] = json!({"inset":reference,"annotationKind":"inset_region"});
    let start = edge(roi_center, roi_size, angle, center);
    let end = edge(center, [border_width, border_height], 0., roi_center);
    let min = [start[0].min(end[0]), start[1].min(end[1])];
    let mut line = base(
        line_id,
        "arrow",
        min[0],
        min[1],
        (start[0] - end[0]).abs(),
        (start[1] - end[1]).abs(),
    );
    line.as_object_mut().unwrap().extend(json!({"strokeColor":"#64748b","strokeStyle":"dashed","startArrowhead":null,"endArrowhead":null,"elbowed":false,"points":[[start[0]-min[0],start[1]-min[1]],[end[0]-min[0],end[1]-min[1]]],"startBinding":{"elementId":roi_id,"focus":0,"gap":8},"endBinding":{"elementId":anchor_id,"focus":0,"gap":8},"customData":{"inset":reference,"annotationKind":"inset_callout"}}).as_object().unwrap().clone());
    let mut next = document.clone();
    next["elements"]
        .as_array_mut()
        .unwrap()
        .extend([roi, line, anchor, image, label]);
    Ok(next)
}
