//! Exact-candidate, backend-measured SVG group fitting without handwritten sample arrays.
use super::*;
#[tool(
    name = "visualization_fit_svg_group_preview",
    description = "Fit an entire bound illustration group using backend-derived nonzero-alpha SVG preview rectangles for the primary and ALL recorded accessories. Read the exact candidate and review artwork first. Supply an anchor or owned image element_id, pending proposal ID/hash and current revision; do not supply samples or normalized_bounds. Reads exact file IDs and full membership from the candidate and creates one protected child with fit_illustration_group. Every image must be an uncropped unrotated nonempty SVG; raster/cropped/unsupported members need explicitly reviewed group samples instead. Bounds include one preview-pixel padding, backgrounds and decorative paint; they are layout measurements, not component identity, topology or stoichiometry detection. Files, images, labels, source records and scientific metadata stay unchanged; only anchor/fit records and supported connectors follow the measured union. Locked/invalid ownership, stale scope or unsupported connector geometry reject atomically. Inspect the child PNG; never adopts."
)]
#[serde(deny_unknown_fields)]
pub struct GroupFitInput {
    pub project_id: String,
    pub figure_id: String,
    pub proposal_id: String,
    pub document_sha256: String,
    pub expected_revision: i64,
    pub title: String,
    pub element_id: String,
    #[serde(
        default,
        deserialize_with = "crate::asset_candidate::deserialize_markup_ids"
    )]
    pub markup_ids: Option<Vec<String>>,
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
        .proposal(&input.project_id, &input.figure_id, &input.proposal_id)
        .map_err(failure)?;
    if candidate.status != "pending"
        || candidate.base_revision != input.expected_revision
        || candidate.document_sha256 != input.document_sha256
    {
        return Err(invalid(
            "group preview fitting requires the exact pending candidate hash and current revision",
        ));
    }
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
                proposal_id: input.proposal_id.clone(),
                document_sha256: input.document_sha256.clone(),
                expected_revision: input.expected_revision,
                title: input.title.clone(),
                element_id: id.clone(),
                file_id: file_id.into(),
                markup_ids: None,
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
        if [
            &input.project_id,
            &input.figure_id,
            &input.proposal_id,
            &input.document_sha256,
            &input.title,
            &input.element_id,
        ]
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
        let value = json!({"action":"revise_proposal","project_id":input.project_id,"figure_id":input.figure_id,"proposal_id":input.proposal_id,"document_sha256":input.document_sha256,"expected_revision":input.expected_revision,"title":input.title,"operations":[{"op":"fit_illustration_group","anchor_id":anchor,"samples":samples}],"markup_ids":input.markup_ids});
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
