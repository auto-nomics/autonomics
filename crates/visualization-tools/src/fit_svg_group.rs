//! Exact-candidate, backend-measured SVG group fitting without handwritten sample arrays.
use super::*;
#[tool(
    name = "visualization_fit_svg_group_preview",
    description = "Fit a complete SVG illustration group using backend-measured bounds of every primary/accessory image. Directly commits one atomic edit. Supports uncropped unrotated nonempty SVGs; inspect the resulting canvas preview."
)]
#[serde(deny_unknown_fields)]
pub struct GroupFitInput {
    pub project_id: String,
    pub figure_id: String,
    pub expected_revision: i64,
    pub element_id: String,
}
struct GroupFitTool {
    path: PathBuf,
}
fn measure(path: PathBuf, input: &GroupFitInput) -> Result<(String, Value, Value), ToolError> {
    let store = Store::open(&path).map_err(failure)?;
    if store
        .get(&input.project_id, &input.figure_id)
        .map_err(failure)?
        .revision
        != input.expected_revision
    {
        return Err(failure(visualization::Error::Conflict));
    }
    let candidate = store
        .revision(&input.project_id, &input.figure_id, input.expected_revision)
        .map_err(failure)?;
    let elements = candidate.document["elements"]
        .as_array()
        .ok_or_else(|| invalid("missing candidate elements"))?;
    let selected = elements
        .iter()
        .find(|e| e["id"] == input.element_id && e["isDeleted"] != true)
        .ok_or_else(|| invalid("selected element is missing"))?;
    let anchor_id = if selected["type"] == "image" {
        selected["customData"]["semanticAnchorId"]
            .as_str()
            .or_else(|| selected["customData"]["semanticBadgeAnchorId"].as_str())
            .ok_or_else(|| {
                invalid("image has no component owner; use explicit binding before fitting")
            })?
    } else {
        input.element_id.as_str()
    };
    let anchor = elements
        .iter()
        .find(|e| e["id"] == anchor_id && e["isDeleted"] != true)
        .ok_or_else(|| invalid("missing component anchor"))?;
    let primary=anchor["customData"]["visualAssetElementId"].as_str().ok_or_else(||invalid("anchor has no primary illustration; replace its artwork or bind placed images before fitting"))?;
    let mut ids = vec![primary.to_owned()];
    if let Some(badges) = anchor["customData"]
        .get("visualBadgeIds")
        .filter(|v| !v.is_null())
    {
        for id in badges
            .as_array()
            .ok_or_else(|| invalid("accessory IDs must be an array"))?
        {
            ids.push(
                id.as_str()
                    .ok_or_else(|| invalid("accessory ID must be a string"))?
                    .to_owned(),
            );
        }
    }
    if ids.len() > 33 || ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len() {
        return Err(invalid(
            "requires one primary plus at most 32 distinct accessories",
        ));
    }
    let mut samples = Vec::new();
    let mut bases = Vec::new();
    for id in ids {
        let image = elements
            .iter()
            .find(|e| e["id"] == id && e["type"] == "image" && e["isDeleted"] != true)
            .ok_or_else(|| invalid("group member image is missing"))?;
        let file_id = image["fileId"]
            .as_str()
            .ok_or_else(|| invalid("group member file is missing"))?;
        let bounds = crate::fit_svg::measured_bounds(
            path.clone(),
            &crate::fit_svg::SvgFitInput {
                project_id: input.project_id.clone(),
                figure_id: input.figure_id.clone(),

                expected_revision: input.expected_revision,

                element_id: id.clone(),
                file_id: file_id.into(),
            },
        )?;
        samples.push(
            json!({"id":id,"file_id":file_id,"normalized_bounds":bounds["normalized_bounds"]}),
        );
        bases.push(json!({"element_id":id,"file_id":file_id,"bounds":bounds}));
    }
    Ok((anchor_id.to_owned(), json!(samples), json!(bases)))
}
#[async_trait]
impl ToolFunction for GroupFitTool {
    type Input = GroupFitInput;
    async fn run(&self, input: GroupFitInput) -> Result<ToolResult, ToolError> {
        if [&input.project_id, &input.figure_id, &input.element_id]
            .iter()
            .any(|v| v.trim().is_empty())
        {
            return Err(invalid(
                "group SVG preview fitting requires nonblank scope, title and element_id",
            ));
        }
        let path = self.path.clone();
        let (input, (anchor, samples, basis)) = tokio::task::spawn_blocking(move || {
            let measured = measure(path, &input)?;
            Ok::<_, ToolError>((input, measured))
        })
        .await
        .map_err(failure)??;
        let value = json!({"action":"apply","project_id":input.project_id,"figure_id":input.figure_id,"expected_revision":input.expected_revision,"operations":[{"op":"fit_illustration_group","anchor_id":anchor,"samples":samples}]});
        let mut result = FigureTool {
            path: self.path.clone(),
        }
        .run(serde_json::from_value(value).map_err(failure)?)
        .await?;
        if let agentik_sdk::types::tools::ToolResultContent::Json(value) = &mut result.content {
            value["svg_group_preview_fit_basis"] = basis;
        }
        Ok(result)
    }
}
pub(crate) fn registration(path: PathBuf) -> ToolRegistration {
    ToolRegistration::from(GroupFitTool { path })
}
