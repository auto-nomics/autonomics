use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Routing key shared by durable queue rows and resident workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnowledgeJobKind {
    SessionExtract,
    ConsolidateKnowledge,
    DiagnoseKnowledgeNode,
    RepairKnowledgeNode,
}

impl KnowledgeJobKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionExtract => "session_extract",
            Self::ConsolidateKnowledge => "consolidate_knowledge",
            Self::DiagnoseKnowledgeNode => "diagnose_knowledge_node",
            Self::RepairKnowledgeNode => "repair_knowledge_node",
        }
    }

    /// Tag used by the persisted JSON job payload.
    pub const fn queue_tag(self) -> &'static str {
        match self {
            Self::SessionExtract => "extract_agent_sessions",
            Self::ConsolidateKnowledge => "consolidate_knowledge",
            Self::DiagnoseKnowledgeNode => "diagnose_knowledge_node",
            Self::RepairKnowledgeNode => "repair_knowledge_node",
        }
    }
}

/// A durable unit of knowledge-pipeline work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnowledgeJob {
    ExtractAgentSessions {
        agent_id: Uuid,
    },
    ConsolidateKnowledge {
        trigger: String,
    },
    DiagnoseKnowledgeNode {
        target_path: String,
        family_hash: Option<String>,
        trigger: String,
    },
    RepairKnowledgeNode {
        finding_id: Uuid,
    },
}

impl KnowledgeJob {
    #[must_use]
    pub fn dedup_key(&self) -> String {
        match self {
            Self::ExtractAgentSessions { agent_id } => {
                format!("extract-agent-sessions:{agent_id}")
            }
            Self::ConsolidateKnowledge { .. } => "consolidate-knowledge".to_string(),
            Self::DiagnoseKnowledgeNode {
                target_path,
                family_hash,
                ..
            } => format!(
                "diagnose-knowledge-node:{target_path}:{}",
                family_hash.as_deref().unwrap_or("current")
            ),
            Self::RepairKnowledgeNode { finding_id } => {
                format!("repair-knowledge-node:{finding_id}")
            }
        }
    }

    #[must_use]
    pub const fn kind(&self) -> KnowledgeJobKind {
        match self {
            Self::ExtractAgentSessions { .. } => KnowledgeJobKind::SessionExtract,
            Self::ConsolidateKnowledge { .. } => KnowledgeJobKind::ConsolidateKnowledge,
            Self::DiagnoseKnowledgeNode { .. } => KnowledgeJobKind::DiagnoseKnowledgeNode,
            Self::RepairKnowledgeNode { .. } => KnowledgeJobKind::RepairKnowledgeNode,
        }
    }
}
