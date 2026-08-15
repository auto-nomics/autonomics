//! KMS tool layer split into read-only and write tool sets.
//!
//! Ported from dendrite's `dendrite-tools` crate. All tools follow the
//! autonomics `ToolFunction` pattern (`#[derive(ToolInput)]` + `impl ToolFunction`).
//!
//! ## Tool isolation
//!
//! These tools form a self-contained group that shares state only through
//! a single [`kms::KmsService`] handle. They are registered separately
//! from the main agent's tools (fs, opengwas, opentargets, …) and gated
//! behind `AgentProfile.enable_kms`.

mod entity_tools;
mod index_tools;
mod knowledge_tools;

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;
use kms::KmsService;

/// Read-only tool set used by task-facing agents.
///
/// Task agents can inspect and retrieve knowledge but never mutate the tree.
/// Write access is reserved for background maintenance agents.
pub fn kms_readonly_registrations(svc: Arc<KmsService>) -> Vec<ToolRegistration> {
    vec![
        entity_tools::KmsSearchEntityTool { svc: svc.clone() }.into(),
        entity_tools::KmsGetEntityTool { svc: svc.clone() }.into(),
        entity_tools::KmsListEntitiesTool { svc: svc.clone() }.into(),
        knowledge_tools::KmsGetKnowledgeTool { svc: svc.clone() }.into(),
        knowledge_tools::KmsGetKnowledgeBatchTool { svc: svc.clone() }.into(),
        knowledge_tools::KmsGetEntityKnowledgeTool { svc: svc.clone() }.into(),
        knowledge_tools::KmsSearchContentTool { svc: svc.clone() }.into(),
        index_tools::KmsLocalTool { svc: svc.clone() }.into(),
        index_tools::KmsSubtreeKnowledgeTool { svc: svc.clone() }.into(),
        index_tools::KmsSearchSubtreeTool { svc }.into(),
    ]
}

/// Write tool set used by KMS maintenance agents.
///
/// These tools intentionally create, update, delete, move, mount, unmount, or
/// merge entities, knowledge, and index nodes. Do not expose them to ordinary
/// task agents when long-term memory must remain read-only.
pub fn kms_write_registrations(svc: Arc<KmsService>) -> Vec<ToolRegistration> {
    vec![
        entity_tools::KmsCreateEntityTool { svc: svc.clone() }.into(),
        entity_tools::KmsUpdateEntityTool { svc: svc.clone() }.into(),
        entity_tools::KmsDeleteEntityTool { svc: svc.clone() }.into(),
        entity_tools::KmsAddNomenclatureTool { svc: svc.clone() }.into(),
        entity_tools::KmsUpdateNomenclatureTool { svc: svc.clone() }.into(),
        entity_tools::KmsDeleteNomenclatureTool { svc: svc.clone() }.into(),
        knowledge_tools::KmsCreateKnowledgeTool { svc: svc.clone() }.into(),
        knowledge_tools::KmsUpdateKnowledgeTool { svc: svc.clone() }.into(),
        knowledge_tools::KmsDeleteKnowledgeTool { svc: svc.clone() }.into(),
        knowledge_tools::KmsRenameKnowledgeTool { svc: svc.clone() }.into(),
        index_tools::KmsCreateIndexTool { svc: svc.clone() }.into(),
        index_tools::KmsDeleteIndexTool { svc: svc.clone() }.into(),
        index_tools::KmsMoveIndexTool { svc: svc.clone() }.into(),
        index_tools::KmsMoveChildrenTool { svc: svc.clone() }.into(),
        index_tools::KmsLinkOrphansTool { svc: svc.clone() }.into(),
        index_tools::KmsDetachKnowledgeTool { svc: svc.clone() }.into(),
        index_tools::KmsMergeSubtreeTool { svc }.into(),
    ]
}
