//! Agent access to the same project figures edited in the GUI.
mod bioart;
mod bioicons;
use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::{ToolResult, tools::ToolResultBlock};
use async_trait::async_trait;
pub use bioart::registrations as bioart_registrations;
pub use bioicons::registrations as bioicons_registrations;
use serde_json::{Value, json};
use std::path::PathBuf;
use visualization::{Store, service::VisualizationService};

// Machine-readable schemas mirror the actual domain types; the Rust adapter
// retains Value to dispatch create, sketch and partial planning documents.
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ActionSchema {
    Projects,
    List,
    Read,
    ReadImage,
    ImageInfo,
    InsetStatus,
    RefreshInset,
    Inset,
    ReadPreview,
    ReadProposalPreview,
    ReadProposal,
    ReviseProposal,
    ReadMarkup,
    ListMarkups,
    ListProposals,
    ReadPlan,
    SavePlan,
    SketchPlan,
    Create,
    Sketch,
    Propose,
    Apply,
    Arrange,
}
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[serde(untagged)]
enum DocumentSchema {
    Brief(visualization::planning::Brief),
    Scene(SceneSchema),
}
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
struct SceneSchema {
    elements: Vec<Value>,
    #[serde(rename = "type")]
    kind: Option<String>,
    version: Option<u32>,
    files: Option<std::collections::BTreeMap<String, Value>>,
    #[serde(rename = "appState")]
    app_state: Option<Value>,
}
#[tool(
    name = "visualization",
    description = "Manage editable scientific figures shared with the GUI. Actions: projects, list, read, image_info, inset, inset_status, refresh_inset, read_image, read_preview, read_proposal_preview, read_proposal, revise_proposal, read_markup, list_markups, list_proposals, read_plan, save_plan, sketch_plan, create, sketch, propose, apply, arrange. For comparison or step panels, declare panels [{id,title,column,row}] and assign every node.panel to a declared ID. Panels compile to native editable frames with independent internal layouts; explicit cross-panel edges use editable scene-level connectors routed around nodes and titles. Updating a frame x/y moves its children and adjusts external connector endpoints; removing a frame detaches and preserves its contents. Use read_plan/save_plan to maintain a versioned scientific brief with message, audience, nodes, edges, assumptions, relationship_note and questions [{id,question,answer}]. Save incomplete briefs to persist focused clarification questions; never invent missing scientific relationships. Resolve the questions and review the structural plan with the user before sketch_plan. sketch_plan only generates a proposal from a ready brief at the exact planning_version and expected_revision. Once an existing figure has a saved brief, use sketch_plan rather than direct sketch. For sketch, supply figure_id and expected_revision to propose a new sketch in the active figure; omit figure_id only when creating a separate figure. Prefer propose for changes that should be previewed in the GUI before acceptance. When a proposed change addresses review markup, pass its IDs in markup_ids after reading and reconciling those reviews; this links the candidate to those notes without closing them. The user can explicitly close linked notes when adopting the proposal. read omits embedded image bytes; use read_image with element_id to view a reference image as a vision tool result. read_proposal with proposal_id and expected_revision returns an immutable candidate scene with image bytes omitted, its full stored document_sha256 and parent lineage. revise_proposal creates a new child from that candidate (not the saved scene), retaining previous edits and files; pass proposal_id, document_sha256, expected_revision, title, and exactly one operations or arrangement. Parent must be pending at the current figure base revision; rejected/accepted or stale parents must be reconciled with the saved scene. Parent stays immutable and is not automatically adopted/rejected. If the parent has open review links, explicitly pass markup_ids (including [] to drop) for reviews still addressed by the child; do not silently inherit resolved or no-longer-addressed notes. read_proposal_preview with proposal_id and expected_revision requests an independent browser render of that exact immutable candidate, then waits briefly for the authenticated workspace renderer (the figure editor need not be open) and returns a vision block. It never adopts the proposal. Inspect scope, document_sha256, base_revision, current_revision and proposal_status; if base_is_current is false, reconcile with the current saved scene. If no renderer is open or a render fails, it reports an error and never substitutes the saved-canvas preview. After fixing render resources, retry_preview:true explicitly requeues a failed render. read_preview returns the full canvas PNG for the current saved revision when the GUI has captured one; never fall back to a stale preview. read_markup with markup_id returns saved review ink over its exact historical canvas as a vision block. The result declares at_revision and current_revision; if they differ, reconcile the historical review with the current scene before proposing edits. read also returns proposal_summary (current/stale pending counts and latest metadata only). Use list_proposals with before and limit (1..100, default 20) to discover pending candidate metadata without scene/image bytes; repeat next_before until null. Read a chosen candidate with read_proposal and the current expected_revision before revising it; listing is not scientific validation or acceptance. Read returns recent markup summaries, markups_pagination.next_before and object-linked review annotations as well as the scene. Use list_markups with before, limit (1..100) and optional status open/resolved to retrieve older reviews; repeat next_before until null. Filter status open to find outstanding reviews even when newer records are resolved; use these notes to interpret selected markup. Use read before apply/propose; use expected_revision from read, never guess it. Use {op:reroute,id} to recompute a generated cross-panel arrow after moving components, from its current bindings and current label size; preserve user text, styles and evidence. Prefer propose for review. Detached endpoints or rotated bound nodes must be resolved first. apply operations are JSON objects: {op:add,element:{id,...}}, {op:update,id,changes:{...}}, {op:remove,id}. Removing an illustrated semantic anchor also removes its owned image and labels: do not remove those IDs again. Connected arrows survive with cleared bindings. Ordinary shape or arrow removal leaves its bound text detached. When the user requests removing a component and its relationships, explicitly remove the affected arrows and their text IDs, preserve unrelated objects, and never invent a replacement relationship. Labels, arrows and icons must remain separate editable elements. Position updates move owned labels/images and ordinary straight connector endpoints. For rotated or elbowed connectors, supply geometry edits explicitly. Preserve unrelated objects, do not flatten the figure into a bitmap. image_info requires element_id and expected_revision and returns actual file dimensions, crop and provenance without image bytes. inset requires expected_revision and inset {element_id,region:{x,y,width,height},x,y,max_size,title}; region is fractional in the current visible image after flips, before rotation; x/y position the new detail panel. Use image_info and read_image first. This creates a pending proposal with editable image crop, region box and neutral callout, preserving original files and licensing. inset_status with element_id (inset anchor/image) and expected_revision compares the current source against its recorded historical snapshot; changed does not merely mean a later figure revision. refresh_inset with the same arguments proposes a refresh in the existing panel, keeping object IDs, panel/title, styles and unrelated objects. The saved fractional region is applied to the current source viewport; replacing the source may change its scientific content. Read status and images before refreshing; if detail_edited is true, local file/crop edits will be replaced only with discard_detail_edits:true. Missing members, ambiguous ownership or rotated panels must be resolved first. To delete a whole inset, remove its anchor_id once; this also removes its owned image/title and matching region/callout decorations, preserving the source image and other insets. Do not repeat cascaded IDs. Insets are frozen snapshots tied to the source revision; later source edits do not refresh them or move the region annotation automatically. Read the saved scene and its group membership before arrange. arrange requires arrangement and expected_revision and proposes alignment or equal edge-gap distribution of existing components without changing their relationships. Native groups, semantic anchors and owned labels/images move together; overlapping selections collapse to one unit. Distribution needs at least three independent units, uses equal edge gaps when space permits and the editor's anchored-center fallback when the span is too small. Exclude inset region/callout annotations. Native ink and unrounded zero-roughness polylines can be part of arranged components. A connector inside a selected component must translate rigidly with its bound endpoints; keep between-component connectors outside those groups. Rotated arrow-bound text and rough/curved/elbowed bounds still require native geometry. Locked members and unsupported independently moved curved/rotated/elbowed connectors must be resolved before arranging. Review crossings/overlaps visually; arrangement does not automatically reroute arrows. For long scientific labels, use {op:set_wrapped_text,id,text} to update text/originalText, wrapping, height and position together with pinned fontFamily 9 metrics. It preserves containers, other objects, styles, files and binding identities; uses native container widths and rejects overflowing shape/frame text rather than clipping or silently resizing containers. Optional max_width applies only to unbound text. Requires unrotated unlocked text; complex curves/elbows, unsupported glyphs/fonts and ambiguous bindings need native editing. Always inspect the candidate for collisions; wrapping is not global layout. For a local change of an existing relation, use {op:set_relation_direction,id,direction,inhibitory:false} in propose/revise_proposal. It updates arrowheads and explicit metadata together, preserving geometry, bindings, labels, evidence and original plan snapshots. Locked/deleted arrows, inset callouts, annotation arrows and opaque metadata are rejected. inhibitory must be explicitly supplied (true only for forward inhibition); do not silently discard inhibition when changing direction. Read the current relation and scientific intent before changing it; this does not rewrite the saved scientific brief. Edges may explicitly declare direction:forward (default), bidirectional, or undirected. Direction controls native arrowheads, including cross-panel connectors. inhibitory:true is supported only with forward direction; resolve conflicting scientific intent instead of discarding it. from/to identify layout endpoints even for undirected relations and do not establish causality. Scientific claims require user review. GUI unsaved changes are not included; ask the user to save before editing them."
)]
pub struct FigureInput {
    #[desc = "For revise_proposal: copy the parent candidate document_sha256 from read_proposal or read_proposal_preview; this is not its PNG hash. Requires proposal_id, expected_revision and title; exactly one operations or arrangement."]
    pub document_sha256: Option<String>,
    #[desc = "For read_proposal/read_proposal_preview/revise_proposal: copy proposal_id from the proposal result ID. Requires figure_id and expected_revision for the current figure head; image is the immutable candidate, not the saved canvas."]
    pub proposal_id: Option<String>,
    #[desc = "For read_proposal_preview only: explicitly requeue a failed browser render after fixing its resources. Ready images never change."]
    pub retry_preview: Option<bool>,
    #[desc = "For arrange: {element_ids:[existing IDs],mode:left|center|right|top|middle|bottom|distribute_horizontal|distribute_vertical}. Requires expected_revision. Expands native groups and owned images/labels; frames keep their contents. Only creates a pending proposal."]
    pub arrangement: Option<visualization::arrange::Spec>,
    #[desc = "For refresh_inset only: explicitly permit replacing local file/crop edits reported by inset_status. Defaults false; proposal remains pending for user review."]
    pub discard_detail_edits: Option<bool>,
    #[desc = "For inset only: source image ID, fractional visible region, new panel x/y, max_size (32..2048) and title (1..80 characters). Frozen revision-bound snapshot; never invent image dimensions."]
    pub inset: Option<visualization::inset::Spec>,
    #[desc = "projects | list | read | image_info | inset | inset_status | refresh_inset | read_image | read_preview | read_proposal_preview | read_proposal | revise_proposal | read_markup | list_markups | list_proposals | read_plan | save_plan | sketch_plan | create | sketch | propose | apply | arrange"]
    #[schemars(with = "ActionSchema")]
    pub action: String,
    #[desc = "Opaque project ID: copy verbatim from the active tab context or projects. Never compose or guess IDs."]
    pub project_id: Option<String>,
    #[desc = "Opaque figure ID: copy verbatim from figure_id in the active tab context or list. Do not reuse any part of project_id."]
    pub figure_id: Option<String>,
    #[desc = "Image object ID from read; required for read_image"]
    pub element_id: Option<String>,
    #[desc = "Review markup ID from read or the user handoff; required for read_markup"]
    pub markup_id: Option<String>,
    #[desc = "For propose/arrange/revise_proposal: IDs of open review markups actually addressed by these edits. Read each with read_markup first; include only addressed reviews, including historical reviews reconciled with the current scene. Does not resolve notes; the user chooses whether to close them when accepting the proposal."]
    pub markup_ids: Option<Vec<String>>,
    #[desc = "list_markups/list_proposals cursor: copy next_before from the previous page; omit for the newest page"]
    pub before: Option<String>,
    #[desc = "list_markups status filter: open or resolved; omit for all records"]
    pub status: Option<String>,
    #[desc = "list_markups/list_proposals page size, 1..100 (default 50/20)"]
    pub limit: Option<u32>,
    #[desc = "Expected planning version for save_plan/sketch_plan; use zero to create a brief, otherwise the version from read_plan"]
    pub planning_version: Option<i64>,
    #[desc = "Required for create/sketch; proposal summary required for propose and sketch_plan"]
    pub title: Option<String>,
    #[desc = "For save_plan: partial scientific brief {message,audience,layout,nodes,edges,assumptions,relationship_note,questions:[{id,question,answer}]}; defaults allow missing facts to remain explicit issues. For create: Excalidraw scene JSON. For sketch: plan {message,audience,layout:grid|horizontal|vertical,nodes:[{id,label,column,row,shape:rectangle|ellipse,evidence:[]}],edges:[{from,to,label,inhibitory,evidence:[]}],assumptions:[]}. Choose layout grid (default), horizontal, or vertical. Grid requires unique column/row positions; automatic horizontal/vertical layouts require omitting column and row and derive layers from explicit relations, including feedback routes. Node input order resolves cycle layout ties. Long labels wrap and node heights adapt in automatic layouts. Sketch compiles to separate editable shapes, labels and arrows."]
    #[schemars(with = "Option<DocumentSchema>")]
    pub document: Option<Value>,
    #[desc = "Exact current revision returned by read; required for edits, save_plan and sketch_plan"]
    pub expected_revision: Option<i64>,
    #[desc = "Array of add/update/remove/reroute/set_relation_direction/set_wrapped_text operations; required for apply. Updates are shallow merges and cannot change IDs."]
    #[schemars(with = "Option<Vec<visualization::scene::Operation>>")]
    pub operations: Option<Value>,
}
struct FigureTool {
    path: PathBuf,
}
fn failure(error: impl std::error::Error + Send + Sync + 'static) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}
fn invalid(message: &str) -> ToolError {
    failure(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message,
    ))
}
fn required(value: Option<String>, name: &str) -> Result<String, ToolError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid(name))
}
fn execute(path: PathBuf, input: FigureInput) -> Result<Value, ToolError> {
    let mut service = VisualizationService::new(Store::open(path).map_err(failure)?);
    if !matches!(
        input.action.as_str(),
        "propose" | "arrange" | "revise_proposal"
    ) && input.markup_ids.as_ref().is_some_and(|ids| !ids.is_empty())
    {
        return Err(invalid(
            "markup_ids is only supported for propose, arrange or revise_proposal; review resolution requires user acceptance",
        ));
    }
    if input.action == "projects" {
        return Ok(json!(service.store().projects().map_err(failure)?));
    }
    let project = required(input.project_id, "project_id is required")?;
    if input.action == "list" {
        return Ok(json!(service.store().list(&project).map_err(failure)?));
    }
    if input.action == "create" || input.action == "sketch" {
        let title = required(input.title, "title is required")?;
        let document = input
            .document
            .ok_or_else(|| invalid("document is required"))?;
        let document = if input.action == "sketch" {
            let plan: visualization::sketch::Plan =
                serde_json::from_value(document).map_err(failure)?;
            visualization::sketch::compile(&plan).map_err(failure)?
        } else {
            document
        };
        visualization::scene::apply(&document, &[]).map_err(failure)?;
        if let Some(id) = input.figure_id {
            if service
                .store()
                .planning(&project, &id)
                .map_err(failure)?
                .is_some()
            {
                return Err(invalid(
                    "This figure has a saved scientific brief; use read_plan/save_plan/sketch_plan to preserve its clarification history",
                ));
            }
            if input.action != "sketch" {
                return Err(invalid(
                    "create makes a separate figure; use sketch or propose for an existing figure",
                ));
            }
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required for an existing figure"))?;
            let mut proposal = json!(
                service
                    .store()
                    .propose(&project, &id, revision, &title, &document)
                    .map_err(failure)?
            );
            proposal.as_object_mut().unwrap().remove("document");
            proposal["figure_id"] = json!(id);
            return Ok(proposal);
        }
        return Ok(json!(
            service
                .store_mut()
                .create(&project, &title, "excalidraw/v2", &document)
                .map_err(failure)?
        ));
    }
    let id = required(input.figure_id, "figure_id is required")?;
    match input.action.as_str() {
        "read_plan" => Ok(json!(
            service.store().planning(&project, &id).map_err(failure)?
        )),
        "save_plan" => {
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let version = input
                .planning_version
                .ok_or_else(|| invalid("planning_version is required (zero for a new brief)"))?;
            let brief: visualization::planning::Brief = serde_json::from_value(
                input
                    .document
                    .ok_or_else(|| invalid("document is required"))?,
            )
            .map_err(failure)?;
            Ok(json!(
                service
                    .store_mut()
                    .save_planning(&project, &id, revision, version, &brief)
                    .map_err(failure)?
            ))
        }
        "sketch_plan" => {
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let version = input
                .planning_version
                .ok_or_else(|| invalid("planning_version is required"))?;
            let summary = required(input.title, "title is required for the planning proposal")?;
            let mut proposal = json!(
                service
                    .store_mut()
                    .planned_proposal(&project, &id, revision, version, &summary)
                    .map_err(failure)?
            );
            proposal.as_object_mut().unwrap().remove("document");
            Ok(proposal)
        }

        "list_proposals" => {
            if input.status.is_some() {
                return Err(invalid(
                    "list_proposals lists pending candidates; status is not supported",
                ));
            }
            let metadata = service.store().get(&project, &id).map_err(failure)?;
            if input
                .expected_revision
                .is_some_and(|revision| revision != metadata.revision)
            {
                return Err(failure(visualization::Error::Conflict));
            }
            let page = service
                .store()
                .proposal_index(
                    &project,
                    &id,
                    input.before.as_deref(),
                    input.limit.unwrap_or(20) as usize,
                )
                .map_err(failure)?;
            Ok(
                json!({"current_revision":metadata.revision,"status":"pending","items":page.items,"next_before":page.next_before,"image_data_omitted":true}),
            )
        }
        "list_markups" => Ok(json!(
            service
                .store()
                .markups_page(
                    &project,
                    &id,
                    input.before.as_deref(),
                    input.status.as_deref(),
                    input.limit.unwrap_or(50)
                )
                .map_err(failure)?
        )),
        "read" => {
            let metadata = service.store().get(&project, &id).map_err(failure)?;
            let mut scene = service
                .store()
                .revision(&project, &id, metadata.revision)
                .map_err(failure)?;
            if let Some(files) = scene
                .document
                .get_mut("files")
                .and_then(Value::as_object_mut)
            {
                for file in files.values_mut() {
                    if let Some(file) = file.as_object_mut() {
                        file.remove("dataURL");
                    }
                }
            }
            let markups = service
                .store()
                .markups_page(&project, &id, None, None, 50)
                .map_err(failure)?;
            let planning=service.store().planning(&project,&id).map_err(failure)?.map(|p|json!({"version":p.version,"based_on_revision":p.based_on_revision,"status":p.status,"issue_count":p.issues.len(),"proposal_id":p.proposal_id}));
            Ok(
                json!({"metadata":metadata,"scene":scene,"image_data_omitted":true,"proposal_summary":service.store().pending_proposal_summary(&project,&id).map_err(failure)?,"annotations":service.store().annotations(&project,&id).map_err(failure)?,"markups":markups.items,"markups_pagination":{"next_before":markups.next_before},"planning":planning}),
            )
        }
        "read_markup" => {
            let metadata = service.store().get(&project, &id).map_err(failure)?;
            if input
                .expected_revision
                .is_some_and(|revision| revision != metadata.revision)
            {
                return Err(failure(visualization::Error::Conflict));
            }
            let markup_id = required(input.markup_id, "markup_id is required")?;
            let mut markup = json!(
                service
                    .store()
                    .markup(&project, &id, &markup_id)
                    .map_err(failure)?
            );
            markup["current_revision"] = json!(metadata.revision);
            markup["historical"] = json!(markup["at_revision"] != metadata.revision);
            Ok(markup)
        }
        "read_preview" => {
            let metadata = service.store().get(&project, &id).map_err(failure)?;
            if input
                .expected_revision
                .is_some_and(|revision| revision != metadata.revision)
            {
                return Err(failure(visualization::Error::Conflict));
            }
            Ok(json!(
                service
                    .store()
                    .preview(&project, &id, metadata.revision)
                    .map_err(|error| match error {
                        visualization::Error::NotFound => invalid("No preview for the current revision; ask the user to save and hand off from the canvas"),
                        other => failure(other),
                    })?
            ))
        }
        "arrange" => {
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let spec = input
                .arrangement
                .ok_or_else(|| invalid("arrangement is required"))?;
            let scene = service.current(&project, &id).map_err(failure)?;
            if scene.revision != revision {
                return Err(failure(visualization::Error::Conflict));
            }
            let next = visualization::arrange::compile(&scene.document, &spec).map_err(failure)?;
            let mut proposal = json!(
                service
                    .store()
                    .propose_with_markups(
                        &project,
                        &id,
                        revision,
                        "整理所选组件的对齐与间距",
                        &next,
                        input.markup_ids.as_deref().unwrap_or(&[])
                    )
                    .map_err(failure)?
            );
            proposal.as_object_mut().unwrap().remove("document");
            Ok(proposal)
        }
        "inset_status" | "refresh_inset" => {
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let target = required(input.element_id, "element_id is required")?;
            if input.action == "inset_status" {
                visualization_assets::inset_status(
                    service.store(),
                    &project,
                    &id,
                    revision,
                    &target,
                )
                .map_err(failure)
            } else {
                let mut proposal = json!(
                    visualization_assets::refresh_inset(
                        service.store(),
                        &project,
                        &id,
                        revision,
                        &target,
                        input.discard_detail_edits.unwrap_or(false)
                    )
                    .map_err(failure)?
                );
                proposal.as_object_mut().unwrap().remove("document");
                Ok(proposal)
            }
        }
        "image_info" | "inset" => {
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            if input.action == "inset" {
                let spec = input.inset.ok_or_else(|| invalid("inset is required"))?;
                Ok(json!(
                    visualization_assets::inset(service.store(), &project, &id, revision, &spec)
                        .map_err(failure)?
                ))
            } else {
                let target = required(input.element_id, "element_id is required")?;
                let mut info = visualization_assets::image_info(
                    service.store(),
                    &project,
                    &id,
                    revision,
                    &target,
                )
                .map_err(failure)?;
                info.as_object_mut().unwrap().remove("dataURL");
                Ok(info)
            }
        }
        "read_proposal" | "revise_proposal" => {
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let proposal_id = required(input.proposal_id, "proposal_id is required")?;
            if input.action == "revise_proposal" {
                let operations = input
                    .operations
                    .map(serde_json::from_value::<Vec<visualization::scene::Operation>>)
                    .transpose()
                    .map_err(failure)?;
                let request = visualization::service::ProposalRevisionInput {
                    parent_proposal_id: proposal_id,
                    parent_document_sha256: required(
                        input.document_sha256,
                        "document_sha256 is required",
                    )?,
                    summary: required(input.title, "title is required")?,
                    operations,
                    arrangement: input.arrangement,
                    markup_ids: input.markup_ids,
                };
                let mut result = json!(
                    service
                        .revise_proposal(&project, &id, revision, &request)
                        .map_err(failure)?
                );
                result.as_object_mut().unwrap().remove("document");
                Ok(result)
            } else {
                let current = service
                    .store()
                    .get(&project, &id)
                    .map_err(failure)?
                    .revision;
                if current != revision {
                    return Err(failure(visualization::Error::Conflict));
                }
                let mut proposal = service
                    .store()
                    .proposal(&project, &id, &proposal_id)
                    .map_err(failure)?;
                if let Some(files) = proposal
                    .document
                    .get_mut("files")
                    .and_then(Value::as_object_mut)
                {
                    for file in files.values_mut() {
                        if let Some(file) = file.as_object_mut() {
                            file.remove("dataURL");
                        }
                    }
                }
                Ok(
                    json!({"proposal":proposal,"current_revision":current,"base_is_current":proposal.base_revision==current,"image_data_omitted":true}),
                )
            }
        }
        "read_proposal_preview" => {
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let proposal = required(input.proposal_id, "proposal_id is required")?;
            Ok(json!(
                service
                    .store()
                    .request_proposal_preview(
                        &project,
                        &id,
                        &proposal,
                        revision,
                        input.retry_preview.unwrap_or(false)
                    )
                    .map_err(failure)?
            ))
        }
        "read_image" => {
            let scene = service.current(&project, &id).map_err(failure)?;
            if input
                .expected_revision
                .is_some_and(|revision| revision != scene.revision)
            {
                return Err(failure(visualization::Error::Conflict));
            }
            let element_id = required(input.element_id, "element_id is required")?;
            let element = scene.document["elements"]
                .as_array()
                .and_then(|elements| {
                    elements.iter().find(|element| {
                        element["id"].as_str() == Some(&element_id)
                            && element["type"] == "image"
                            && element["isDeleted"] != true
                    })
                })
                .ok_or_else(|| invalid("image object not found"))?;
            let file_id = element["fileId"]
                .as_str()
                .ok_or_else(|| invalid("image object has no file"))?;
            let data = scene.document["files"][file_id]["dataURL"]
                .as_str()
                .ok_or_else(|| invalid("image bytes unavailable"))?;
            let raster;
            let data = if let Some(encoded) = data.strip_prefix("data:image/svg+xml;base64,") {
                use base64::Engine;
                if encoded.len() > 6 * 1024 * 1024 {
                    return Err(invalid("SVG exceeds image size limit"));
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(failure)?;
                raster = ::bioicons::vector::normalize(&bytes)
                    .map_err(failure)?
                    .preview_data_url;
                raster.as_str()
            } else {
                data
            };

            Ok(
                json!({"dataURL":data,"figure_id":id,"element_id":element_id,"revision":scene.revision,"provenance":element.get("customData")}),
            )
        }
        "apply" | "propose" => {
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let operations: Vec<visualization::scene::Operation> = serde_json::from_value(
                input
                    .operations
                    .ok_or_else(|| invalid("operations is required"))?,
            )
            .map_err(failure)?;
            if operations.len() > 1000 {
                return Err(invalid("at most 1000 operations"));
            }
            if input.action == "propose" {
                let summary = required(input.title, "title must summarize the proposal")?;
                let mut proposal = json!(
                    service
                        .propose_with_markups(
                            &project,
                            &id,
                            revision,
                            &summary,
                            &operations,
                            input.markup_ids.as_deref().unwrap_or(&[])
                        )
                        .map_err(failure)?
                );
                proposal.as_object_mut().unwrap().remove("document");
                return Ok(proposal);
            }
            Ok(json!(
                service
                    .apply(&project, &id, revision, &operations)
                    .map_err(failure)?
            ))
        }
        _ => Err(invalid("unknown visualization action")),
    }
}
#[async_trait]
impl ToolFunction for FigureTool {
    type Input = FigureInput;
    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let image = matches!(
            input.action.as_str(),
            "read_image" | "read_preview" | "read_markup" | "read_proposal_preview"
        );
        let candidate = if input.action == "read_proposal_preview" {
            Some((
                input.project_id.clone(),
                input.figure_id.clone(),
                input.proposal_id.clone(),
                input.expected_revision,
            ))
        } else {
            None
        };
        let path = self.path.clone();
        let mut value = tokio::task::spawn_blocking(move || execute(path, input))
            .await
            .map_err(failure)??;
        if let Some((project, figure, proposal, revision)) = candidate {
            let (project, figure, proposal, revision) = (
                project.ok_or_else(|| invalid("project_id is required"))?,
                figure.ok_or_else(|| invalid("figure_id is required"))?,
                proposal.ok_or_else(|| invalid("proposal_id is required"))?,
                revision.ok_or_else(|| invalid("expected_revision is required"))?,
            );
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
            while value["render_status"] == "requested" && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let (path, project, figure, proposal) = (
                    self.path.clone(),
                    project.clone(),
                    figure.clone(),
                    proposal.clone(),
                );
                value = tokio::task::spawn_blocking(move || {
                    let store = Store::open(path).map_err(failure)?;
                    Ok::<Value, ToolError>(json!(
                        store
                            .proposal_preview(&project, &figure, &proposal, revision)
                            .map_err(failure)?
                    ))
                })
                .await
                .map_err(failure)??;
            }
            if value["render_status"] == "requested" {
                return Err(invalid(
                    "Candidate preview is queued; keep an authenticated GUI workspace open and retry read_proposal_preview. No saved-canvas image was substituted.",
                ));
            }
            if value["render_status"] == "failed" {
                return Err(invalid(&format!(
                    "Candidate rendering failed: {}. Fix the resources, then retry_preview:true.",
                    value["error"]
                        .as_str()
                        .unwrap_or("unknown browser rendering error")
                )));
            }
        }
        if image {
            let data_url = value
                .as_object_mut()
                .and_then(|value| value.remove("dataURL"))
                .and_then(|value| value.as_str().map(str::to_owned))
                .ok_or_else(|| invalid("image bytes unavailable"))?;
            let (header, data) = data_url
                .split_once(',')
                .ok_or_else(|| invalid("invalid image encoding"))?;
            let mime = match header {
                "data:image/png;base64" => "image/png",
                "data:image/jpeg;base64" => "image/jpeg",
                "data:image/webp;base64" => "image/webp",
                _ => return Err(invalid("vision supports embedded PNG, JPEG or WebP images")),
            };
            if data.is_empty()
                || data.len() > 6 * 1024 * 1024
                || data.len() % 4 != 0
                || !data
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte))
            {
                return Err(invalid(
                    "invalid image encoding or image exceeds the 6 MiB base64 vision limit; resize the reference image",
                ));
            }
            return Ok(ToolResult::with_blocks(vec![
                ToolResultBlock::text(value.to_string()),
                ToolResultBlock::image_base64(mime, data),
            ]));
        }
        Ok(ToolResult::success_json(value))
    }
}
pub fn registrations(path: PathBuf) -> Vec<ToolRegistration> {
    vec![ToolRegistration::from(FigureTool { path })]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exported_schema_exposes_typed_briefs_and_edit_operations() {
        fn property<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
            if let Some(found) = value.get("properties").and_then(|p| p.get(name)) {
                return Some(found);
            }
            match value {
                Value::Object(o) => o.values().find_map(|v| property(v, name)),
                Value::Array(a) => a.iter().find_map(|v| property(v, name)),
                _ => None,
            }
        }
        let tool = registrations("unused.sqlite".into()).remove(0);
        let schema = serde_json::to_value(tool.definition.input_schema).unwrap();
        assert!(
            schema["properties"]["action"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!("sketch_plan"))
        );
        assert!(schema.to_string().contains("set_wrapped_text"));
        assert!(schema.to_string().contains("set_relation_direction"));
        let document = &schema["properties"]["document"];
        assert_eq!(property(document, "edges").unwrap()["type"], "array");
        let question = property(document, "questions").unwrap();
        assert_eq!(question["type"], "array");
        assert_eq!(property(question, "answer").unwrap()["type"], "string");
        assert_eq!(property(document, "from").unwrap()["type"], "string");
        assert!(
            schema["properties"]["title"]["description"]
                .as_str()
                .unwrap()
                .contains("sketch_plan")
        );
        assert!(!schema.to_string().contains("\"$ref\""));
    }
    #[tokio::test]
    async fn proposal_preview_tool_waits_for_renderer_and_never_falls_back_to_saved_image() {
        const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        async fn publish(path: PathBuf, figure: String, proposal: String) {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                let store = Store::open(&path).unwrap();
                if let Some(job) = store
                    .proposal_render_requests("p", &figure)
                    .unwrap()
                    .into_iter()
                    .find(|p| p.proposal_id == proposal)
                {
                    store
                        .complete_proposal_preview(
                            "p",
                            &figure,
                            &proposal,
                            visualization::proposal_preview::PreviewCompletion {
                                base_revision: job.base_revision,
                                document_sha256: &job.document_sha256,
                                data_url: Some(PNG),
                                error: None,
                                renderer: "test-worker",
                            },
                        )
                        .unwrap();
                    return;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "render request was not published"
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))
            .unwrap();
        store.save_preview("p", &figure.id, 1, PNG).unwrap();
        let p = store
            .propose(
                "p",
                &figure.id,
                1,
                "Candidate",
                &json!({"elements":[{"id":"candidate","type":"rectangle"}]}),
            )
            .unwrap();
        let tool = registrations(path.clone()).remove(0);
        let input = json!({"action":"read_proposal_preview","project_id":"p","figure_id":figure.id,"proposal_id":p.id,"expected_revision":1});
        let worker = tokio::spawn(publish(path.clone(), figure.id.clone(), p.id.clone()));
        let result =
            serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap())
                .unwrap();
        worker.await.unwrap();
        assert_eq!(result["content"][1]["type"], "image");
        assert_eq!(result["content"][1]["source"]["media_type"], "image/png");
        let caption: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(caption["scope"], "proposal_candidate");
        assert_eq!(caption["proposal_id"], p.id);
        assert_eq!(caption["proposal_status"], "pending");
        assert!(caption.get("dataURL").is_none());
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        store.accept_proposal("p", &figure.id, &p.id).unwrap();
        let mut historical = input;
        historical["expected_revision"] = json!(2);
        let result =
            serde_json::to_value(tool.implementation.execute(historical).await.unwrap()).unwrap();
        let caption: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(caption["current_revision"], 2);
        assert_eq!(caption["proposal_status"], "accepted");
        assert_eq!(caption["base_is_current"], false);
        let failed = store
            .propose("p", &figure.id, 2, "Failed", &json!({"elements":[]}))
            .unwrap();
        let queued = store
            .request_proposal_preview("p", &figure.id, &failed.id, 2, false)
            .unwrap();
        store
            .complete_proposal_preview(
                "p",
                &figure.id,
                &failed.id,
                visualization::proposal_preview::PreviewCompletion {
                    base_revision: 2,
                    document_sha256: &queued.document_sha256,
                    data_url: None,
                    error: Some("missing image"),
                    renderer: "test-worker",
                },
            )
            .unwrap();
        let mut retry = json!({"action":"read_proposal_preview","project_id":"p","figure_id":figure.id,"proposal_id":failed.id,"expected_revision":2});
        assert!(tool.implementation.execute(retry.clone()).await.is_err());
        retry["retry_preview"] = json!(true);
        let worker = tokio::spawn(publish(path.clone(), figure.id.clone(), failed.id.clone()));
        assert!(tool.implementation.execute(retry).await.is_ok());
        worker.await.unwrap();
        store.save_preview("p", &figure.id, 2, PNG).unwrap();
        let no_renderer = store
            .propose("p", &figure.id, 2, "No renderer", &json!({"elements":[]}))
            .unwrap();
        let error=tool.implementation.execute(json!({"action":"read_proposal_preview","project_id":"p","figure_id":figure.id,"proposal_id":no_renderer.id,"expected_revision":2})).await.err().unwrap();
        assert!(error.to_string().contains("queued"));
        assert_eq!(
            store
                .proposal_preview("p", &figure.id, &no_renderer.id, 2)
                .unwrap()
                .render_status,
            "requested"
        );
    }
    #[tokio::test]
    async fn arrange_tool_proposes_with_review_links_and_keeps_revision_until_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let plan=serde_json::from_value(json!({"message":"Synthetic layout","audience":"Researchers","nodes":[{"id":"a","label":"A","column":0,"row":0},{"id":"b","label":"B","column":1,"row":1},{"id":"c","label":"C","column":2,"row":0}]})).unwrap();
        let scene = visualization::sketch::compile(&plan).unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &scene)
            .unwrap();
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        let review = store
            .create_markup(
                "p",
                &figure.id,
                1,
                "Align these components",
                &[visualization::markup::Stroke {
                    kind: visualization::markup::Kind::Draw,
                    points: vec![[0., 0.], [30., 30.]],
                }],
                png,
            )
            .unwrap();
        let tool = registrations(path).remove(0);
        let input = json!({"action":"arrange","project_id":"p","figure_id":figure.id,"expected_revision":1,"arrangement":{"element_ids":["node-0","node-1","node-2"],"mode":"top"},"markup_ids":[review.id]});
        let result =
            serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap())
                .unwrap();
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        assert!(result["content"].get("document").is_none());
        let proposal = store.proposals("p", &figure.id).unwrap().remove(0);
        assert_eq!(proposal.review_markups.len(), 1);
        assert_eq!(proposal.review_markups[0].id, review.id);
        let nodes = proposal.document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "rectangle")
            .collect::<Vec<_>>();
        assert!(nodes.iter().all(|n| n["y"] == nodes[0]["y"]));
        assert_eq!(store.markups("p", &figure.id).unwrap()[0].status, "open");
        let mut stale = input;
        stale["expected_revision"] = json!(0);
        assert!(tool.implementation.execute(stale).await.is_err());
        store
            .accept_proposal_with_reviews("p", &figure.id, &proposal.id, true)
            .unwrap();
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 2);
        assert_eq!(
            store.markups("p", &figure.id).unwrap()[0].status,
            "resolved"
        );
        assert_eq!(store.revision("p", &figure.id, 1).unwrap().document, scene);
    }
    #[tokio::test]
    async fn inset_tool_proposes_and_does_not_expose_image_bytes_in_info() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();

        use base64::Engine;
        let url=format!("data:image/svg+xml;base64,{}",base64::engine::general_purpose::STANDARD.encode(b"<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'><path d='M0 0h100v100H0z'/></svg>"));
        let (scene, target) = visualization::image::insert(
            &json!({"elements":[]}),
            visualization::image::Image {
                file_id: "source",
                data_url: &url,
                width: 100,
                height: 100,
                provenance: json!({"asset":{"license":"cc0"}}),
            },
            0.,
            0.,
        )
        .unwrap();
        store.create_project("p", "Study").unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &scene)
            .unwrap();
        let spec = json!({"element_id":target,"region":{"x":0.1,"y":0.2,"width":0.4,"height":0.5},"x":600,"y":100,"max_size":320,"title":"Detail"});

        let tool = registrations(path).remove(0);
        let info=serde_json::to_value(tool.implementation.execute(json!({"action":"image_info","project_id":"p","figure_id":figure.id,"element_id":target,"expected_revision":1})).await.unwrap()).unwrap();
        assert_eq!(info["content"]["natural_width"], 100);
        assert!(info["content"].get("dataURL").is_none());
        let mut input = json!({"action":"inset","project_id":"p","figure_id":figure.id,"expected_revision":1,"inset":spec});
        let result =
            serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap())
                .unwrap();
        assert_eq!(
            result["content"]["document"]["elements"]
                .as_array()
                .unwrap()
                .len(),
            6
        );
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        input["expected_revision"] = json!(0);
        assert!(tool.implementation.execute(input).await.is_err());
        store
            .accept_proposal("p", &figure.id, result["content"]["id"].as_str().unwrap())
            .unwrap();

        let target = &result["content"]["document"]["elements"][3]["id"];
        let checked=serde_json::to_value(tool.implementation.execute(json!({"action":"inset_status","project_id":"p","figure_id":figure.id,"element_id":target,"expected_revision":2})).await.unwrap()).unwrap();
        assert_eq!(checked["content"]["changed"], false);
        assert!(!checked.to_string().contains("data:image"));
        let refreshed=serde_json::to_value(tool.implementation.execute(json!({"action":"refresh_inset","project_id":"p","figure_id":figure.id,"element_id":target,"expected_revision":2})).await.unwrap()).unwrap();
        assert_eq!(refreshed["content"]["base_revision"], 2);
        assert!(refreshed["content"].get("document").is_none());
        let anchor = &result["content"]["document"]["elements"][3]["id"];
        let removal=serde_json::to_value(tool.implementation.execute(json!({"action":"propose","project_id":"p","figure_id":figure.id,"expected_revision":2,"title":"Remove inset","operations":[{"op":"remove","id":anchor}]})).await.unwrap()).unwrap();
        let saved_removal = store
            .proposals("p", &figure.id)
            .unwrap()
            .into_iter()
            .find(|p| p.id == removal["content"]["id"].as_str().unwrap())
            .unwrap();
        assert_eq!(saved_removal.document["elements"], scene["elements"]);
        assert_eq!(saved_removal.document["files"], scene["files"]);
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 2);
    }
    #[tokio::test]
    async fn planning_tools_preserve_answers_and_require_a_ready_version_for_sketches() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))
            .unwrap();
        let tool = registrations(path).remove(0);
        let base = json!({"project_id":"p","figure_id":figure.id,"expected_revision":1});
        let mut save = base.clone();
        save["action"] = json!("save_plan");
        save["planning_version"] = json!(0);
        save["document"] = json!({"message":"A generic signaling example","audience":"Researchers","questions":[{"id":"q1","question":"Which components?"}]});
        let saved =
            serde_json::to_value(tool.implementation.execute(save.clone()).await.unwrap()).unwrap();
        assert_eq!(saved["content"]["status"], "needs_clarification");
        let mut generate = base.clone();
        generate["action"] = json!("sketch_plan");
        generate["planning_version"] = json!(1);
        generate["title"] = json!("Planned sketch");
        assert!(tool.implementation.execute(generate.clone()).await.is_err());
        let mut read = base.clone();
        read["action"] = json!("read_plan");
        let read = serde_json::to_value(tool.implementation.execute(read).await.unwrap()).unwrap();
        assert_eq!(
            read["content"]["document"]["questions"][0]["question"],
            "Which components?"
        );
        let mut brief = read["content"]["document"].clone();
        brief["questions"][0]["answer"] = json!("A single generic cell");
        brief["nodes"] = json!([{"id":"cell","label":"Generic cell"}]);
        save["planning_version"] = json!(1);
        save["document"] = brief;
        tool.implementation.execute(save.clone()).await.unwrap();
        assert!(tool.implementation.execute(save).await.is_err());
        generate["planning_version"] = json!(2);
        let proposal =
            serde_json::to_value(tool.implementation.execute(generate).await.unwrap()).unwrap();
        assert!(proposal["content"].get("document").is_none());
        let mut bypass = base.clone();
        bypass["action"] = json!("sketch");
        bypass["title"] = json!("Bypass");
        bypass["document"] = json!({"message":"Other","audience":"Researchers","layout":"horizontal","nodes":[{"id":"x","label":"Other cell"}]});
        assert!(tool.implementation.execute(bypass).await.is_err());
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        store
            .accept_proposal("p", &figure.id, proposal["content"]["id"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            store.revision("p", &figure.id, 2).unwrap().document["elements"][0]["customData"]["planningSource"]
                ["document"]["questions"][0]["answer"],
            "A single generic cell"
        );
    }
    #[tokio::test]
    async fn markup_vision_keeps_exact_review_revision_and_does_not_leak_bytes_into_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))
            .unwrap();
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        let review = store
            .create_markup(
                "p",
                &figure.id,
                1,
                "Add a missing step",
                &[visualization::markup::Stroke {
                    kind: visualization::markup::Kind::Draw,
                    points: vec![[1., 2.], [30., 40.]],
                }],
                png,
            )
            .unwrap();
        store
            .save("p", &figure.id, 1, "Changed", &json!({"elements":[]}))
            .unwrap();
        let tool = registrations(path).remove(0);
        let read = tool
            .implementation
            .execute(json!({"action":"read","project_id":"p","figure_id":figure.id}))
            .await
            .unwrap();
        let read = serde_json::to_value(read).unwrap();
        assert_eq!(read["content"]["markups"][0]["id"], review.id);
        assert!(!read.to_string().contains("iVBOR"));
        let page = serde_json::to_value(tool.implementation.execute(json!({"action":"list_markups","project_id":"p","figure_id":figure.id,"status":"open","limit":1})).await.unwrap()).unwrap();
        assert_eq!(page["content"]["items"][0]["id"], review.id);
        assert!(page["content"]["next_before"].is_null());
        assert!(!page.to_string().contains("iVBOR"));
        assert!(tool.implementation.execute(json!({"action":"list_markups","project_id":"p","figure_id":figure.id,"status":"unknown"})).await.is_err());

        let linked = json!({"action":"propose","project_id":"p","figure_id":figure.id,"expected_revision":2,"title":"Address historical review","operations":[{"op":"add","element":{"id":"step","type":"rectangle"}}],"markup_ids":[review.id]});
        let candidate =
            serde_json::to_value(tool.implementation.execute(linked.clone()).await.unwrap())
                .unwrap();
        assert_eq!(candidate["content"]["review_markups"][0]["id"], review.id);
        assert!(!candidate.to_string().contains("iVBOR"));
        assert_eq!(
            store
                .markup("p", &figure.id, &review.id)
                .unwrap()
                .summary
                .status,
            "open"
        );
        let mut direct = linked;
        direct["action"] = json!("apply");
        assert!(tool.implementation.execute(direct).await.is_err());
        let input = json!({"action":"read_markup","project_id":"p","figure_id":figure.id,"markup_id":review.id,"expected_revision":2});
        let result =
            serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap())
                .unwrap();
        assert_eq!(result["content"][1]["type"], "image");
        assert_eq!(result["content"][1]["source"]["media_type"], "image/png");
        let caption: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(caption["at_revision"], 1);
        assert_eq!(caption["current_revision"], 2);
        assert_eq!(caption["historical"], true);
        let mut stale = input.clone();
        stale["expected_revision"] = json!(1);
        assert!(tool.implementation.execute(stale).await.is_err());
        let mut other = input;
        other["project_id"] = json!("other");
        assert!(tool.implementation.execute(other).await.is_err());
    }
    #[tokio::test]
    async fn sketch_in_an_existing_figure_proposes_without_creating_another_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let figure = store
            .create(
                "p",
                "Active canvas",
                "excalidraw/v2",
                &json!({"elements":[],"files":{}}),
            )
            .unwrap();
        let tool = registrations(path).remove(0);
        let input = |revision| json!({"action":"sketch","project_id":"p","figure_id":figure.id,"expected_revision":revision,"title":"Cell overview","document":{"message":"Show a cell","audience":"researchers","layout":"horizontal","nodes":[{"id":"cell","label":"Cell"}]}});
        assert!(tool.implementation.execute(input(0)).await.is_err());
        let proposal =
            serde_json::to_value(tool.implementation.execute(input(1)).await.unwrap()).unwrap();
        assert_eq!(proposal["content"]["figure_id"], figure.id);
        assert_eq!(store.list("p").unwrap().len(), 1);
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        store
            .accept_proposal("p", &figure.id, proposal["content"]["id"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            store.revision("p", &figure.id, 2).unwrap().document["elements"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(store.list("p").unwrap().len(), 1);
    }
    #[tokio::test]
    async fn reference_images_use_vision_blocks_without_polluting_scene_reads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        let document = json!({"elements":[{"id":"reference","type":"image","fileId":"pic"}],"files":{"pic":{"id":"pic","mimeType":"image/png","dataURL":format!("data:image/png;base64,{png}")}}});
        let figure = store
            .create("p", "Reference", "excalidraw/v2", &document)
            .unwrap();
        store
            .save_preview("p", &figure.id, 1, &format!("data:image/png;base64,{png}"))
            .unwrap();
        let tool = registrations(path).remove(0);
        let preview = tool.implementation.execute(json!({"action":"read_preview","project_id":"p","figure_id":figure.id,"expected_revision":1})).await.unwrap();
        assert_eq!(
            serde_json::to_value(preview).unwrap()["content"][1]["source"]["data"],
            png
        );
        assert!(tool.implementation.execute(json!({"action":"read_preview","project_id":"p","figure_id":figure.id,"expected_revision":0})).await.is_err());
        let read = tool
            .implementation
            .execute(json!({"action":"read","project_id":"p","figure_id":figure.id}))
            .await
            .unwrap();
        let read = serde_json::to_value(read).unwrap();
        assert!(
            read["content"]["scene"]["document"]["files"]["pic"]
                .get("dataURL")
                .is_none()
        );
        assert_eq!(
            store.revision("p", &figure.id, 1).unwrap().document,
            document
        );
        let image = tool.implementation.execute(json!({"action":"read_image","project_id":"p","figure_id":figure.id,"element_id":"reference","expected_revision":1})).await.unwrap();
        let image = serde_json::to_value(image).unwrap();
        assert_eq!(image["content"][1]["type"], "image");
        assert_eq!(image["content"][1]["source"]["media_type"], "image/png");
        assert_eq!(image["content"][1]["source"]["data"], png);
        assert!(tool.implementation.execute(json!({"action":"read_image","project_id":"p","figure_id":figure.id,"element_id":"reference","expected_revision":0})).await.is_err());
        assert!(tool.implementation.execute(json!({"action":"read_image","project_id":"p","figure_id":figure.id,"element_id":"missing"})).await.is_err());
    }
    #[tokio::test]
    async fn candidate_discovery_is_paged_scoped_and_version_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        store.create_project("other", "Other").unwrap();
        let scene = json!({"elements":[],"files":{"asset":{"dataURL":"PRIVATE-BYTES"}}});
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &scene)
            .unwrap();
        let mut ids = vec![];
        for index in 0..3 {
            ids.push(
                store
                    .propose("p", &figure.id, 1, &format!("Candidate {index}"), &scene)
                    .unwrap()
                    .id,
            );
        }
        let tool = registrations(path).remove(0);
        let input = json!({"action":"list_proposals","project_id":"p","figure_id":figure.id,"expected_revision":1,"limit":2});
        let first = serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap())
            .unwrap();
        assert_eq!(first["content"]["items"].as_array().unwrap().len(), 2);
        assert_eq!(first["content"]["items"][0]["id"], ids[2]);
        assert!(!first.to_string().contains("PRIVATE-BYTES"));
        assert!(
            !first["content"]["items"][0]
                .as_object()
                .unwrap()
                .contains_key("document")
        );
        let mut next = input.clone();
        next["before"] = first["content"]["next_before"].clone();
        let second =
            serde_json::to_value(tool.implementation.execute(next).await.unwrap()).unwrap();
        assert_eq!(second["content"]["items"][0]["id"], ids[0]);
        assert!(second["content"]["next_before"].is_null());
        let read = serde_json::to_value(
            tool.implementation
                .execute(json!({"action":"read","project_id":"p","figure_id":figure.id}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(read["content"]["proposal_summary"]["total"], 3);
        assert!(!read.to_string().contains("PRIVATE-BYTES"));
        for (key, value) in [
            ("expected_revision", json!(2)),
            ("project_id", json!("other")),
            ("limit", json!(0)),
            ("status", json!("accepted")),
        ] {
            let mut invalid = input.clone();
            invalid[key] = value;
            assert!(tool.implementation.execute(invalid).await.is_err());
        }
    }

    #[tokio::test]
    async fn local_relation_operation_is_reviewed_and_composes_with_candidate_revisions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let plan=serde_json::from_value(json!({"message":"Synthetic relation fixture","audience":"Testers","layout":"horizontal","nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b","label":"relation","evidence":["fixture-source"]}]})).unwrap();
        let original = visualization::sketch::compile(&plan).unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &original)
            .unwrap();
        let tool = registrations(path).remove(0);
        let input = json!({"action":"propose","project_id":"p","figure_id":figure.id,"expected_revision":1,"title":"Make reciprocal","operations":[{"op":"set_relation_direction","id":"edge-0","direction":"bidirectional","inhibitory":false},{"op":"update","id":"edge-label-0","changes":{"text":"Reviewed","originalText":"Reviewed"}}]});
        let result =
            serde_json::to_value(tool.implementation.execute(input.clone()).await.unwrap())
                .unwrap();
        let parent = store
            .proposal("p", &figure.id, result["content"]["id"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            parent.document["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == "edge-0")
                .unwrap()["startArrowhead"],
            "arrow"
        );
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        let revised=serde_json::to_value(tool.implementation.execute(json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"expected_revision":1,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"title":"Explicit inhibition","operations":[{"op":"set_relation_direction","id":"edge-0","direction":"forward","inhibitory":true}]})).await.unwrap()).unwrap();
        let child = store
            .proposal("p", &figure.id, revised["content"]["id"].as_str().unwrap())
            .unwrap();
        let edge = child.document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "edge-0")
            .unwrap();
        assert_eq!(edge["startArrowhead"], Value::Null);
        assert_eq!(edge["endArrowhead"], "bar");
        assert_eq!(edge["customData"]["evidence"], json!(["fixture-source"]));
        assert_eq!(
            child.document["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == "edge-label-0")
                .unwrap()["text"],
            "Reviewed"
        );
        let mut missing = input;
        missing["operations"][0]
            .as_object_mut()
            .unwrap()
            .remove("inhibitory");
        assert!(tool.implementation.execute(missing).await.is_err());
        assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 2);
        store.accept_proposal("p", &figure.id, &child.id).unwrap();
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 2);
        assert_eq!(
            store.revision("p", &figure.id, 1).unwrap().document,
            original
        );
        assert_eq!(
            store.proposal("p", &figure.id, &parent.id).unwrap().status,
            "pending"
        );
    }

    #[tokio::test]
    async fn candidate_revision_tool_preserves_parent_and_redacts_image_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let figure = store.create("p", "Figure", "excalidraw/v2", &json!({"elements":[],"files":{"asset":{"dataURL":"private-image-bytes","mimeType":"image/png"}}})).unwrap();
        let parent = store.propose("p", &figure.id, 1, "Parent", &json!({"elements":[{"id":"added","type":"text","text":"First"}],"files":{"asset":{"dataURL":"private-image-bytes","mimeType":"image/png"}}})).unwrap();
        let tool = registrations(path).remove(0);
        let read = serde_json::to_value(tool.implementation.execute(json!({"action":"read_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"expected_revision":1})).await.unwrap()).unwrap();
        assert!(!read.to_string().contains("private-image-bytes"));
        assert_eq!(
            read["content"]["proposal"]["document_sha256"],
            parent.document_sha256
        );
        assert_eq!(
            read["content"]["proposal"]["document"]["elements"][0]["text"],
            "First"
        );
        let mut input = json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"proposal_id":parent.id,"document_sha256":"wrong","expected_revision":1,"title":"Child","operations":[{"op":"update","id":"added","changes":{"text":"Revised"}}]});
        assert!(tool.implementation.execute(input.clone()).await.is_err());
        input["document_sha256"] = json!(parent.document_sha256);
        let result =
            serde_json::to_value(tool.implementation.execute(input).await.unwrap()).unwrap();
        assert!(result["content"]["document"].is_null());
        assert_eq!(result["content"]["parent_proposal_id"], parent.id);
        let child = store
            .proposal("p", &figure.id, result["content"]["id"].as_str().unwrap())
            .unwrap();
        assert_eq!(child.document["elements"][0]["text"], "Revised");
        assert_eq!(child.document["files"], parent.document["files"]);
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        assert_eq!(
            store
                .proposal("p", &figure.id, &parent.id)
                .unwrap()
                .document,
            parent.document
        );
    }

    #[tokio::test]
    async fn registered_tool_creates_sketch_and_proposes_a_local_text_edit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("figures.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let tool = registrations(path.clone()).remove(0);
        assert_eq!(tool.definition.name, "visualization");
        let created = tool.implementation.execute(json!({"action":"sketch","project_id":"p","title":"Signal","document":{"message":"Binding","audience":"researchers","nodes":[{"id":"a","label":"Ligand","column":0,"row":0},{"id":"b","label":"Receptor","column":1,"row":0}],"edges":[{"from":"a","to":"b","label":"binds"}]}})).await.unwrap();
        let created = serde_json::to_value(created).unwrap();
        let id = created["content"]["id"].as_str().unwrap();
        let before = store.revision("p", id, 1).unwrap();
        let proposal = tool.implementation.execute(json!({"action":"propose","project_id":"p","figure_id":id,"expected_revision":1,"title":"Clarify ligand","operations":[{"op":"update","id":"label-0","changes":{"text":"Ligand A"}}]})).await.unwrap();
        let proposal = serde_json::to_value(proposal).unwrap();
        assert_eq!(store.get("p", id).unwrap().revision, 1);
        let proposal_id = proposal["content"]["id"].as_str().unwrap();
        store.accept_proposal("p", id, proposal_id).unwrap();
        let after = store.revision("p", id, 2).unwrap();
        assert_eq!(after.document["elements"][1]["text"], "Ligand A");
        assert_eq!(after.document["elements"][1]["originalText"], "Ligand A");
        assert_eq!(
            after.document["elements"][2],
            before.document["elements"][2]
        );
        store
            .annotate("p", id, 2, &["label-0".into()], "Increase label emphasis")
            .unwrap();
        let read = tool
            .implementation
            .execute(json!({"action":"read","project_id":"p","figure_id":id}))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(read).unwrap()["content"]["annotations"][0]["note"],
            "Increase label emphasis"
        );
        assert!(tool.implementation.execute(json!({"action":"apply","project_id":"p","figure_id":id,"expected_revision":1,"operations":[]})).await.is_err());
    }
    #[test]
    fn agent_edits_share_gui_storage_and_reject_stale_revisions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("visualizations.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))
            .unwrap();
        let input = |revision| FigureInput {
            inset: None,
            arrangement: None,
            proposal_id: None,
            document_sha256: None,
            retry_preview: None,
            discard_detail_edits: None,
            action: "apply".into(),
            project_id: Some("p".into()),
            figure_id: Some(figure.id.clone()),
            element_id: None,
            markup_id: None,
            markup_ids: None,
            before: None,
            status: None,
            limit: None,
            planning_version: None,
            title: None,
            document: None,
            expected_revision: Some(revision),
            operations: Some(json!([{"op":"add","element":{"id":"cell","type":"ellipse"}}])),
        };
        assert_eq!(execute(path.clone(), input(1)).unwrap()["revision"], 2);
        assert!(execute(path.clone(), input(1)).is_err());
        assert_eq!(
            store.revision("p", &figure.id, 2).unwrap().document["elements"][0]["id"],
            "cell"
        );
    }
}
