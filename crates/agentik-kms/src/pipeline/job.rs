use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A durable unit of knowledge-pipeline work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnowledgeJob {
    ExtractAgentSessions { agent_id: Uuid },
    ConsolidateKnowledge { trigger: String },
}

impl KnowledgeJob {
    #[must_use]
    pub fn dedup_key(&self) -> String {
        match self {
            Self::ExtractAgentSessions { agent_id } => {
                format!("extract-agent-sessions:{agent_id}")
            }
            Self::ConsolidateKnowledge { .. } => "consolidate-knowledge".to_string(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::ExtractAgentSessions { .. } => "extract_agent_sessions",
            Self::ConsolidateKnowledge { .. } => "consolidate_knowledge",
        }
    }
}
