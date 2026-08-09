//! KMS tool layer — 27 `kms_*` tools exposing the knowledge tree to agents.
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

/// Full tool set: all 27 kms_* tools (entity + knowledge + index).
///
/// Pass the same `Arc<KmsService>` to every tool group — they share one
/// knowledge tree. Each `ToolRegistration` is independent and can be
/// individually enabled/disabled by the caller.
pub fn kms_registrations(svc: Arc<KmsService>) -> Vec<ToolRegistration> {
    let mut tools = Vec::new();
    tools.extend(entity_tools::registrations(svc.clone()));
    tools.extend(knowledge_tools::registrations(svc.clone()));
    tools.extend(index_tools::registrations(svc));
    tools
}

/// Read-only subset (9 tools): used by retrieval-only agents.
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
        index_tools::KmsSubtreeKnowledgeTool { svc }.into(),
    ]
}
