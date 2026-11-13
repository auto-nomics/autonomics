//! Agent access to the same project figures edited in the GUI.
mod add_text;
mod reflow_text;
mod scoped_read;
mod find_space;
mod asset_candidate;
mod bioart;
mod bioicons;
mod bioicons_batch;
mod catalog_search;
mod evidence;
mod follow_up;
mod fit;
mod fit_svg;
mod fit_svg_group;
mod translate;
mod scale;
mod crop;
mod move_text;
mod plan_label;
use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::{ToolResult, tools::ToolResultBlock};
use async_trait::async_trait;
pub use bioart::registrations as bioart_registrations;
pub use bioicons::registrations as bioicons_registrations;
pub use evidence::registrations as evidence_registrations;
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
    description = "Manage editable scientific figures shared with the GUI. Compact existing top-level frames with propose/revise_proposal operation {op:pack_frames,element_ids:[frame_ids],vertical:false,gap:144}; gap is a JSON number in 8..512. It preserves current spatial order and orthogonal positions, moves full owned components and reroutes supported connectors. For explicitly authorized caption rewrapping, prepend set_wrapped_text operations for the selected cross-panel arrow captions with exact originalText, existing font_size and max_width; never abbreviate science or reduce font size implicitly. Read the resulting document and PNG: narrower gaps can increase routing length or create tall captions. This always creates a pending candidate, never adopts. To repair drifted endpoints or a diagonal sharp LOCAL connector, use rebind_connector_ports with id, start_side and end_side (left/right/top/bottom). This explicitly rebuilds a same-frame orthogonal path at centered shape ports, preserving binding IDs, gaps, direction, scientific metadata and caption position. Noncentered/fixedPoint bindings, curves, locks and impossible routes reject. Read the exact candidate and inspect PNG before adopting. Candidate reads, revision receipts and final PNG metadata include materialization_checkpoint: resolve needs_fit/missing/ambiguous components, label ownership issues and unassigned images for the requested scope before reporting materialization complete. This is bounded ownership/fit snapshot freshness, never scientific validity, visible-paint overlap or layout quality. Report partial completion explicitly. scene_geometry_review.text_rectangle_overlap checks all supported live text rectangles, including detached labels and bound captions; inspect overlap_count and status/checked/skipped/pair_omitted coverage. caption_path_overlap only checks caption coverage of its own arrow and cannot establish that text-text overlaps are zero. Neither is glyph-ink, pixel or scientific validation. For a component with multiple owned uncropped unrotated SVG images, prefer visualization_fit_svg_group_preview with the exact candidate ID/hash and element_id; it derives the full sample set from saved files. For other explicitly reviewed group bounds, fit_illustration_group in propose/revise_proposal operations takes anchor_id and samples:[{id,file_id,normalized_bounds:[x,y,width,height]}] for the exact primary plus every accessory. Read current image_info SVG bounds for every member first; the union updates only the anchor and supported connectors. It records all member snapshots and preserves each image, label and source. Unsupported old ports or connector geometry reject atomically; never forge fit metadata. To bind already placed images into one component without reimporting or moving them, use bind_illustration in propose/revise_proposal operations: anchor_id, primary_id, label_ids and members:[{id,meaning}]. Explicit accessories retain their recorded meanings and sources; all members must share a frame and whole existing groups must be listed. This removes old fit metadata: refit the primary against the current SVG and inspect PNG afterward. This does not verify biological identity or stoichiometry. Never forge customData ownership using generic update. For save_plan prefer compact_response:true when submitting a complete known plan: the receipt retains version/status/issues/layout review and read_plan continuation without echoing the full document. Omit/false preserves the legacy full response. This changes only the tool response, never saved scientific content. Review channels are distinct: annotations/focused_annotation_id are object notes, never sketch markup IDs. Only IDs from list_markups/read_markup may be used in markup_ids or read_markup; list_markups.before is a sketch-markup pagination cursor, never an annotation ID. To address an object annotation, use its note and element_ids from GUI context/current read; omit markup_ids or use [] unless separately addressing actual sketch markups. This does not auto-resolve annotations. To move one bound arrow caption along its existing sharp rectilinear path, use {op:position_arrow_caption,id:caption_text_id,position:0.5}; position is a finite fraction strictly between 0 and 1 measured from the start binding along path length, not canvas coordinates. It preserves full text/typography, endpoints, direction and actual path shape while adjusting native collinear points to retain the requested label center. Unsupported curves, rotated/locked objects, ambiguous bindings or frame escape reject. Inspect PNG for overlaps; no scientific meaning is inferred. To explicitly attach an existing standalone accessory image to an illustrated component, use {op:attach_illustration_badge,id:badge_image_id,anchor_id,meaning:user_supplied_meaning}; both must already be in the same frame. This retains the primary artwork and provenance, adds reciprocal badge ownership and the native component group, and follows rigid translation/duplication. It records supplied meaning without scientific validation. Use {op:detach_illustration_badge,id,anchor_id} to unlink without deleting or moving the image. Never use a state badge as the whole complex primary image. image_info reads natural dimensions, visible crop and file hash without image bytes. For SVGs, svg_preview_alpha_bounds reports a sampled alpha bounding box with one preview-pixel padding and file_sha256. Use only with the matching file_id/current file hash; normalized_bounds refer to the whole uncropped source, before native crop, rotation, flip or element opacity. Empty previews have no bounds. Opaque backgrounds and visible decorations are included; these are layout hints, not exact vector or scientific geometry. Inspect the candidate PNG before fitting. For a pending candidate image pass both proposal_id and its exact document_sha256 with the current expected_revision; omitting both reads only the saved formal revision, never an unadopted image. inset remains formal-only. To explicitly exchange two existing top-level panels without regenerating the figure, use {op:swap_frames,first_id:panel_a,second_id:panel_b}. It atomically swaps frame origins and rigid contents, then reroutes affected generated sharp cross-panel connectors; all text, sizes, sources, artwork and historical plans remain. Frames must be independent unlocked unrotated components; unsupported connectors/locked labels reject the whole edit. This is an explicit layout edit, not automatic biological ordering; inspect the resulting candidate PNG. To explicitly reduce empty space in an existing top-level frame, propose/revise_proposal may use {op:fit_frame,id,padding:24}. This only shrinks its right/bottom edges around all live direct children, keeps its origin and every child unchanged, never grows, and retains padding where the original frame permits. Padding must be 8..128. Empty/locked/rotated/nested frames, unsupported child bounds or children outside the old frame are rejected rather than clipped. It does not rearrange panels, reroute connectors or alter historical plans. Preview before adoption. For an existing wide unbound scientific text box, propose/revise_proposal may use {op:fit_text_width,id}. This only shrinks the box around its current wrapped lines, retaining all text, typography, visible alignment, grouping and metadata; it never reflows or grows text. Bound/rotated/locked/non-scientific-font text and text with attached connectors are rejected. Preview before adoption. To explicitly change an existing arrow caption width, use set_wrapped_text on its text ID with the complete unchanged originalText and numeric max_width; this rewraps and recenters that label on the existing sharp arrow path without changing arrow geometry, bindings, styling or evidence. Omitting max_width preserves the caption width. Shape-bound labels still require resizing their container rather than max_width. Preview for collisions after widening. For verbatim text edits use originalText as the source, not text with renderer-inserted line breaks. Preserve existing explicit newlines and pass the revised originalText to set_wrapped_text; let the native wrapper regenerate display lines. Dependent candidate mutations must run sequentially: wait for the returned child proposal_id/document_sha256 and use that child as the next base, or batch compatible operations in one revision. Revisions require a pending leaf parent by default: parallel writes against the same parent allow only one child. On a pending-child error, list_proposals and read_proposal to continue the intended leaf; never choose an arbitrary branch. Only when the user explicitly requests alternatives may revise_proposal set allow_branch:true. Alternatives never merge and the newest candidate does not include sibling changes. If a successful edit needs correction, continue or undo from its returned child; do not restart from the old parent unless the user explicitly requests alternatives. read/read_proposal and candidate mutation results expose scene_geometry_review with full live-object bounds/aspect ratio and minimum known text font size, including captions and image viewports. Check these after adding notes or artwork against the requested composition; plan.layout_review excludes later additions. caption_path_overlap reports bounded warnings when a supported sharp unrotated native arrow has most of its centerline inside its own bound text rectangle; use arrow_id/label_id to inspect the PNG and improve spacing or reroute while preserving the scientific wording. This is a rectangle overlap hint, not glyph-ink or renderer visibility measurement; skipped_count and warning_omitted_count disclose incomplete coverage. Partial/unavailable status or skipped/omitted elements means incomplete coverage. These are geometric object extents, not glyph/stroke collision or scientific review; inspect the actual PNG too. Actions: projects, list, read, image_info, inset, inset_status, refresh_inset, read_image, read_preview, read_proposal_preview, read_proposal, revise_proposal, read_markup, list_markups, list_proposals, read_plan, save_plan, sketch_plan, create, sketch, propose, apply, arrange. For comparison or step panels, declare panels [{id,title,column,row}] and assign every node.panel to a declared ID. Panels compile to native editable frames with independent internal layouts; explicit cross-panel edges use editable scene-level connectors routed around nodes and titles. Updating a frame x/y moves its children and adjusts external connector endpoints; removing a frame detaches and preserves its contents. For one explicitly authorized saved-plan label change, prefer visualization_update_plan_label with the exact plan_field and complete expected_text from read_plan; it preserves all unrelated plan fields server-side and saves one new planning version without changing the canvas. For 1..20 explicitly authorized label edits in the same turn, visualization_update_plan_labels takes a changes array and validates every entry before one atomic save. Use read_plan/save_plan (send the brief in the typed planning argument, not document) to maintain a versioned scientific brief with message, audience, nodes, edges, assumptions, relationship_note and questions [{id,question,answer,blocking:true}]. When key scope is missing, start with at most three essential questions in this turn (organism, stimulus/subtype, endpoint), not an exhaustive competing graph or deep searches across many species. blocking expresses whether an answer is required, not whether it has already been supplied. Keep required scientific questions blocking:true (default) after answering; fill answer without changing blocking. They prevent sketching only while their answer is empty. Preserve empty answers until clarified. Optional language/color/delivery preferences may be blocking:false and left unanswered; do not invent answers to make a plan ready. A later turn can ask further essential questions if needed. Save incomplete briefs to persist focused clarification questions; never invent missing scientific relationships. Ask scope questions neutrally: do not present unverified complex membership, mechanisms or competing branches as established facts in the question itself. After reading sources, review both saved question wording and answers for factual consistency; correct factual errors in the question text while preserving its ID and the user-supplied answer. Keeping a question ID does not require retaining an erroneous scientific premise. read_plan and save_plan include graph_review: weak_components, source_node_ids, sink_node_ids, isolated_node_ids and invalid_edge_indices derived from the declared graph, never scientific validation. read_plan/save_plan also include read-only layout_review from the compiled sketch: bounds, width/height aspect_ratio and each panel bounds, or a compilation error. Use the actual panel dimensions to adjust layout and panel column/row before the user confirms a compact plan; long labels can change panel sizes. These estimates exclude later captions/assets and are not a PNG, overlap check or scientific validation. Layout diagnostics do not impose a publication ratio or authorize a plan change. If a continuous signal is intended, trace the actual direction from the ligand/receptor source to the requested endpoint; a downstream label mentioning a receptor does not connect it to that receptor. Separate substrates may need explicit supported incoming relationships from the active complex. Do not add a biological edge merely to make components connected; reconcile the intended abstraction and evidence. Disconnected comparisons are permitted. Resolve blocking questions and review the structural plan with the user before sketch_plan. sketch_plan only generates a proposal from a ready brief at the exact planning_version and expected_revision. Its title is an optional proposal summary; omitting it uses the GUI default summary, while an explicitly blank title is rejected. Once an existing figure has a saved brief, use sketch_plan rather than direct sketch. For sketch, supply figure_id and expected_revision to propose a new sketch in the active figure; omit figure_id only when creating a separate figure. Prefer propose for changes that should be previewed in the GUI before acceptance. When a proposed change addresses review markup, pass its IDs in markup_ids after reading and reconciling those reviews; this links the candidate to those notes without closing them. The user can explicitly close linked notes when adopting the proposal. read omits embedded image bytes; use read_image with element_id to view a reference image as a vision tool result. read_proposal with proposal_id and expected_revision returns an immutable candidate scene with image bytes omitted, its full stored document_sha256 and parent lineage. revise_proposal creates a new child from that candidate (not the saved scene), retaining previous edits and files; pass proposal_id, document_sha256, expected_revision, title, and exactly one operations or arrangement. Parent must be pending at the current figure base revision; rejected/accepted or stale parents must be reconciled with the saved scene. Parent stays immutable and is not automatically adopted/rejected. If the parent has open review links, explicitly pass markup_ids (including [] to drop) for reviews still addressed by the child; do not silently inherit resolved or no-longer-addressed notes. read_proposal_preview with proposal_id and expected_revision requests an independent browser render of that exact immutable candidate, then waits briefly for the authenticated workspace renderer (the figure editor need not be open) and returns a vision block. It never adopts the proposal. For successful scoped results, copy follow_up_inputs.read_proposal or follow_up_inputs.read_proposal_preview verbatim for the exact candidate; they include the real project_id, figure_id, proposal_id and current expected_revision. candidate_context also carries the document_sha256 for subsequent explicit revision. Never reconstruct UUIDs, substitute visualization_id as a field name, or use planning_version as expected_revision. These continuations are read-only and do not authorize adoption. Inspect scope, document_sha256, base_revision, current_revision and proposal_status; if base_is_current is false, reconcile with the current saved scene. If no renderer is open or a render fails, it reports an error and never substitutes the saved-canvas preview. After fixing render resources, retry_preview:true explicitly requeues a failed render. read_preview returns the full canvas PNG for the current saved revision when the GUI has captured one; never fall back to a stale preview. read_markup with markup_id returns saved review ink over its exact historical canvas as a vision block. The result declares at_revision and current_revision; if they differ, reconcile the historical review with the current scene before proposing edits. read also returns proposal_summary (current/stale pending counts and latest metadata only). Use list_proposals with before and limit (1..100, default 20) to discover pending candidate metadata without scene/image bytes; repeat next_before until null. Read a chosen candidate with read_proposal and the current expected_revision before revising it; listing is not scientific validation or acceptance. Read returns recent markup summaries, markups_pagination.next_before and object-linked review annotations as well as the scene. Use list_markups with before, limit (1..100) and optional status open/resolved to retrieve older reviews; repeat next_before until null. Filter status open to find outstanding reviews even when newer records are resolved; use these notes to interpret selected markup. Use read before apply/propose; use expected_revision from read, never guess it. Use {op:reroute,id} to recompute a generated cross-panel arrow after moving components, from its current bindings and current label size; preserve user text, styles and evidence. Prefer propose for review. Detached endpoints or rotated bound nodes must be resolved first. apply operations are JSON objects: {op:add,element:{id,...}}, {op:update,id,changes:{...}}, {op:remove,id}. Removing an illustrated semantic anchor also removes its owned image and labels: do not remove those IDs again. Connected arrows survive with cleared bindings. Ordinary shape or arrow removal leaves its bound text detached. When the user requests removing a component and its relationships, explicitly remove the affected arrows and their text IDs, preserve unrelated objects, and never invent a replacement relationship. Labels, arrows and icons must remain separate editable elements. Position updates move owned labels/images and ordinary straight connector endpoints. For rotated or elbowed connectors, supply geometry edits explicitly. Preserve unrelated objects, do not flatten the figure into a bitmap. image_info requires element_id and expected_revision and returns actual file dimensions, crop and provenance without image bytes. inset requires expected_revision and inset {element_id,region:{x,y,width,height},x,y,max_size,title}; region is fractional in the current visible image after flips, before rotation; x/y position the new detail panel. Use image_info and read_image first. This creates a pending proposal with editable image crop, region box and neutral callout, preserving original files and licensing. inset_status with element_id (inset anchor/image) and expected_revision compares the current source against its recorded historical snapshot; changed does not merely mean a later figure revision. refresh_inset with the same arguments proposes a refresh in the existing panel, keeping object IDs, panel/title, styles and unrelated objects. The saved fractional region is applied to the current source viewport; replacing the source may change its scientific content. Read status and images before refreshing; if detail_edited is true, local file/crop edits will be replaced only with discard_detail_edits:true. Missing members, ambiguous ownership or rotated panels must be resolved first. To delete a whole inset, remove its anchor_id once; this also removes its owned image/title and matching region/callout decorations, preserving the source image and other insets. Do not repeat cascaded IDs. Insets are frozen snapshots tied to the source revision; later source edits do not refresh them or move the region annotation automatically. Read the saved scene and its group membership before arrange. arrange requires arrangement and expected_revision and proposes alignment or equal edge-gap distribution of existing components without changing their relationships. Native groups, semantic anchors and owned labels/images move together; overlapping selections collapse to one unit. Distribution needs at least three independent units, uses equal edge gaps when space permits and the editor's anchored-center fallback when the span is too small. Exclude inset region/callout annotations. Native ink and unrounded zero-roughness polylines can be part of arranged components. A connector inside a selected component must translate rigidly with its bound endpoints; keep between-component connectors outside those groups. Rotated arrow-bound text and rough/curved/elbowed bounds still require native geometry. Locked members and unsupported independently moved curved/rotated/elbowed connectors must be resolved before arranging. Review crossings/overlaps visually; arrangement does not automatically reroute arrows. Use {op:duplicate,element_ids:[IDs],id_prefix,dx,dy} to copy existing native components as a pending proposal. Supply 1..100 explicit IDs and a fresh 1..64 ASCII alphanumeric/underscore/hyphen prefix. Copy expands groups, frame contents and bound/illustrated owners; new element IDs are prefix:e:oldId and group IDs prefix:g:oldGroup. Internal bindings and semantic ownership point to copies; external bindings and uncopied frame membership are detached. Original objects and image bytes remain unchanged. Locked members, annotations, inset snapshots, invalid/missing resources and identity collisions are rejected. Scientific claims, references and historical plans are copied unchanged, not validated for a new context; do not infer that repeated components prove a new experiment or relationship. Read the candidate and inspect its preview before adoption. To move existing components or several notes by one offset, use {op:translate,element_ids:[ids],dx:0,dy:79} in propose/revise_proposal. For exactly one unbound text/illustration label, prefer visualization_move_text with a single element_id and numeric dx/dy; it preserves the group and moves no image, anchor or connector. dx/dy are JSON numbers. This preserves IDs, source metadata, text, files and relative layout, expands complete native groups/frames/owned image-label components once, and adjusts supported attached connectors and labels. Locked/protected members or a connector that would deform are rejected atomically. Do not duplicate and delete objects to simulate movement. Use {op:sync_inset_region,id} on an inset anchor/image to reposition its region and straight callout around the frozen original source pixels after source transforms; this preserves the snapshot, title, reference and image bytes. Requires the original unchanged source file and its frozen region fully inside the current crop, unlocked decorations and an unrotated panel. Source file changes require an explicit refresh_inset instead. Preview and review before adoption; it does not enable automatic following. For an illustrated component whose connectors remain far from its visible art, use {op:fit_illustration_anchor,id,file_id,normalized_bounds:[x,y,width,height]} in propose/revise_proposal. Read the exact owned image file ID and inspect its pixels or SVG parts first; bounds are explicit fractions of the whole uncropped unflipped image, not biological identities. The operation fits the semantic anchor to the reviewed image bounds, preserving a relative layout slot for later asset replacements, keeps image/labels/files/scientific metadata, and adjusts local sharp orthogonal connector endpoints plus their labels while preserving bindings and arrowheads. It requires unlocked unrotated components and supported centered ports; curves, fixed ports, crops/flips, self-loops and protected annotations require native editing. A replacement invalidates the old fit; refit the new art before rerouting. Do not guess molecular structure or regenerate the figure. Always inspect the candidate PNG for collisions and visual attachment; this is not global obstacle routing. For uniform center-fixed resizing of an already fitted owned illustration, prefer visualization_scale_illustration with exact candidate ID/hash, element_id, file_id and numeric factor (1.1 means enlarge 10%). It reuses the reviewed fit and rejects missing/stale bounds; do not re-enter normalized_bounds or use native flip scale as a resize. For moving or resizing a standalone image, use {op:set_image_geometry,id,x:32.5,y:170,width:167,height:121} with real JSON numbers, not quoted strings in update.changes. It preserves the file, crop, angle, flip scale, source/license and every unrelated object. Width/height specify scene size; scale is only the native ±1 flip sign and must not be used as a fractional resize. Locked images, owned illustrations, inset members and images with bound objects require their component-specific/native operation. Read current image geometry and inspect the resulting child preview. Do not use multi-object center alignment to position illustrations under different stages; that aligns them to each other. For new standalone captions or scope notes, use {op:add_wrapped_text,id,text,x,y,max_width,font_size:16} rather than hand-writing native text JSON. It constructs all native fields with numeric values, scientific font 9, pinned wrapping and measured height; optionally supply frame_id only when the text must remain inside that live unlocked unrotated frame. Do not put a below-panel scope note inside a frame. This records the supplied text, not scientific validation. IDs must be fresh; read the candidate geometry before positioning and inspect its new preview. For long scientific labels, use {op:set_wrapped_text,id,text} to update text/originalText, wrapping, height and position together with pinned fontFamily 9 metrics. It preserves containers, other objects, styles, files and binding identities; uses native container widths and rejects overflowing shape/frame text rather than clipping or silently resizing containers. Optional max_width applies to unbound text or explicitly resized arrow captions; shape-bound text uses its container width. Optional font_size (4..200) explicitly changes the scientific font size and reflows atomically while preserving font family and line height; it never silently grows containers. Requires unrotated unlocked text; complex curves/elbows, unsupported glyphs/fonts and ambiguous bindings need native editing. Always inspect the candidate for collisions; wrapping is not global layout. For a local change of an existing relation, use {op:set_relation_direction,id,direction,inhibitory:false} in propose/revise_proposal. It updates arrowheads and explicit metadata together, preserving geometry, bindings, labels, evidence and original plan snapshots. Locked/deleted arrows, inset callouts, annotation arrows and opaque metadata are rejected. inhibitory must be explicitly supplied (true only for forward inhibition); do not silently discard inhibition when changing direction. Use {op:set_relation_evidence,id,expected_evidence,evidence:[references]} to replace only relation evidence. expected_evidence must exactly match the full current customData.evidence read from that scene (null when absent); never use the truncated inspector summary or silently discard structured references. Supply at most 100 nonblank references of at most 2000 characters, or [] to explicitly clear. This records supplied references without verifying their scientific validity and preserves other metadata and historical plans. Never invent citations. Read the current relation and scientific intent before changing it; this does not rewrite the saved scientific brief. Edges may set line_style:solid (default), dashed, or dotted. This is visual styling only: define any scientific meaning in labels/legend and retain evidence; dashed alone does not establish uncertainty or an optional mechanism. Edges may explicitly declare direction:forward (default), bidirectional, or undirected. Direction controls native arrowheads, including cross-panel connectors. inhibitory:true is supported only with forward direction; resolve conflicting scientific intent instead of discarding it. from/to identify layout endpoints even for undirected relations and do not establish causality. Do not infer biological absence, species exclusivity or lack of a mechanism from zero search hits, a 404, or a retrieval failure. Distinguish records actually read, records cited for depicted mechanisms, and records deliberately excluded from the figure. A scope exclusion is not a retrieval absence: before saying no relevant record was read, reconcile that claim with all successful reads, including out-of-scope and regulatory records. Prefer a figure-scope statement when no coverage claim is needed; a caveat such as not necessarily absent does not repair an inaccurate account of what was read. Do not add cross-stimulus/pathway mechanism details just to fill a component budget; retain a sourced abstract transition when finer detail was not requested or is context-dependent. Use figure_evidence to discover and read curated Reactome source records before supplying pathway evidence; inspect the relevant reactions and preserve species, scope and uncertainty rather than treating a search result or broad pathway URL as proof of each edge. For a family-level node, residue numbers in either node or relation labels must be explicitly assigned to each relevant isoform using the records actually read; a sequence of unlabeled site pairs or a general caption caveat does not establish that attribution. When site-specific detail is unnecessary or not established, keep the depicted activation state abstract and preserve the source evidence rather than inventing a mapping. Scientific claims require user review. GUI unsaved changes are not included; ask the user to save before editing them."
)]
pub struct FigureInput {
    #[desc = "save_plan only: true returns a compact receipt without echoing the complete submitted plan document. Retains version, status, validation issues, layout review and read_plan continuation. Defaults false for compatibility. Saved content and scientific scope are unchanged; use read_plan when the full document is needed."]
    pub compact_response: Option<bool>,
    #[desc = "For read/read_proposal only (expected_revision required): 1..20 distinct live object IDs copied verbatim. Returns exact selected_elements and bounded native geometry_context, omitting scene.document or proposal.document and unrelated object plans. Saved revision identity, candidate hashes and scene_geometry_review still cover the complete scene. Prefer for local edits; never use this projection as a replacement document. Omit for a full scene read."]
    #[schemars(length(min = 1, max = 20))]
    pub element_ids: Option<Vec<String>>,
    #[desc = "For sketch/sketch_plan targeting an existing nonempty figure only: true explicitly declares a user-requested whole-scene replacement. Defaults false. To preserve a reference image or existing objects, use propose/revise_proposal with insert_sketch instead. Never enable to bypass a preservation request."]
    pub replace_existing: Option<bool>,
    #[desc = "For revise_proposal only: explicitly create an alternative from a non-leaf parent when the user requested alternatives. Defaults false: continue the intended pending leaf. Never enable merely to bypass a stale-parent error."]
    pub allow_branch: Option<bool>,
    #[desc = "For candidate image_info, pair the exact document_sha256 with proposal_id and expected_revision; no title or operations. For revise_proposal: copy the parent candidate document_sha256 from read_proposal or read_proposal_preview; this is not its PNG hash. Requires proposal_id, expected_revision and title; exactly one operations or arrangement."]
    pub document_sha256: Option<String>,
    #[desc = "For read_proposal/read_proposal_preview/revise_proposal and candidate image_info: copy proposal_id from the proposal result ID. Requires figure_id and expected_revision for the current figure head; image is the immutable candidate, not the saved canvas."]
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
    #[desc = "For propose/arrange/revise_proposal: IDs of open review markups actually addressed by these edits. Read each with read_markup first; include only addressed reviews, including historical reviews reconciled with the current scene. Use a native JSON array, never an object. Omit this optional field when no reviews apply; [] explicitly drops parent links when revising. Does not resolve notes; the user chooses whether to close them when accepting the proposal."]
    pub markup_ids: Option<Vec<String>>,
    #[desc = "list_markups/list_proposals cursor: copy next_before from the previous page; omit for the newest page"]
    pub before: Option<String>,
    #[desc = "list_markups status filter: open or resolved; omit for all records"]
    pub status: Option<String>,
    #[desc = "list_markups/list_proposals page size, 1..100 (default 50/20)"]
    pub limit: Option<u32>,
    #[desc = "Expected planning version for save_plan/sketch_plan; use zero to create a brief, otherwise the version from read_plan"]
    pub planning_version: Option<i64>,
    #[desc = "Preferred typed scientific brief for save_plan. Use this instead of document: message, audience, layout, panels/nodes/edges/assumptions arrays, relationship_note, and questions:[{id,question,answer,blocking:true}]. blocking is persistent requiredness, not unresolved status: retain blocking:true after supplying an essential answer; only optional preferences use false. Preserve unanswered questions with empty answer strings. Native JSON arrays only; do not flatten questions into message text. Cannot be combined with document."]
    #[schemars(with = "Option<visualization::planning::Brief>")]
    pub planning: Option<Value>,
    #[desc = "Required for create/sketch; proposal summary required for propose. For sketch_plan only, omitted title uses the GUI default proposal summary; explicitly blank title is invalid."]
    pub title: Option<String>,
    #[desc = "For save_plan use planning instead; document is retained for older callers only. Legacy partial scientific brief {message,audience,layout,nodes,edges,assumptions,relationship_note,questions:[{id,question,answer,blocking:true}]}; defaults allow missing facts to remain explicit issues. Use native JSON arrays for nodes, edges, panels, assumptions and questions; never item wrappers or quoted arrays. Unanswered questions use answer as an empty string. Retain blocking:true for required questions after answering; false denotes optional preferences, not answered questions. For create: Excalidraw scene JSON. For sketch: plan {message,audience,layout:grid|horizontal|vertical,nodes:[{id,label,column,row,shape:rectangle|ellipse,evidence:[]}],edges:[{from,to,label,inhibitory,evidence:[]}],assumptions:[]}. Choose layout grid (default), horizontal, or vertical. Grid requires unique column/row positions; automatic horizontal/vertical layouts require omitting column and row and derive layers from explicit relations, including feedback routes. Node input order resolves cycle layout ties. Long labels wrap and node heights adapt in automatic layouts. Sketch compiles to separate editable shapes, labels and arrows."]
    #[schemars(with = "Option<DocumentSchema>")]
    pub document: Option<Value>,
    #[desc = "Exact current revision returned by read; required for edits, save_plan and sketch_plan"]
    pub expected_revision: Option<i64>,
    #[desc = "Native operations for propose/revise_proposal/apply. insert_sketch appends an explicit typed plan at top-left x/y with a fresh id_prefix, preserving all existing reference images/objects/files/settings/history. Use plan:{message,audience,layout,nodes,edges,assumptions} inside this operation, not document; it compiles native bound shapes/text/arrows and does not confirm a saved scientific plan. set_node_geometry explicitly moves/resizes one unrotated native rectangle/ellipse/diamond, reflows its complete bound scientific text and updates centered sharp connectors while preserving other nodes; illustrated anchors use set_illustration_geometry instead. Other operations: fit_text_width/set_node_geometry/translate/set_image_geometry/fit_illustration_anchor/add_wrapped_text/sync_inset_region/duplicate/add/update/remove/reroute/set_relation_direction/set_relation_evidence/set_wrapped_text. Updates are shallow merges and cannot change IDs."]
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
fn decode_document<T: serde::de::DeserializeOwned>(
    document: Value,
    root: &str,
) -> Result<T, ToolError> {
    // Internally tagged enum deserialization loses field paths inside variants.
    // Keep precise placement feedback without coercing quoted numbers.
    if root == "operations" && let Some(operations) = document.as_array() {
        for (index, operation) in operations.iter().enumerate() {
            if operation["op"] == "update" {
                let misplaced = operation.as_object().and_then(|object| object.keys().find(|key| !matches!(key.as_str(), "op" | "id" | "changes")));
                if let Some(field) = misplaced {
                    return Err(invalid(&format!("Invalid operations at operations[{index}].{field}: update native fields belong inside changes, never at the operation top level. Use {{\"op\":\"update\",\"id\":\"existing-id\",\"changes\":{{\"text\":\"complete text\",\"x\":120,\"y\":80}}}}. For scientific text wrapping use set_wrapped_text with id/text/font_size instead. Read the current candidate and retry the intended edit; nothing was saved.")));
                }
                if !operation.get("changes").is_some_and(Value::is_object) {
                    return Err(invalid(&format!("Invalid operations at operations[{index}].changes: update requires a JSON object of native fields, not an array, string or null. Use {{\"op\":\"update\",\"id\":\"existing-id\",\"changes\":{{\"x\":120,\"y\":80}}}}. Keep numeric values as JSON numbers. Read the current candidate and retry; nothing was saved.")));
                }
            }
            let positional = if operation["op"] == "translate" { Some((operation, vec!["dx", "dy"], "")) }
                else if operation["op"] == "update" { Some((&operation["changes"], visualization::scene::NUMERIC_UPDATE_FIELDS.to_vec(), ".changes")) } else { None };
            if let Some((value, fields, suffix)) = positional {
                for field in fields {
                    if let Some(value) = value.get(field) && !value.as_f64().is_some_and(f64::is_finite) {
                        return Err(invalid(&format!("Invalid operations at operations[{index}]{suffix}.{field}: expected a finite JSON number, not a quoted string, null or container. Retry with real JSON numbers, for example {{op:update,id:existing_text_id,changes:{{y:120}}}} for a label-only position; prefer visualization_move_text with one text element_id and top-level numeric dx/dy for an unbound text object. translate moves complete groups/components and must not replace a single grouped-label edit. For text reflow use set_wrapped_text with numeric font_size/max_width. Read the candidate and retry; nothing was saved.")));
                    }
                }
            }
            if operation["op"] == "set_node_geometry" {
                for field in ["x", "y", "width", "height"] {
                    if let Some(value) = operation.get(field) && !value.as_f64().is_some_and(f64::is_finite) {
                        return Err(invalid(&format!("Invalid operations at operations[{index}].{field}: native node geometry requires a finite JSON number. Use {{\"op\":\"set_node_geometry\",\"id\":\"existing-node\",\"x\":40,\"y\":100,\"width\":260,\"height\":120}} with dimensions read from the current candidate. Text reflows in the new bounds and centered sharp connectors remain attached; nothing was saved.")));
                    }
                }
            }
            if operation["op"] == "set_wrapped_text" || operation["op"] == "add_wrapped_text" {
                for field in ["font_size", "max_width"] {
                    if let Some(value)=operation.get(field) && !value.is_null() && !value.as_f64().is_some_and(f64::is_finite) {
                        return Err(invalid(&format!("Invalid operations at operations[{index}].{field}: expected a finite JSON number, not a quoted string or container. Use {{op:set_wrapped_text,id:existing_text_id,text:complete_text,font_size:18}} with a real number; max_width applies to unbound text or arrow captions, not shape-bound labels. Read the candidate and retry; nothing was saved.")));
                    }
                }
            }
            if operation["op"] == "fit_illustration_anchor" || operation["op"] == "set_illustration_geometry" {
                let bounds = operation.get("normalized_bounds").and_then(Value::as_array);
                if !bounds.is_some_and(|values| values.len() == 4 && values.iter().all(|value| value.as_f64().is_some_and(f64::is_finite))) {
                    return Err(invalid(&format!("Invalid operations at operations[{index}].normalized_bounds: expected exactly four finite JSON numbers in an array [x,y,width,height], for example [0.35,0.02,0.27,0.96]. Objects such as {{x:...,y:...}}, quoted arrays and quoted numbers are invalid. Read and preserve the reviewed bounds from the exact candidate anchor's customData.visualAnchorFit.normalizedBounds array, or inspect the source image to review new bounds; values are fractions of the whole uncropped, unflipped source image (SVG or PNG), not canvas pixels. Never replace reviewed bounds with the example values. Nothing was saved.")));
                }
            }
            if operation["op"] == "set_image_geometry" || operation["op"] == "set_illustration_geometry" {
                for field in ["x", "y", "width", "height"] {
                    if let Some(value) = operation.get(field) && !value.as_f64().is_some_and(f64::is_finite) {
                        return Err(invalid(&format!("Invalid operations at operations[{index}].{field}: expected a finite JSON number, not a quoted string, null or container. Read the image geometry and retry; nothing was saved.")));
                    }
                }
            }
        }
    }
    serde_path_to_error::deserialize(document).map_err(|error| {
        let path=error.path().to_string();
        let location=if path=="." || path.is_empty(){root.to_owned()}else if path.starts_with('['){format!("{root}{path}")}else{format!("{root}.{path}")};
        if root == "operations" {
            return invalid(&format!("Invalid operations at {location}: {}. Use native JSON types in each operation: numbers without quotes, booleans true/false without quotes, and arrays [...] rather than objects or quoted JSON. Match the selected op schema; do not switch operation kinds to bypass a parameter error. Read the current candidate before retrying; nothing was saved.", error.inner()));
        }
        invalid(&format!("Invalid {root} at {location}: {}. Array fields must be JSON arrays [...] (for example {{\"questions\":[{{\"id\":\"q1\",\"question\":\"Audience?\",\"answer\":\"\"}}]}}), not {{item:...}} objects or quoted JSON. Read the current document or candidate before retrying; nothing was saved.", error.inner()))
    })
}
fn check_sketch_replacement(service: &VisualizationService, project: &str, id: &str, revision: i64, allowed: bool) -> Result<(), ToolError> {
    let current = service.current(project, id).map_err(failure)?;
    if current.revision != revision { return Err(failure(visualization::Error::Conflict)); }
    let occupied = current.document["elements"].as_array().is_some_and(|items| items.iter().any(|e| e["isDeleted"] != true))
        || current.document["files"].as_object().is_some_and(|files| !files.is_empty());
    if occupied && !allowed {
        return Err(invalid("whole-scene sketch would replace existing objects or files; nothing was saved. To preserve references and existing content, use propose/revise_proposal with insert_sketch (plan, fresh id_prefix, numeric x/y). Set replace_existing:true only for an explicitly requested whole-scene replacement."));
    }
    Ok(())
}
fn execute(path: PathBuf, input: FigureInput) -> Result<Value, ToolError> {
    if input.compact_response.is_some() && input.action != "save_plan" {
        return Err(invalid("compact_response is only supported for save_plan"));
    }
    if let Some(ids) = &input.element_ids {
        if !matches!(input.action.as_str(), "read" | "read_proposal") {
            return Err(invalid("element_ids is only supported for read or read_proposal"));
        }
        scoped_read::validate(ids).map_err(|message| invalid(&message))?;
    }
    if input.replace_existing.is_some() && !matches!(input.action.as_str(), "sketch" | "sketch_plan") {
        return Err(invalid("replace_existing is only supported for sketch or sketch_plan"));
    }
    if input.allow_branch == Some(true) && input.action != "revise_proposal" {
        return Err(invalid("allow_branch is only supported for revise_proposal"));
    }
    let mut service = VisualizationService::new(Store::open(path).map_err(failure)?
        .with_linear_candidate_revisions(input.allow_branch != Some(true)));
    if input.planning.is_some() && input.action != "save_plan" {
        return Err(invalid("planning is only supported for save_plan"));
    }
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
            let plan: visualization::sketch::Plan = decode_document(document, "document")?;
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
            check_sketch_replacement(&service, &project, &id, revision, input.replace_existing == Some(true))?;
            let mut proposal = json!(
                service
                    .store()
                    .propose(&project, &id, revision, &title, &document)
                    .map_err(failure)?
            );
            proposal["scene_geometry_review"] = json!(visualization::geometry_review::review(&proposal["document"]));
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
            let brief = match (input.planning, input.document) {
                (Some(planning), None) => decode_document(planning, "planning")?,
                (None, Some(document)) => decode_document(document, "document")?,
                (Some(_), Some(_)) => {
                    return Err(invalid("send planning or legacy document, not both"));
                }
                (None, None) => return Err(invalid("planning is required for save_plan")),
            };
            let saved = service.store_mut().save_planning(&project, &id, revision, version, &brief).map_err(failure)?;
            let mut result = json!(&saved);
            if input.compact_response == Some(true) {
                result.as_object_mut().unwrap().remove("document");
                result["document_omitted"] = json!(true);
                result["plan_counts"] = json!({"nodes":saved.document.nodes.len(),"edges":saved.document.edges.len(),"panels":saved.document.panels.len(),"questions":saved.document.questions.len(),"assumptions":saved.document.assumptions.len()});
            }
            Ok(result)
        }
        "sketch_plan" => {
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            let version = input
                .planning_version
                .ok_or_else(|| invalid("planning_version is required"))?;
            let summary = required(Some(input.title.unwrap_or_else(|| "根据绘图计划生成草图".into())), "planning proposal summary cannot be blank")?;
            check_sketch_replacement(&service, &project, &id, revision, input.replace_existing == Some(true))?;
            let mut proposal = json!(
                service
                    .store_mut()
                    .planned_proposal(&project, &id, revision, version, &summary)
                    .map_err(failure)?
            );
            proposal["scene_geometry_review"] = json!(visualization::geometry_review::review(&proposal["document"]));
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
            if input.element_ids.is_some() && input.expected_revision.is_none() {
                return Err(invalid("expected_revision is required for scoped read"));
            }
            if input.expected_revision.is_some_and(|revision| revision != metadata.revision) {
                return Err(failure(visualization::Error::Conflict));
            }
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
            let planning=service.store().planning(&project,&id).map_err(failure)?.map(|p|json!({"version":p.version,"based_on_revision":p.based_on_revision,"status":p.status,"issue_count":p.issues.len(),"graph_review":p.graph_review,"layout_review":p.layout_review,"proposal_id":p.proposal_id}));
            let result = json!({"metadata":metadata,"scene_geometry_review":visualization::geometry_review::review(&scene.document),"scene":scene,"image_data_omitted":true,"proposal_summary":service.store().pending_proposal_summary(&project,&id).map_err(failure)?,"annotations":service.store().annotations(&project,&id).map_err(failure)?,"markups":markups.items,"markups_pagination":{"next_before":markups.next_before},"planning":planning});
            if let Some(ids) = &input.element_ids {
                scoped_read::project_saved(result, ids).map_err(|message| invalid(&message))
            } else {
                Ok(result)
            }
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
            proposal["scene_geometry_review"] = json!(visualization::geometry_review::review(&proposal["document"]));
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
                proposal["scene_geometry_review"] = json!(visualization::geometry_review::review(&proposal["document"]));
                proposal.as_object_mut().unwrap().remove("document");
                Ok(proposal)
            }
        }
        "image_info" | "inset" => {
            let revision = input
                .expected_revision
                .ok_or_else(|| invalid("expected_revision is required"))?;
            if input.action == "inset" {
                if input.proposal_id.is_some() || input.document_sha256.is_some() {
                    return Err(invalid("inset requires a saved revision; candidate arguments are not supported"));
                }
                let spec = input.inset.ok_or_else(|| invalid("inset is required"))?;
                Ok(json!(
                    visualization_assets::inset(service.store(), &project, &id, revision, &spec)
                        .map_err(failure)?
                ))
            } else {
                let target = required(input.element_id, "element_id is required")?;
                let mut info = match (input.proposal_id.as_deref(),input.document_sha256.as_deref()) {
                    (None,None) => visualization_assets::image_info(service.store(),&project,&id,revision,&target).map_err(failure)?,
                    (Some(proposal_id),Some(hash)) if !proposal_id.trim().is_empty() && !hash.trim().is_empty() => {
                        if service.store().get(&project,&id).map_err(failure)?.revision != revision {
                            return Err(failure(visualization::Error::Conflict));
                        }
                        let proposal=service.store().proposal(&project,&id,proposal_id).map_err(failure)?;
                        if proposal.status!="pending" || proposal.base_revision!=revision || proposal.document_sha256!=hash {
                            return Err(invalid("image_info candidate requires exact hash, pending status and current base revision"));
                        }
                        let mut value=visualization_assets::image_info_document(&proposal.document,&target).map_err(failure)?;
                        value["revision"]=json!(revision);
                        value["current_revision"]=json!(revision);
                        value["base_revision"]=json!(proposal.base_revision);
                        value["visualization_id"]=json!(proposal.visualization_id);
                        value["proposal_id"]=json!(proposal.id);
                        value["document_sha256"]=json!(proposal.document_sha256);
                        value["scope"]=json!("proposal_candidate");
                        value
                    },
                    _ => return Err(invalid("image_info requires both proposal_id and document_sha256 for a candidate")),
                };
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
                    .map(|operations| {
                        decode_document::<Vec<visualization::scene::Operation>>(
                            operations,
                            "operations",
                        )
                    })
                    .transpose()?;
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
                result["scene_geometry_review"] = json!(visualization::geometry_review::review(&result["document"]));
                result["materialization_checkpoint"] = visualization::materialization_review::review(&result["document"]);
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
                if let Some(ids) = &input.element_ids {
                    return scoped_read::project(serde_json::to_value(&proposal).map_err(failure)?, current, ids).map_err(|message| invalid(&message));
                }
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
                    json!({"materialization_checkpoint":visualization::materialization_review::review(&proposal.document),"scene_geometry_review":visualization::geometry_review::review(&proposal.document),"proposal":proposal,"current_revision":current,"base_is_current":proposal.base_revision==current,"image_data_omitted":true}),
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
            let operations: Vec<visualization::scene::Operation> = decode_document(
                input
                    .operations
                    .ok_or_else(|| invalid("operations is required"))?,
                "operations",
            )?;
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
                proposal["scene_geometry_review"] = json!(visualization::geometry_review::review(&proposal["document"]));
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
    async fn execute(&self, input: Value) -> Result<ToolResult, ToolError> {
        let typed = serde_path_to_error::deserialize(input).map_err(|error| {
            let hint = if error.path().to_string() == "markup_ids" {
                "markup_ids requires a JSON array of review IDs, not {} or an item wrapper. Omit it when no reviews apply; [] explicitly drops inherited links, never resolves the reviews."
            } else {
                "Use the advertised JSON field types; arrays are [...], not item objects or quoted JSON."
            };
            invalid(&format!("Invalid input at input.{}: {}. {hint} Nothing was saved.",error.path(),error.inner()))
        })?;
        self.run(typed).await
    }
    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let scope = (input.project_id.clone(), input.figure_id.clone());
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
            let path = self.path.clone();
            let document = tokio::task::spawn_blocking(move || {
                let store = Store::open(path).map_err(failure)?;
                let candidate = store.proposal(&project, &figure, &proposal).map_err(failure)?;
                Ok::<Value, ToolError>(candidate.document)
            }).await.map_err(failure)??;
            value["materialization_checkpoint"] = visualization::materialization_review::review(&document);
            value["scene_geometry_review"] = json!(visualization::geometry_review::review(&document));
        }
        follow_up::attach(&mut value, scope.0.as_deref(), scope.1.as_deref());
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
    vec![ToolRegistration::from(FigureTool { path: path.clone() }), fit::registration(path.clone()), fit_svg::registration(path.clone()), fit_svg_group::registration(path.clone()), translate::registration(path.clone()), add_text::registration(path.clone()), scale::registration(path.clone()), crop::registration(path.clone()), move_text::registration(path.clone()), plan_label::registration(path.clone()), plan_label::batch_registration(path.clone()), find_space::registration(path.clone()), reflow_text::registration(path)]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fit_bounds_errors_identify_the_field_without_coercion() {
        for bounds in [json!({"x":0.1,"y":0.2,"width":0.3,"height":0.4}),json!([0.1,0.2,0.3]),json!(["0.1",0.2,0.3,0.4]),json!("[0.1,0.2,0.3,0.4]")] {
            let error=decode_document::<Vec<visualization::scene::Operation>>(json!([{"op":"fit_illustration_anchor","id":"a","file_id":"f","normalized_bounds":bounds}]),"operations").unwrap_err().to_string();
            assert!(error.contains("operations[0].normalized_bounds"));
            assert!(error.contains("[x,y,width,height]"));
            assert!(!error.contains("questions"));
        }
        let operations=decode_document::<Vec<visualization::scene::Operation>>(json!([{"op":"fit_illustration_anchor","id":"a","file_id":"f","normalized_bounds":[0.1,0.2,0.3,0.4]}]),"operations").unwrap();
        assert_eq!(operations.len(),1);
    }
    #[test]
    fn text_reflow_number_errors_identify_the_precise_optional_field(){
        for op in ["set_wrapped_text", "add_wrapped_text"] { for field in ["font_size", "max_width"] {
            let mut value=json!({"op":op,"id":"label","text":"Full label","font_size":18,"max_width":200});
            if op=="add_wrapped_text" {value["x"]=json!(0);value["y"]=json!(0);}
            value[field]=json!("18");
            let error=decode_document::<Vec<visualization::scene::Operation>>(json!([value]),"operations").unwrap_err().to_string();
            assert!(error.contains(&format!("operations[0].{field}")));assert!(!error.contains("For translate"));
        }}
        assert!(decode_document::<Vec<visualization::scene::Operation>>(json!([{"op":"set_wrapped_text","id":"label","text":"Full label","font_size":null,"max_width":null}]),"operations").is_ok());
    }
    #[test]
    fn illustration_geometry_errors_identify_bounds_and_numeric_fields() {
        for bounds in [json!(null),json!({"x":0,"y":0,"width":1,"height":1}),json!([0,0,1]),json!([0,0,"1",1])] {
            let error=decode_document::<Vec<visualization::scene::Operation>>(json!([{"op":"set_illustration_geometry","id":"a","file_id":"f","x":1,"y":2,"width":3,"height":4,"normalized_bounds":bounds}]),"operations").unwrap_err().to_string();
            assert!(error.contains("operations[0].normalized_bounds"));
            assert!(error.contains("[x,y,width,height]"));
            assert!(!error.contains("For translate"));
        }
        for field in ["x","y","width","height"] {
            let mut op=json!({"op":"set_illustration_geometry","id":"a","file_id":"f","x":1,"y":2,"width":3,"height":4,"normalized_bounds":[0,0,1,1]});
            op[field]=json!("3");
            let error=decode_document::<Vec<visualization::scene::Operation>>(json!([op]),"operations").unwrap_err().to_string();
            assert!(error.contains(&format!("operations[0].{field}")));
        }
        let valid=json!([{"op":"set_illustration_geometry","id":"a","file_id":"f","x":1,"y":2,"width":3,"height":4,"normalized_bounds":[0,0,1,1]}]);
        assert_eq!(decode_document::<Vec<visualization::scene::Operation>>(valid,"operations").unwrap().len(),1);
    }
    #[test]
    fn operation_type_errors_use_edit_examples_without_accepting_quoted_booleans() {
        let error=decode_document::<Vec<visualization::scene::Operation>>(json!([{"op":"set_relation_direction","id":"a","direction":"forward","inhibitory":"false"}]),"operations").unwrap_err().to_string();
        assert!(error.contains("expected a boolean"));
        assert!(error.contains("booleans true/false without quotes"));
        assert!(!error.contains("questions"));
    }
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
                &json!({"elements":[{"id":"candidate","type":"rectangle","customData":{"scientificId":"protein"}}]}),
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
        assert_eq!(caption["follow_up_inputs"]["read_proposal_preview"], input);
        assert!(caption.get("dataURL").is_none());
        assert_eq!(caption["materialization_checkpoint"]["total"], 1);
        assert_eq!(caption["materialization_checkpoint"]["counts"]["missing"], 1);
        assert_eq!(caption["materialization_checkpoint"]["components"][0]["anchor_id"], "candidate");
        assert_eq!(caption["materialization_checkpoint"]["scientific_validity_verified"], false);
        assert!(caption["scene_geometry_review"]["scope"].as_str().unwrap().contains("excluding_strokes"));
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
        assert_eq!(caption["candidate_context"]["base_revision"], 1);
        assert_eq!(caption["follow_up_inputs"]["read_proposal_preview"]["expected_revision"], 2);
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
        let ready = serde_json::to_value(tool.implementation.execute(save.clone()).await.unwrap()).unwrap();
        assert_eq!(ready["content"]["graph_review"]["weak_components"], json!([["cell"]]));
        assert_eq!(ready["content"]["graph_review"]["isolated_node_ids"], json!(["cell"]));
        assert_eq!(ready["content"]["graph_review"]["verification"], "not_verified");
        let summary = serde_json::to_value(tool.implementation.execute(json!({"action":"read","project_id":"p","figure_id":figure.id})).await.unwrap()).unwrap();
        assert_eq!(summary["content"]["planning"]["graph_review"], ready["content"]["graph_review"]);
        assert_eq!(ready["content"]["layout_review"]["status"], "available");
        assert!(ready["content"]["layout_review"]["bounds"]["width"].as_f64().unwrap() > 0.);
        assert_eq!(summary["content"]["planning"]["layout_review"], ready["content"]["layout_review"]);
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
        let revised=serde_json::to_value(tool.implementation.execute(json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"expected_revision":1,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"title":"Explicit inhibition","operations":[{"op":"set_relation_direction","id":"edge-0","direction":"forward","inhibitory":true},{"op":"set_relation_evidence","id":"edge-0","expected_evidence":["fixture-source"],"evidence":["user-reviewed-source"]},{"op":"duplicate","element_ids":["edge-0"],"id_prefix":"repeated","dx":0,"dy":100}]})).await.unwrap()).unwrap();
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
        let repeated = child.document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "repeated:e:edge-0")
            .unwrap();
        assert_eq!(repeated["endArrowhead"], "bar");
        assert_eq!(repeated["startBinding"], Value::Null);
        assert_eq!(
            repeated["customData"]["evidence"],
            json!(["user-reviewed-source"])
        );

        assert_eq!(
            edge["customData"]["evidence"],
            json!(["user-reviewed-source"])
        );
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
        let proposal = tool.implementation.execute(json!({"action":"propose","project_id":"p","figure_id":id,"expected_revision":1,"title":"Clarify ligand","operations":[{"op":"update","id":"label-0","changes":{"text":"Ligand A"}},{"op":"add_wrapped_text","id":"scope-caption","text":"Reduced schematic; supplied sources require review.","x":0,"y":600,"max_width":500,"font_size":16}]})).await.unwrap();
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
        let caption = after.document["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|element| element["id"] == "scope-caption")
            .unwrap();
        assert_eq!(caption["fontFamily"], 9);
        assert_eq!(caption["fontSize"], 16.);
        assert_eq!(
            caption["originalText"],
            "Reduced schematic; supplied sources require review."
        );
        assert!(caption["height"].as_f64().unwrap() > 0.);
        assert_eq!(caption["boundElements"], json!([]));
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
            compact_response: None,
            element_ids: None,
            replace_existing: None,
            allow_branch: None,
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
            planning: None,
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
    #[tokio::test]
    async fn planning_errors_identify_exact_fields_without_saving_partial_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("planning-errors.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let figure = store
            .create(
                "p",
                "Figure",
                "excalidraw/v2",
                &json!({"elements":[],"files":{}}),
            )
            .unwrap();
        let tool = registrations(path.clone()).into_iter().find(|tool| tool.definition.name == "visualization").unwrap();
        for (document, field) in [
            (
                json!({"assumptions":{"item":["Conceptual"]}}),
                "document.assumptions",
            ),
            (
                json!({"questions":[{"id":"q","question":"Audience?","answer":7}]}),
                "document.questions[0].answer",
            ),
            (json!({"nodes":"[]"}), "document.nodes"),
        ] {
            let result=tool.implementation.execute(json!({"action":"save_plan","project_id":"p","figure_id":figure.id,"expected_revision":1,"planning_version":0,"document":document})).await;
            let error = result.expect_err("invalid input must fail").to_string();
            assert!(error.contains(field), "{error}");
            assert!(error.contains("nothing was saved"));
            assert!(store.planning("p", &figure.id).unwrap().is_none());
            assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
        }
    }
    #[tokio::test]
    async fn typed_planning_argument_preserves_questions_and_rejects_ambiguous_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("typed-planning.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let figure = store
            .create(
                "p",
                "Figure",
                "excalidraw/v2",
                &json!({"elements":[],"files":{}}),
            )
            .unwrap();
        let tool = registrations(path).remove(0);
        let base = json!({"action":"save_plan","project_id":"p","figure_id":figure.id,"expected_revision":1,"planning_version":0,"planning":{"message":"Receptor schematic","questions":[{"id":"audience","question":"Audience?","answer":""}]}});
        tool.implementation.execute(base.clone()).await.unwrap();
        let saved = store.planning("p", &figure.id).unwrap().unwrap();
        assert_eq!(saved.document.questions.len(), 1);
        assert_eq!(saved.document.questions[0].answer, "");
        assert_eq!(saved.status, "needs_clarification");
        for bad in [
            json!({"action":"save_plan","project_id":"p","figure_id":figure.id,"expected_revision":1,"planning_version":1,"planning":{"message":"Ambiguous"},"document":{"message":"Different"}}),
            json!({"action":"read_plan","project_id":"p","figure_id":figure.id,"planning":{}}),
        ] {
            assert!(tool.implementation.execute(bad).await.is_err());
            assert_eq!(store.planning("p", &figure.id).unwrap().unwrap().version, 1);
        }
        let schema = serde_json::to_value(tool.definition.input_schema).unwrap();
        let planning = &schema["properties"]["planning"];
        assert!(
            planning["type"]
                .as_array()
                .unwrap()
                .contains(&json!("object"))
        );
        assert_eq!(planning["properties"]["questions"]["type"], "array");
        assert!(planning.get("anyOf").is_none());
    }
    #[tokio::test]
    async fn empty_review_array_errors_are_actionable_and_never_drop_links_implicitly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review-array.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.create_project("p", "Study").unwrap();
        let document = json!({"elements":[{"id":"node","type":"rectangle","x":0,"y":0,"width":100,"height":60}],"files":{}});
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &document)
            .unwrap();
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        let review = store
            .create_markup(
                "p",
                &figure.id,
                1,
                "Review this component",
                &[visualization::markup::Stroke {
                    kind: visualization::markup::Kind::Emphasis,
                    points: vec![[0., 0.], [50., 30.]],
                }],
                png,
            )
            .unwrap();
        let parent = store
            .propose_with_markups(
                "p",
                &figure.id,
                1,
                "Parent",
                &document,
                std::slice::from_ref(&review.id),
            )
            .unwrap();
        let tool = registrations(path).remove(0);
        let base = json!({"action":"revise_proposal","project_id":"p","figure_id":figure.id,"expected_revision":1,"proposal_id":parent.id,"document_sha256":parent.document_sha256,"title":"Refine style","operations":[{"op":"update","id":"node","changes":{"backgroundColor":"#e0f2fe"}}]});
        for invalid_ids in [json!({}), json!({"item":[]}), json!("")] {
            let mut request = base.clone();
            request["markup_ids"] = invalid_ids;
            let error = tool
                .implementation
                .execute(request)
                .await
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains("input.markup_ids"), "{error}");
            assert!(error.contains("[] explicitly drops inherited links"));
            assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
        }
        assert!(
            tool.implementation.execute(base.clone()).await.is_err(),
            "omitting markup_ids cannot silently drop linked reviews"
        );
        let mut broken = base.clone();
        broken["markup_ids"] = json!([]);
        broken["operations"] = json!([{"op":"duplicate","element_ids":{"item":["node"]},"id_prefix":"copy","dx":10,"dy":10}]);
        let error = tool
            .implementation
            .execute(broken)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("operations[0]"), "{error}");
        assert_eq!(store.proposals("p", &figure.id).unwrap().len(), 1);
        let mut explicit = base.clone();
        explicit["markup_ids"] = json!([]);
        let result =
            serde_json::to_value(tool.implementation.execute(explicit).await.unwrap()).unwrap();
        let child = store
            .proposal("p", &figure.id, result["content"]["id"].as_str().unwrap())
            .unwrap();
        assert!(child.review_markups.is_empty());
        assert_eq!(child.document["elements"][0]["backgroundColor"], "#e0f2fe");
        let mut keep = base;
        // Compare explicit review-link alternatives from the same immutable parent.
        keep["allow_branch"] = json!(true);
        keep["markup_ids"] = json!([review.id]);
        let result =
            serde_json::to_value(tool.implementation.execute(keep).await.unwrap()).unwrap();
        assert_eq!(result["content"]["review_markups"][0]["id"], review.id);
        assert_eq!(
            store
                .proposal("p", &figure.id, &parent.id)
                .unwrap()
                .document,
            parent.document
        );
        assert_eq!(
            store
                .proposal("p", &figure.id, &parent.id)
                .unwrap()
                .review_markups
                .len(),
            1
        );
        assert_eq!(
            store
                .markup("p", &figure.id, &review.id)
                .unwrap()
                .summary
                .status,
            "open"
        );
        assert_eq!(
            store.revision("p", &figure.id, 1).unwrap().document,
            document
        );
        assert_eq!(store.get("p", &figure.id).unwrap().revision, 1);
    }
}
