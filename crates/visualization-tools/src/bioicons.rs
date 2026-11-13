//! BioIcons search, PNG vision previews, and reviewed vector insertion/replacement.
use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::{ToolResult, tools::ToolResultBlock};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::LazyLock};
static CLIENT: LazyLock<::bioicons::Result<::bioicons::BioIconsClient>> =
    LazyLock::new(::bioicons::BioIconsClient::new);
fn error(e: impl std::fmt::Display) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(std::io::Error::other(e.to_string())),
    }
}
#[tool(
    name = "bioicons",
    description = "Search BioIcons SVG illustrations. Actions search, search_batch, preview, insert, replace, palette, recolor, parts, geometry, edit_parts. parts inspects rendered SVG paths and returns snapshot-scoped IDs, bounds and a numbered PNG; numbers identify graphical paths, never biological identities. Each bounds [x,y,width,height] already includes the path stroke and ancestor transforms in normalized SVG root coordinates; do not retrieve raw SVG merely to add stroke width. Bounds are geometric boxes, not exact clipped visible-pixel masks. Later paths may fully cover earlier paths; palette presence does not prove visible color. parts includes render_contribution status and, when completed, changed_preview_pixels_when_hidden per path: a bounded leave-one-path-out RGBA comparison on a transparent SVG root preview, excluding canvas crop/background and other objects, not path area or a visibility mask. Zero can also mean identical paint, clipping, or subpixel detail; prefer a path with actual render contribution for a visible edit, then inspect PNG. Limits return not_computed without inferring visibility. Inspect the final PNG and report occlusion or visually unmet requests rather than claiming success from palette values alone. parts also reports stroke_width in normalized SVG local units (0 means no parsed stroke definition). edit_parts stroke_width sets an absolute finite width in (0,1000], not canvas pixels; omitted preserves width, and a path without stroke also needs an explicit stroke color. Width changes keep path commands and transforms, but may enlarge painted bounds or be clipped by the original viewport/clips; inspect PNG and recheck fit/spacing before adoption. edit_parts proposes fill/stroke (#RRGGBB or none) local opacity (0..1) and translate [dx,dy] edits in normalized SVG root coordinates on specified paths, using sha256 and edits from parts. Shared masks, clips and pattern resources are not editable paths. Omitted attributes remain unchanged. Translation compensates ancestor scaling, rotation and reflection; paint follows the path, parent clipping and the SVG viewport stay fixed. Use preview and review the result; geometry returns numbered endpoint/control handles and command topology for one part_id and sha256 from parts. Use edit_parts points:[{index,position:[x,y]}] for sparse absolute root-coordinate control/anchor edits from geometry; at most 64 edited points on paths with at most 512 points. Keep topology and other points unchanged. Do not combine points and translate for the same path; make separate reviewed proposals. Shape edits keep paint coordinates and parent clips fixed. Read parts again after every SVG edit or replacement, including candidate edits, because IDs belong to the exact old file snapshot. palette reads a saved SVG image (or its semantic anchor) and returns actual #RRGGBB paints with a PNG preview. recolor uses colors mapping old_hex to new_hex to propose simultaneous exact paint replacements; preserves text glyphs, geometry, original author/license/source hash and unrelated image instances. Both require project_id, figure_id, expected_revision and element_id; neither requires network access. Read palette first, do not invent source colours. SVG text must already be outlined so recoloring cannot silently substitute its font. insert appends a standalone image at x/y; use replace to preserve an existing node identity. To move/resize the new image, use visualization revise_proposal with {op:set_image_geometry,id,x,y,width,height}, all JSON numbers. Native scale is only flip signs ±1, not fractional size; dimensions are width/height. Never use align-center on two images that belong under different stages. For multiple independent concepts use search_batch with queries:1..8 distinct nonblank strings (each at most 256 UTF-8 bytes); omit query. It returns four ranked hits per query with independent lexical match counts and search scope, or an explicit error. A shared catalogue failure stops the batch; remaining queries are not_attempted, never zero-hit results. Refine with search for more results and preview chosen assets; batching does not verify biological suitability. Search ranks exact names and whole name tokens before prefix/substring matches. match_summary counts all lexical tiers before pagination, not scientific identity. search_scope echoes the actual query and explicitly reports that scientific suitability and catalogue-wide absence have not been established. If exact_name and name_tokens are zero, partial matches such as raster for RAS may be irrelevant; refine the query or report that no suitable icon was confirmed among the queried results rather than inserting a superficially matching name. Keyword queries do not enumerate the entire catalogue: never turn zero hits or rejected lexical matches into a catalogue-wide absence claim. A later coverage caveat does not repair an earlier absolute statement. Search then inspect a PNG preview and per-icon author/license before selecting; do not infer scientific specificity from generic art. insert and replace create reviewable proposals on the exact current figure revision; they never commit directly. SVG remains a scalable vector image with provenance and source/normalized hashes; internal paths are not individual canvas objects. Unsupported raster/active SVG or unsupported text is rejected rather than silently dropped. Use visualization read/read_image to inspect saved objects and current revision. Preserve unrelated elements, scientific labels and arrow anchors. User must save local edits before agent modification. For insert, replace, palette, parts, geometry, recolor or edit_parts on a pending candidate, first use visualization read_proposal, then supply proposal_id, its exact document_sha256 and expected_revision. Inspection returns scope/proposal/hash/file identity plus a PNG from that exact candidate, without adopting it. Changes create a child candidate that preserves all previous edits and leaves the parent/formal figure untouched. On changes, explicitly provide markup_ids to retain or drop open review links; [] intentionally drops them. Omit markup_ids on read-only inspection. Recolor and part edits require no catalog/network access. Inspect the new child with visualization read_proposal_preview before adoption. Standalone image replacement keeps its image ID, viewport, rotation, flip signs, user notes and bindings. Existing crop fractions map to the new source dimensions; this is a graphical crop, not the same biological region. The new source/license and prior-source history are recorded. Inspect the candidate crop before adoption. Successful changes include candidate_context and follow_up_inputs with complete read_proposal/read_proposal_preview arguments; copy these instead of reconstructing IDs or confusing plan and figure versions. Organelle/context artwork may be inserted independently with a visible qualification; never replace a molecular node with an organelle or assign a false semanticAnchorId. Do not infer protein-specific structure or additional mechanism from generic artwork. Fresh illustration labels must remain visible within their native frame. The initial scientific-font caption may be placed above/below the artwork if its default position would clip; when no clear in-frame position can be measured, replacement fails without saving. Explicitly enlarge the frame or revise the component layout before retrying, never drop or shrink the scientific label to bypass this protection. Later replacement preserves user-positioned existing captions."
)]
pub struct Input {
    #[desc = "search | search_batch | preview | insert | replace | palette | recolor | parts | geometry | edit_parts"]
    pub action: String,
    pub query: Option<String>,
    #[desc = "Only search/search_batch: exact author name from catalogue attribution, ignoring case/outer whitespace. Restricts every query; omit to search all authors. Same author does not guarantee matching style or scientific suitability."]
    pub author: Option<String>,
    #[desc = "Only for search_batch: 1..8 distinct nonblank query strings, each at most 256 UTF-8 bytes. Omit query; each result retains its own retrieval scope and lexical counts."]
    pub queries: Option<Vec<String>>,
    pub asset_id: Option<String>,
    pub project_id: Option<String>,
    pub figure_id: Option<String>,
    pub expected_revision: Option<i64>,
    pub element_id: Option<String>,
    #[desc = "Optional pending candidate ID for insert/replace/palette/parts/geometry/recolor/edit_parts; requires its exact document_sha256. Reads inspect that candidate; writes create a child."]
    pub proposal_id: Option<String>,
    #[desc = "Exact candidate document_sha256; use with proposal_id, distinct from the SVG file sha256"]
    pub document_sha256: Option<String>,
    #[desc = "Explicit review IDs to retain/address; [] explicitly drops links; omission rejects a candidate with open linked reviews"]
    #[serde(default, deserialize_with = "crate::asset_candidate::deserialize_markup_ids")]
    pub markup_ids: Option<Vec<String>>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    #[desc = "Exact simultaneous paint replacements as an object mapping #RRGGBB to #RRGGBB; required for recolor"]
    pub colors: Option<std::collections::BTreeMap<String, String>>,
    #[desc = "Exact SVG snapshot hash from parts; required for edit_parts"]
    pub sha256: Option<String>,
    #[desc = "Path ID from parts; required for geometry"]
    pub part_id: Option<String>,
    #[desc = "1..32 path edits using part_id from parts; translate [dx,dy] uses source SVG coordinates, not canvas pixels; points uses absolute positions and indices from geometry; omitted attributes stay unchanged"]
    pub edits: Option<Vec<::bioicons::parts::Edit>>,
}
struct BioIconsTool {
    path: PathBuf,
}
#[async_trait]
impl ToolFunction for BioIconsTool {
    type Input = Input;
    async fn run(&self, input: Input) -> Result<ToolResult, ToolError> {
        if let Some(author) = &input.author {
            if !matches!(input.action.as_str(), "search" | "search_batch") || author.trim().is_empty() || author.len() > 300 {
                return Err(error("author is only supported for search/search_batch and must be a nonblank name of at most 300 UTF-8 bytes"));
            }
        }
        if input.queries.is_some() && input.action != "search_batch" {
            return Err(error("queries is supported only for search_batch; use query for search"));
        }
        crate::asset_candidate::validate_vector(
            &input.action,
            input.proposal_id.as_deref(),
            input.document_sha256.as_deref(),
            input.markup_ids.as_deref(),
        )
        .map_err(error)?;
        if matches!(
            input.action.as_str(),
            "palette" | "recolor" | "parts" | "geometry" | "edit_parts"
        ) {
            let path = self.path.clone();
            return tokio::task::spawn_blocking(move || local_svg(path, input))
                .await
                .map_err(error)?;
        }

        if input.action == "replace" && (input.x.is_some() || input.y.is_some()) {
            return Err(error(
                "replace preserves current placement; omit x/y. Use visualization revise_proposal set_image_geometry to move or resize a standalone image",
            ));
        }
        if input.action == "insert" && input.element_id.is_some() {
            return Err(error(
                "insert creates a new image at x/y; omit element_id or use replace for an existing object",
            ));
        }
        if input.action == "search_batch" {
            if input.query.is_some() {
                return Err(error("search_batch requires queries; omit singular query"));
            }
            let queries=input.queries.as_deref().ok_or_else(||error("search_batch requires queries"))?;
            crate::bioicons_batch::validate(queries).map_err(error)?;
            let client=CLIENT.as_ref().map_err(error)?;
            return Ok(ToolResult::success_json(crate::bioicons_batch::search_with_author(client,queries,input.author.as_deref()).await));
        }
        let client = CLIENT.as_ref().map_err(error)?;
        if input.action == "search" {
            let query = input.query.as_deref().ok_or_else(|| error("query required"))?;
            let page = client.search_with_author(query, 0, 12, input.author.as_deref()).await.map_err(error)?;
            return Ok(ToolResult::success_json(crate::catalog_search::with_scope(
                "BioIcons", query, json!(page),
            )));
        }
        if !matches!(input.action.as_str(), "preview" | "insert" | "replace") {
            return Err(error("unknown bioicons action"));
        }
        let asset = client
            .vector(
                input
                    .asset_id
                    .as_deref()
                    .ok_or_else(|| error("asset_id required"))?,
            )
            .await
            .map_err(error)?;
        if input.action == "preview" {
            let data = asset
                .vector
                .preview_data_url
                .split_once(',')
                .ok_or_else(|| error("invalid preview"))?
                .1;
            return Ok(ToolResult::with_blocks(vec![ToolResultBlock::text(json!({"asset":asset.asset,"source_sha256":asset.vector.source_sha256,"sha256":asset.vector.sha256,"colors":asset.vector.colors,"normalization":asset.vector.normalization}).to_string()),ToolResultBlock::image_base64("image/png",data)]));
        }
        let path = self.path.clone();
        let result = tokio::task::spawn_blocking(move || persist(path, input, asset))
            .await
            .map_err(error)??;
        Ok(ToolResult::success_json(result))
    }
}
fn local_svg(path: PathBuf, input: Input) -> Result<ToolResult, ToolError> {
    use visualization_assets::candidate::{self, Inspection, Reference, SvgEdit, SvgRefinement};
    let project = input
        .project_id
        .as_deref()
        .ok_or_else(|| error("project_id required"))?;
    let figure = input
        .figure_id
        .as_deref()
        .ok_or_else(|| error("figure_id required"))?;
    let revision = input
        .expected_revision
        .ok_or_else(|| error("expected_revision required"))?;
    let target = input
        .element_id
        .as_deref()
        .ok_or_else(|| error("element_id required"))?;
    let reference = Reference::optional(
        input.proposal_id.as_deref(),
        input.document_sha256.as_deref(),
    )
    .map_err(error)?;
    let store = visualization::Store::open(path).map_err(error)?
        .with_linear_candidate_revisions(true);
    if matches!(input.action.as_str(), "palette" | "parts" | "geometry") {
        let kind = match input.action.as_str() {
            "palette" => Inspection::Palette,
            "parts" => Inspection::Parts,
            _ => Inspection::Geometry {
                sha256: input
                    .sha256
                    .as_deref()
                    .ok_or_else(|| error("sha256 required"))?,
                part_id: input
                    .part_id
                    .as_deref()
                    .ok_or_else(|| error("part_id required"))?,
            },
        };
        let mut value =
            candidate::inspect_svg(&store, project, figure, revision, reference, target, kind)
                .map_err(error)?;
        let image_key = if input.action == "palette" {
            "previewDataURL"
        } else {
            "annotatedDataURL"
        };
        let url = value
            .as_object_mut()
            .unwrap()
            .remove(image_key)
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| error("preview unavailable"))?;
        value.as_object_mut().unwrap().remove("previewDataURL");
        let data = url
            .split_once(',')
            .ok_or_else(|| error("invalid preview"))?
            .1;
        return Ok(ToolResult::with_blocks(vec![
            ToolResultBlock::text(value.to_string()),
            ToolResultBlock::image_base64("image/png", data),
        ]));
    }
    let edit = if input.action == "edit_parts" {
        SvgEdit::Parts {
            sha256: input
                .sha256
                .as_deref()
                .ok_or_else(|| error("sha256 required"))?,
            edits: input
                .edits
                .as_deref()
                .ok_or_else(|| error("edits required"))?,
        }
    } else {
        SvgEdit::Recolor(
            input
                .colors
                .as_ref()
                .ok_or_else(|| error("colors required"))?,
        )
    };
    let proposal = candidate::refine_svg(
        &store,
        project,
        figure,
        revision,
        SvgRefinement {
            reference,
            target,
            edit,
            markup_ids: input.markup_ids.as_deref(),
        },
    )
    .map_err(error)?;
    Ok(ToolResult::success_json(
        crate::follow_up::proposal_result(project, &proposal, Some(target)),
    ))
}

fn persist(
    path: PathBuf,
    input: Input,
    asset: ::bioicons::vector::VectorAsset,
) -> Result<Value, ToolError> {
    let project = input
        .project_id
        .ok_or_else(|| error("project_id required"))?;
    let figure = input.figure_id.ok_or_else(|| error("figure_id required"))?;
    let revision = input
        .expected_revision
        .ok_or_else(|| error("expected_revision required"))?;
    let store = visualization::Store::open(path).map_err(error)?
        .with_linear_candidate_revisions(true);
    if store.get(&project, &figure).map_err(error)?.revision != revision {
        return Err(error(visualization::Error::Conflict));
    }

    let summary = format!(
        "{} BioIcons {}",
        if input.action == "replace" {
            "Replace with"
        } else {
            "Insert"
        },
        asset.asset.name
    );
    let image = visualization::image::Image {
        file_id: &asset.vector.sha256,
        data_url: &asset.vector.data_url,
        width: asset.vector.width,
        height: asset.vector.height,
        provenance: json!({"source":"BioIcons","asset":asset.asset,"sha256":asset.vector.sha256,"source_sha256":asset.vector.source_sha256,"colors":asset.vector.colors,"normalization":asset.vector.normalization}),
    };
    if let (Some(parent_id), Some(hash)) = (&input.proposal_id, &input.document_sha256) {
        let (proposal, id) = if input.action == "insert" {
            visualization_assets::candidate::insert(
                &store,
                &project,
                &figure,
                revision,
                visualization_assets::candidate::Insertion {
                    reference: visualization_assets::candidate::Reference {
                        proposal_id: parent_id,
                        document_sha256: hash,
                    },
                    image,
                    x: input.x.unwrap_or(0.),
                    y: input.y.unwrap_or(0.),
                    summary: &summary,
                    markup_ids: input.markup_ids.as_deref(),
                },
            )
        } else {
            visualization_assets::candidate::replace(
                &store,
                &project,
                &figure,
                revision,
                visualization_assets::candidate::Replacement {
                    parent_id,
                    parent_document_sha256: hash,
                    target: input
                        .element_id
                        .as_deref()
                        .ok_or_else(|| error("element_id required"))?,
                    image,
                    summary: &summary,
                    markup_ids: input.markup_ids.as_deref(),
                },
            )
        }
        .map_err(error)?;
        return Ok(
            crate::follow_up::proposal_result(&project, &proposal, Some(&id)),
        );
    }
    let scene = store.revision(&project, &figure, revision).map_err(error)?;
    let (document, id) = if input.action == "replace" {
        visualization::image::illustrate(
            &scene.document,
            input
                .element_id
                .as_deref()
                .ok_or_else(|| error("element_id required"))?,
            image,
        )
    } else {
        visualization::image::insert(
            &scene.document,
            image,
            input.x.unwrap_or(0.),
            input.y.unwrap_or(0.),
        )
    }
    .map_err(error)?;
    let proposal = store
        .propose_with_markups(
            &project,
            &figure,
            revision,
            &summary,
            &document,
            input.markup_ids.as_deref().unwrap_or(&[]),
        )
        .map_err(error)?;
    Ok(
        crate::follow_up::proposal_result(&project, &proposal, Some(&id)),
    )
}
pub fn registrations(path: PathBuf) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(BioIconsTool { path })]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn svg_replacement_is_reviewed_preserves_provenance_and_returns_png_vision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = visualization::Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let plan=serde_json::from_value(json!({"message":"Conceptual relation","audience":"Researchers","layout":"horizontal","nodes":[{"id":"a","label":"Receptor"},{"id":"b","label":"Response"}],"edges":[{"from":"a","to":"b","label":"explicit relation"}]})).unwrap();
        let scene = visualization::sketch::compile(&plan).unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &scene)
            .unwrap();
        let vector=::bioicons::vector::normalize(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 80 100'><path d='M10 10h60v80H10z' fill='#0088cc'/></svg>").unwrap();
        let asset = ::bioicons::Asset {
            id: "fixture".into(),
            name: "Generic receptor".into(),
            category: "Receptors".into(),
            license: "cc-by-4.0".into(),
            license_url: Some("https://creativecommons.org/licenses/by/4.0/".into()),
            author: "Fixture Author".into(),
            author_url: None,
            source_url: "https://example.org/asset".into(),
            svg_url: "https://example.org/asset.svg".into(),
            catalog_sha256: "catalog-fixture".into(),
        };
        let candidate_asset = serde_json::to_value(&asset).unwrap();
        let result = persist(
            path.clone(),
            Input {
                action: "replace".into(),
                author: None,
                query: None,
                queries: None,
                asset_id: Some("fixture".into()),
                project_id: Some("p".into()),
                figure_id: Some(figure.id.clone()),
                expected_revision: Some(1),
                element_id: Some("node-0".into()),
                proposal_id: None,
                document_sha256: None,
                markup_ids: None,
                x: None,
                y: None,
                colors: None,
                sha256: None,
                part_id: None,
                edits: None,
            },
            ::bioicons::vector::VectorAsset { asset, vector },
        )
        .unwrap();
        let follow=serde_json::to_value(crate::registrations(path.clone()).remove(0).implementation.execute(result["follow_up_inputs"]["read_proposal"].clone()).await.unwrap()).unwrap();
        assert_eq!(follow["content"]["proposal"]["id"], result["proposal_id"]);
        assert_eq!(follow["content"]["proposal"]["document_sha256"], result["document_sha256"]);
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        let proposal = store
            .proposals("p", &figure.id)
            .unwrap()
            .into_iter()
            .find(|p| p.id == result["proposal_id"])
            .unwrap();
        let image = proposal.document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "image")
            .unwrap();
        let id = image["id"].as_str().unwrap().to_string();
        assert_eq!(image["customData"]["asset"]["license"], "cc-by-4.0");
        assert_eq!(
            proposal.document["files"][image["fileId"].as_str().unwrap()]["mimeType"],
            "image/svg+xml"
        );
        assert_eq!(
            proposal.document["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == "node-1"),
            scene["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == "node-1")
        );
        let next_input: Input = serde_json::from_value(json!({"action":"replace","project_id":"p","figure_id":figure.id,"expected_revision":1,"element_id":"node-1","proposal_id":proposal.id,"document_sha256":proposal.document_sha256,"markup_ids":[]})).unwrap();
        let next_vector = ::bioicons::vector::normalize(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 80 100'><path d='M10 10h60v80H10z' fill='#0088cc'/></svg>").unwrap();
        let child_result = persist(
            path.clone(),
            next_input,
            ::bioicons::vector::VectorAsset {
                asset: serde_json::from_value(candidate_asset.clone()).unwrap(),
                vector: next_vector,
            },
        )
        .unwrap();
        let child = store
            .proposal(
                "p",
                &figure.id,
                child_result["proposal_id"].as_str().unwrap(),
            )
            .unwrap();
        assert_eq!(
            child.parent_proposal_id.as_deref(),
            Some(proposal.id.as_str())
        );
        assert_eq!(
            child.document["elements"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["type"] == "image")
                .count(),
            2
        );
        assert_eq!(
            store
                .proposal("p", &figure.id, &proposal.id)
                .unwrap()
                .document,
            proposal.document
        );
        assert_eq!(store.revision("p", &figure.id, 1).unwrap().document, scene);
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        let inserted: Input = serde_json::from_value(json!({"action":"insert","project_id":"p","figure_id":figure.id,"expected_revision":1,"proposal_id":child.id,"document_sha256":child.document_sha256,"markup_ids":[],"x":700,"y":440})).unwrap();
        let insert_result = persist(path.clone(), inserted, ::bioicons::vector::VectorAsset {
            asset: serde_json::from_value(candidate_asset.clone()).unwrap(),
            vector: ::bioicons::vector::normalize(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 80 100'><path d='M10 10h60v80H10z' fill='#0088cc'/></svg>").unwrap(),
        }).unwrap();
        let appended = store
            .proposal(
                "p",
                &figure.id,
                insert_result["proposal_id"].as_str().unwrap(),
            )
            .unwrap();
        assert_eq!(
            appended.parent_proposal_id.as_deref(),
            Some(child.id.as_str())
        );
        assert_eq!(
            &appended.document["elements"].as_array().unwrap()
                [..child.document["elements"].as_array().unwrap().len()],
            child.document["elements"].as_array().unwrap()
        );
        let new_image = appended.document["elements"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(new_image["x"], 700.);
        assert_eq!(new_image["y"], 440.);
        assert_eq!(new_image["customData"]["asset"]["license"], "cc-by-4.0");
        assert_eq!(appended.document["files"], child.document["files"]);
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        store
            .accept_proposal("p", &figure.id, &proposal.id)
            .unwrap();
        let tool = crate::registrations(path.clone()).remove(0);
        let result=serde_json::to_value(tool.implementation.execute(json!({"action":"read_image","project_id":"p","figure_id":figure.id,"element_id":id,"expected_revision":2})).await.unwrap()).unwrap();
        assert_eq!(result["content"][1]["source"]["media_type"], "image/png");
        assert!(
            result["content"][1]["source"]["data"]
                .as_str()
                .unwrap()
                .starts_with("iVBOR")
        );
        let local = registrations(path).remove(0);
        let palette=serde_json::to_value(local.implementation.execute(json!({"action":"palette","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0"})).await.unwrap()).unwrap();
        assert_eq!(palette["content"][1]["source"]["media_type"], "image/png");
        let caption: Value =
            serde_json::from_str(palette["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(caption["colors"], json!(["#0088cc"]));
        assert!(!caption.to_string().contains("data:image"));
        let recolored=serde_json::to_value(local.implementation.execute(json!({"action":"recolor","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0","colors":{"#0088cc":"#00aa88"}})).await.unwrap()).unwrap();
        assert_eq!(recolored["content"]["status"], "pending");
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 2);
        let parts=serde_json::to_value(local.implementation.execute(json!({"action":"parts","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0"})).await.unwrap()).unwrap();
        assert_eq!(parts["content"][1]["source"]["media_type"], "image/png");
        let detail: Value =
            serde_json::from_str(parts["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(!detail.to_string().contains("data:image"));
        let proposal=serde_json::to_value(local.implementation.execute(json!({"action":"edit_parts","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0","sha256":detail["sha256"],"edits":[{"part_id":detail["parts"][0]["part_id"],"fill":"#aa8844","translate":[2,3]}]})).await.unwrap()).unwrap();
        assert_eq!(proposal["content"]["status"], "pending");
        let geometry=serde_json::to_value(local.implementation.execute(json!({"action":"geometry","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0","sha256":detail["sha256"],"part_id":detail["parts"][0]["part_id"]})).await.unwrap()).unwrap();
        assert_eq!(geometry["content"][1]["source"]["media_type"], "image/png");
        let handles: Value =
            serde_json::from_str(geometry["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(handles["revision"], 2);
        assert!(!handles.to_string().contains("data:image"));
        let point = handles["points"][0]["position"].as_array().unwrap();
        let result=serde_json::to_value(local.implementation.execute(json!({"action":"edit_parts","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0","sha256":detail["sha256"],"edits":[{"part_id":detail["parts"][0]["part_id"],"points":[{"index":0,"position":[point[0].as_f64().unwrap()+2.,point[1].as_f64().unwrap()+3.]}]}]})).await.unwrap()).unwrap();
        assert_eq!(result["content"]["status"], "pending");
        let count = store.proposals("p", &figure.id).unwrap().len();
        assert!(local.implementation.execute(json!({"action":"recolor","project_id":"p","figure_id":figure.id,"expected_revision":2,"element_id":"node-0","colors":{"#123456":"#00aa88"}})).await.is_err());
        assert_eq!(store.proposals("p", &figure.id).unwrap().len(), count);
    }
}
