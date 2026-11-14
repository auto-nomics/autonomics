//! Scalar native crop with source dimensions resolved from the exact file.
use super::*;
#[tool(
    name = "visualization_crop_image",
    description = "Directly crop an image using x/y/width/height fractions of the original source. Source dimensions are read from the current file. Preserves files/provenance. Optional fit_to_crop fits a single-primary owner to the crop rectangle in the same atomic edit. Inspect the current canvas first."
)]
#[serde(deny_unknown_fields)]
pub struct CropInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
    pub element_id: String,
    pub file_id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub fit_to_crop: bool,
}
struct CropTool {
    path: PathBuf,
}
fn operations(path: PathBuf, input: &CropInput) -> Result<Value, ToolError> {
    let store = Store::open(path).map_err(failure)?;
    if store
        .get(&input.project_id, &input.figure_id)
        .map_err(failure)?
        .revision
        != input.expected_revision
    {
        return Err(failure(visualization::Error::Conflict));
    }
    let scene = store
        .revision(&input.project_id, &input.figure_id, input.expected_revision)
        .map_err(failure)?;
    let image =
        visualization::image::element(&scene.document, &input.element_id).map_err(failure)?;
    if image["fileId"] != input.file_id {
        return Err(invalid("crop file_id does not match the current image"));
    }
    let image_id = image["id"]
        .as_str()
        .ok_or_else(|| invalid("missing image ID"))?;
    let info =
        visualization_assets::image_info_document(&scene.document, image_id).map_err(failure)?;
    let mut result = vec![
        json!({"op":"crop_image","id":image_id,"file_id":input.file_id,"natural_width":info["natural_width"],"natural_height":info["natural_height"],"normalized_crop":[input.x,input.y,input.width,input.height]}),
    ];
    if input.fit_to_crop {
        let owner=image["customData"]["semanticAnchorId"].as_str().ok_or_else(||invalid("fit_to_crop requires an owned primary image; crop standalone/accessory images without this option"))?;
        let anchor = scene.document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == owner)
            .ok_or_else(|| invalid("missing primary owner"))?;
        if anchor["customData"]["visualBadgeIds"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
        {
            return Err(invalid(
                "fit_to_crop cannot stand in for multi-image group fitting; crop without it, then refit the full group",
            ));
        }
        result.push(json!({"op":"fit_illustration_anchor","id":owner,"file_id":input.file_id,"normalized_bounds":[input.x,input.y,input.width,input.height]}));
    }
    Ok(json!(result))
}
#[async_trait]
impl ToolFunction for CropTool {
    type Input = CropInput;
    async fn run(&self, input: CropInput) -> Result<ToolResult, ToolError> {
        let crop = [input.x, input.y, input.width, input.height];
        if crop.iter().any(|v| !v.is_finite())
            || input.x < 0.
            || input.y < 0.
            || input.width <= 0.
            || input.height <= 0.
            || input.x + input.width > 1. + 1e-9
            || input.y + input.height > 1. + 1e-9
        {
            return Err(invalid(
                "crop requires finite numeric x/y >=0, width/height >0 with x+width and y+height <=1",
            ));
        }
        let operations = operations(self.path.clone(), &input)?;
        let value = json!({"action":"apply","project_id":input.project_id,"figure_id":input.figure_id,"expected_revision":input.expected_revision,"operations":operations});
        FigureTool {
            path: self.path.clone(),
        }
        .run(serde_json::from_value(value).map_err(failure)?)
        .await
    }
}
pub(crate) fn registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(CropTool { path })
}
