//! Index / tree tools (10): create / delete / move-index / move-children /
//! local / subtree-knowledge / search-subtree / link-orphans /
//! detach-knowledge / merge-subtree.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use kms::{KmsService, TargetType};
use serde_json::json;

fn svc_err(e: String) -> ToolError {
    ToolError::ExecutionFailed { source: e.into() }
}

// ═══════════════════════════════════════════════════════════════════
// kms_create_index
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_create_index",
    description = "Create an index entry under a parent index. Use parent_ref as an absolute \
                   path (e.g. '/编程语言/Python') or plain title. Use '/' for the root."
)]
pub struct CreateIndexInput {
    #[desc = "ABSOLUTE PATH of the parent index — use '/' for the root"]
    pub parent_ref: String,
    #[desc = "Title of this index entry"]
    pub title: String,
    #[desc = "Name of knowledge to reference (optional, for knowledge-type index)"]
    pub target_ref: Option<String>,
    #[desc = "'knowledge' if linking to a knowledge entry (optional)"]
    pub target_type: Option<String>,
}

pub struct KmsCreateIndexTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsCreateIndexTool {
    type Input = CreateIndexInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let target_type = input.target_type.as_deref().map(|tt| match tt {
            "knowledge" => TargetType::Knowledge,
            _ => TargetType::Group,
        });
        self.svc
            .create_index_by_ref(&input.parent_ref, Some(input.title.clone()), input.target_ref.as_deref(), target_type)
            .await
            .map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({ "title": input.title })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_delete_index
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_delete_index",
    description = "Delete an EMPTY Group-type index node by its title. Refuses to delete \
                   non-empty indexes, knowledge mounts, or the root."
)]
pub struct DeleteIndexInput {
    #[desc = "Title of the (empty, group-type) index to delete"]
    pub title: String,
}

pub struct KmsDeleteIndexTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsDeleteIndexTool {
    type Input = DeleteIndexInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        self.svc.delete_index(&input.title).await.map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({ "deleted": input.title })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_move_index
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_move_index",
    description = "Move an index node (and its entire subtree) to a new parent. Both paths \
                   accept absolute path syntax (starts with '/')."
)]
pub struct MoveIndexInput {
    #[desc = "ABSOLUTE PATH of the index to move"]
    pub index_path: String,
    #[desc = "ABSOLUTE PATH of the new parent"]
    pub new_parent_path: String,
}

pub struct KmsMoveIndexTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsMoveIndexTool {
    type Input = MoveIndexInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let result = self
            .svc
            .move_index(&input.index_path, &input.new_parent_path)
            .await
            .map_err(svc_err)?;
        Ok(ToolResult::success(result))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_move_children
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_move_children",
    description = "Move named child indices from source_path into a group index mounted under \
                   remount_path. The destination group is find-or-create."
)]
pub struct MoveChildrenInput {
    #[desc = "ABSOLUTE PATH of the node whose children should be moved"]
    pub source_path: String,
    #[desc = "ABSOLUTE PATH of the node under which the destination group is mounted"]
    pub remount_path: String,
    #[desc = "Title of the destination group (find-or-create)"]
    pub new_group_title: String,
    #[desc = "Titles (NOT paths) of child indexes to move under the destination group"]
    pub child_titles: Vec<String>,
}

pub struct KmsMoveChildrenTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsMoveChildrenTool {
    type Input = MoveChildrenInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let result = self
            .svc
            .move_children(
                &input.source_path,
                &input.remount_path,
                &input.new_group_title,
                &input.child_titles,
            )
            .await
            .map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({
            "location": result.location,
            "new_group_id": result.new_group_id.to_string(),
            "group_created": result.group_created,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_local
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_local",
    description = "Stateless: fetch a structured local view of any node in the index tree. \
                   Returns node metadata, ancestor path, direct children, sibling count, and \
                   subtree summary. Does NOT modify the pointer."
)]
pub struct LocalInput {
    #[desc = "Absolute or relative path. Defaults to '/' (root)."]
    pub path: Option<String>,
}

pub struct KmsLocalTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsLocalTool {
    type Input = LocalInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let path = input.path.as_deref().unwrap_or("/");
        let view = self.svc.get_local_view_by_path(path).await.map_err(svc_err)?;

        let path_titles: Vec<_> = view
            .path
            .iter()
            .map(|n| n.title.clone().unwrap_or_else(|| "(unnamed)".into()))
            .collect();

        let children: Vec<_> = view
            .children
            .iter()
            .map(|c| {
                let kind = match c.target_type {
                    TargetType::Knowledge => "knowledge",
                    TargetType::Group => "group",
                };
                json!({
                    "id": c.id.to_string(),
                    "title": c.title,
                    "type": kind,
                    "position": c.position,
                })
            })
            .collect();

        Ok(ToolResult::success_json(json!({
            "path_resolved": path_titles,
            "node": {
                "id": view.node.id.to_string(),
                "title": view.node.title,
                "type": match view.node.target_type {
                    TargetType::Knowledge => "knowledge",
                    TargetType::Group => "group",
                },
            },
            "sibling_count": view.sibling_count,
            "children": children,
            "subtree": {
                "total_nodes": view.subtree_summary.total_nodes,
                "knowledge_count": view.subtree_summary.knowledge_count,
                "group_count": view.subtree_summary.group_count,
                "max_depth": view.subtree_summary.max_depth,
                "knowledge_titles": view.subtree_summary.knowledge_titles,
                "truncated": view.subtree_summary.truncated,
            }
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_subtree_knowledge
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_subtree_knowledge",
    description = "Stateless: list every knowledge entry inside the subtree rooted at `path`."
)]
pub struct SubtreeKnowledgeInput {
    #[desc = "Absolute path of the subtree root"]
    pub path: String,
}

pub struct KmsSubtreeKnowledgeTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsSubtreeKnowledgeTool {
    type Input = SubtreeKnowledgeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let knowledge = self
            .svc
            .get_subtree_knowledge_by_path(&input.path)
            .await
            .map_err(svc_err)?;

        let results: Vec<_> = knowledge
            .iter()
            .map(|k| {
                json!({
                    "title": k.title,
                    "knowledge_type": k.knowledge_type.as_str(),
                    "content": k.content,
                })
            })
            .collect();

        Ok(ToolResult::success_json(json!({
            "count": results.len(),
            "knowledges": results,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_search_subtree
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_search_subtree",
    description = "Stateless: search the subtree rooted at `path` for knowledge entries whose \
                   TITLE contains the keyword (case-insensitive)."
)]
pub struct SearchSubtreeInput {
    #[desc = "Absolute path of the subtree root"]
    pub path: String,
    #[desc = "Substring to search for in knowledge titles (case-insensitive)"]
    pub keyword: String,
}

pub struct KmsSearchSubtreeTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsSearchSubtreeTool {
    type Input = SearchSubtreeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let node_id = self
            .svc
            .get_local_view_by_path(&input.path)
            .await
            .map_err(svc_err)?
            .node
            .id;
        let knowledge = self
            .svc
            .search_knowledge_titles(node_id, &input.keyword)
            .await
            .map_err(svc_err)?;

        let results: Vec<_> = knowledge
            .iter()
            .map(|k| {
                json!({
                    "title": k.title,
                    "knowledge_type": k.knowledge_type.as_str(),
                    "content": k.content,
                })
            })
            .collect();

        Ok(ToolResult::success_json(json!({
            "count": results.len(),
            "results": results,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_link_orphans
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_link_orphans",
    description = "Batch-link orphan knowledge entries under a parent index. Each knowledge \
                   title becomes a knowledge-type index child."
)]
pub struct LinkOrphansInput {
    #[desc = "Title of the parent index node to link orphans under"]
    pub parent_ref: String,
    #[desc = "Array of orphan knowledge titles to link"]
    pub knowledge_titles: Vec<String>,
}

pub struct KmsLinkOrphansTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsLinkOrphansTool {
    type Input = LinkOrphansInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if input.knowledge_titles.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "knowledge_titles must not be empty".into(),
            });
        }
        let refs: Vec<&str> = input.knowledge_titles.iter().map(|s| s.as_str()).collect();
        let linked = self
            .svc
            .link_orphans(&input.parent_ref, &refs)
            .await
            .map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({
            "linked": linked,
            "count": linked.len(),
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_detach_knowledge
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_detach_knowledge",
    description = "Temporarily UNMOUNT a Knowledge from the index tree. The Knowledge row is \
                   preserved as an ORPHAN. You MUST re-link it via kms_link_orphans before \
                   ending your turn."
)]
pub struct DetachKnowledgeInput {
    #[desc = "Title of the knowledge-typed Index node to detach"]
    pub title: String,
}

pub struct KmsDetachKnowledgeTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsDetachKnowledgeTool {
    type Input = DetachKnowledgeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let knowledge_id = self
            .svc
            .detach_knowledge_index(&input.title)
            .await
            .map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({
            "detached_title": input.title,
            "orphan_knowledge_id": knowledge_id.to_string(),
            "warning": "Knowledge is now ORPHAN. Re-link via kms_link_orphans before ending your turn.",
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_merge_subtree
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_merge_subtree",
    description = "Merge a staging sub-tree into a target parent. All children of the staging \
                   node are reparented under the target, then the staging node is deleted."
)]
pub struct MergeSubtreeInput {
    #[desc = "Title of the staging sub-root node"]
    pub sub_root_title: String,
    #[desc = "Title of the target parent in the main tree"]
    pub target_parent_title: String,
}

pub struct KmsMergeSubtreeTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsMergeSubtreeTool {
    type Input = MergeSubtreeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let sub_root_id = self
            .svc
            .resolve_index(&input.sub_root_title)
            .await
            .map_err(svc_err)?;
        let target_parent_id = self
            .svc
            .resolve_index(&input.target_parent_title)
            .await
            .map_err(svc_err)?;
        let moved = self
            .svc
            .merge_subtree(sub_root_id, target_parent_id)
            .await
            .map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({
            "sub_root": input.sub_root_title,
            "target_parent": input.target_parent_title,
            "moved_children": moved,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// Registration
// ═══════════════════════════════════════════════════════════════════

pub fn registrations(svc: Arc<KmsService>) -> Vec<ToolRegistration> {
    vec![
        KmsCreateIndexTool { svc: svc.clone() }.into(),
        KmsDeleteIndexTool { svc: svc.clone() }.into(),
        KmsMoveIndexTool { svc: svc.clone() }.into(),
        KmsMoveChildrenTool { svc: svc.clone() }.into(),
        KmsLocalTool { svc: svc.clone() }.into(),
        KmsSubtreeKnowledgeTool { svc: svc.clone() }.into(),
        KmsSearchSubtreeTool { svc: svc.clone() }.into(),
        KmsLinkOrphansTool { svc: svc.clone() }.into(),
        KmsDetachKnowledgeTool { svc: svc.clone() }.into(),
        KmsMergeSubtreeTool { svc }.into(),
    ]
}
