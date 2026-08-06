//! Knowledge tools (8): create / update / delete / get / batch / entity-knowledge /
//! rename / search-content.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use kms::{KnowledgeType, KmsService};
use serde_json::json;

fn svc_err(e: String) -> ToolError {
    ToolError::ExecutionFailed { source: e.into() }
}

fn knowledge_type_from_str(s: &str) -> KnowledgeType {
    match s {
        "relation" => KnowledgeType::Relation,
        _ => KnowledgeType::Aspect,
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_create_knowledge
// ═══════════════════════════════════════════════════════════════════

const VAGUE_TITLE_SUFFIXES: &[&str] = &[
    "概述", "总结", "小结", "定义", "简介", "说明", "介绍", "基本概念", "特征",
];

fn validate_knowledge_title(title: &str) -> Result<(), ToolError> {
    let suffix = title.split(" · ").nth(1).unwrap_or(title);
    for &keyword in VAGUE_TITLE_SUFFIXES {
        if suffix.contains(keyword) {
            return Err(ToolError::ValidationFailed {
                message: format!(
                    "标题 \"{title}\" 的切面描述包含模糊词汇 \"{keyword}\"。\
                     切面描述必须是具体的方面。"
                ),
            });
        }
    }
    Ok(())
}

#[tool(
    name = "kms_create_knowledge",
    description = "Create a knowledge entry about an entity or entities. Knowledge can be 'aspect' \
                   (about one entity) or 'relation' (between multiple entities). Content uses \
                   [[entity name]] wiki-style links."
)]
pub struct CreateKnowledgeInput {
    #[desc = "Title of the knowledge entry (format: 'Entity · Aspect')"]
    pub title: String,
    #[desc = "'aspect' or 'relation'"]
    pub knowledge_type: String,
    #[desc = "Array of all entity names mentioned in the content"]
    pub entities: Vec<String>,
    #[desc = "The knowledge content — use [[entity name]] to mark entity mentions"]
    pub content: Option<String>,
}

pub struct KmsCreateKnowledgeTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsCreateKnowledgeTool {
    type Input = CreateKnowledgeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        validate_knowledge_title(&input.title)?;
        let ktype = knowledge_type_from_str(&input.knowledge_type);

        // Resolve entity refs.
        let mut entities = Vec::with_capacity(input.entities.len());
        for r in &input.entities {
            let id = self.svc.resolve(r).await.map_err(|_| {
                svc_err(format!(
                    "entity '{r}' not found — create it first with kms_create_entity"
                ))
            })?;
            entities.push(id);
        }

        let knowledge = self
            .svc
            .create_knowledge(&input.title, ktype, entities, input.content)
            .await
            .map_err(svc_err)?;

        Ok(ToolResult::success_json(json!({ "title": knowledge.title })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_update_knowledge
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_update_knowledge",
    description = "Update a knowledge entry's content and/or entities. Does NOT change the title."
)]
pub struct UpdateKnowledgeInput {
    #[desc = "Current title of the knowledge to update"]
    pub title_ref: String,
    #[desc = "New content — use [[entity name]] to mark entity mentions"]
    pub content: Option<String>,
    #[desc = "New array of all entity names mentioned in the content"]
    pub entities: Option<Vec<String>>,
}

pub struct KmsUpdateKnowledgeTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsUpdateKnowledgeTool {
    type Input = UpdateKnowledgeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entity_refs: Option<Vec<&str>> = input
            .entities
            .as_ref()
            .map(|v| v.iter().map(|s| s.as_str()).collect());
        let knowledge = self
            .svc
            .update_knowledge_by_ref(
                &input.title_ref,
                input.content.as_deref(),
                entity_refs,
            )
            .await
            .map_err(svc_err)?;

        Ok(ToolResult::success_json(json!({
            "title": knowledge.title,
            "knowledge_type": knowledge.knowledge_type.as_str(),
            "entities": knowledge.entities,
            "content": knowledge.content,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_delete_knowledge
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_delete_knowledge",
    description = "Delete a knowledge entry. Indexes referencing this knowledge are downgraded \
                   to empty Group nodes."
)]
pub struct DeleteKnowledgeInput {
    #[desc = "Title of the knowledge to delete"]
    pub title: String,
}

pub struct KmsDeleteKnowledgeTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsDeleteKnowledgeTool {
    type Input = DeleteKnowledgeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        self.svc.delete_knowledge(&input.title).await.map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({ "deleted": input.title })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_get_knowledge
// ═══════════════════════════════════════════════════════════════════

#[tool(name = "kms_get_knowledge", description = "Get the full content of a knowledge entry by title.")]
pub struct GetKnowledgeInput {
    #[desc = "Title of the knowledge entry"]
    pub title: String,
}

pub struct KmsGetKnowledgeTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsGetKnowledgeTool {
    type Input = GetKnowledgeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let id = self.svc.resolve_knowledge(&input.title).await.map_err(svc_err)?;
        let knowledge = self.svc.get_knowledge(id).await.map_err(svc_err)?;

        // Resolve entity names for the agent.
        let entity_names = resolve_entity_names(&self.svc, &knowledge.entities).await;

        Ok(ToolResult::success_json(json!({
            "title": knowledge.title,
            "knowledge_type": knowledge.knowledge_type.as_str(),
            "entities": entity_names,
            "content": knowledge.content,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_get_knowledge_batch
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_get_knowledge_batch",
    description = "Fetch multiple knowledge entries by title in one call. Returns one result per \
                   input title in the same order. Missing titles are reported as not_found."
)]
pub struct GetKnowledgeBatchInput {
    #[desc = "Array of knowledge titles to fetch"]
    pub titles: Vec<String>,
}

pub struct KmsGetKnowledgeBatchTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsGetKnowledgeBatchTool {
    type Input = GetKnowledgeBatchInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let results_raw = self
            .svc
            .get_knowledge_batch(input.titles)
            .await
            .map_err(svc_err)?;

        let mut results = Vec::with_capacity(results_raw.len());
        for r in results_raw {
            match (r.status, r.knowledge) {
                (kms::BatchStatus::Ok, Some(kv)) => {
                    let entity_names = resolve_entity_names(&self.svc, &kv.entities).await;
                    results.push(json!({
                        "title": r.title,
                        "status": "ok",
                        "knowledge": {
                            "title": kv.title,
                            "knowledge_type": kv.knowledge_type,
                            "entities": entity_names,
                            "content": kv.content,
                        }
                    }));
                }
                _ => {
                    results.push(json!({ "title": r.title, "status": "not_found" }));
                }
            }
        }

        Ok(ToolResult::success_json(json!({
            "count": results.len(),
            "results": results,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_get_entity_knowledge
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_get_entity_knowledge",
    description = "Get all knowledge entries that reference a given entity."
)]
pub struct GetEntityKnowledgeInput {
    #[desc = "Name of the entity to look up"]
    pub entity_name: String,
}

pub struct KmsGetEntityKnowledgeTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsGetEntityKnowledgeTool {
    type Input = GetEntityKnowledgeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entity_id = self.svc.resolve(&input.entity_name).await.map_err(svc_err)?;
        let knowledge_list = self
            .svc
            .get_entity_referencing_knowledge(entity_id)
            .await
            .map_err(svc_err)?;

        let results: Vec<_> = knowledge_list
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
            "entity": input.entity_name,
            "count": results.len(),
            "knowledges": results,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_rename_knowledge
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_rename_knowledge",
    description = "Rename a knowledge entry. All indexes referencing this knowledge are updated \
                   to the new title."
)]
pub struct RenameKnowledgeInput {
    #[desc = "Current title of the knowledge to rename"]
    pub current_title: String,
    #[desc = "New title for the knowledge entry"]
    pub new_title: String,
}

pub struct KmsRenameKnowledgeTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsRenameKnowledgeTool {
    type Input = RenameKnowledgeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let knowledge = self
            .svc
            .rename_knowledge(&input.current_title, &input.new_title)
            .await
            .map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({
            "old_title": input.current_title,
            "new_title": knowledge.title,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_search_content
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_search_content",
    description = "Stateless: search the subtree rooted at `path` for knowledge entries whose \
                   CONTENT contains the keyword (case-insensitive). Returns ranked hits with \
                   snippets."
)]
pub struct SearchContentInput {
    #[desc = "Absolute path of the subtree root (e.g. '/编程语言/Python')"]
    pub path: String,
    #[desc = "Substring to search for in knowledge content (case-insensitive)"]
    pub keyword: String,
    #[desc = "Max hits to return (default 20)"]
    #[default = 20]
    pub top_k: Option<usize>,
}

pub struct KmsSearchContentTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsSearchContentTool {
    type Input = SearchContentInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let node_id = self
            .svc
            .get_local_view_by_path(&input.path)
            .await
            .map_err(svc_err)?
            .node
            .id;
        let top_k = input.top_k.unwrap_or(20);
        let hits = self
            .svc
            .search_knowledge_content(node_id, &input.keyword, top_k)
            .await
            .map_err(svc_err)?;

        let results: Vec<_> = hits
            .iter()
            .map(|h| {
                json!({
                    "title": h.knowledge.title,
                    "snippet": h.snippet,
                    "match_count": h.match_count,
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
// Shared helpers
// ═══════════════════════════════════════════════════════════════════

async fn resolve_entity_names(svc: &KmsService, entity_ids: &[uuid::Uuid]) -> Vec<String> {
    let mut names = Vec::with_capacity(entity_ids.len());
    for eid in entity_ids {
        match svc.get_entity(*eid).await {
            Ok(e) => names.push(e.name.first().map(|n| n.full.clone()).unwrap_or_default()),
            Err(_) => names.push(eid.to_string()),
        }
    }
    names
}

// ═══════════════════════════════════════════════════════════════════
// Registration
// ═══════════════════════════════════════════════════════════════════

pub fn registrations(svc: Arc<KmsService>) -> Vec<ToolRegistration> {
    vec![
        KmsCreateKnowledgeTool { svc: svc.clone() }.into(),
        KmsUpdateKnowledgeTool { svc: svc.clone() }.into(),
        KmsDeleteKnowledgeTool { svc: svc.clone() }.into(),
        KmsGetKnowledgeTool { svc: svc.clone() }.into(),
        KmsGetKnowledgeBatchTool { svc: svc.clone() }.into(),
        KmsGetEntityKnowledgeTool { svc: svc.clone() }.into(),
        KmsRenameKnowledgeTool { svc: svc.clone() }.into(),
        KmsSearchContentTool { svc }.into(),
    ]
}
