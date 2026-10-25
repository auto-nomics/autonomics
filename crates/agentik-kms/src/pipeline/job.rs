use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Routing key shared by durable queue rows and resident workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnowledgeJobKind {
    SessionExtract,
    ConsolidateKnowledge,
}

impl KnowledgeJobKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionExtract => "session_extract",
            Self::ConsolidateKnowledge => "consolidate_knowledge",
        }
    }

    /// Tag used by the persisted JSON job payload.
    pub const fn queue_tag(self) -> &'static str {
        match self {
            Self::SessionExtract => "extract_agent_sessions",
            Self::ConsolidateKnowledge => "consolidate_knowledge",
        }
    }
}

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
    pub const fn kind(&self) -> KnowledgeJobKind {
        match self {
            Self::ExtractAgentSessions { .. } => KnowledgeJobKind::SessionExtract,
            Self::ConsolidateKnowledge { .. } => KnowledgeJobKind::ConsolidateKnowledge,
        }
    }
}
