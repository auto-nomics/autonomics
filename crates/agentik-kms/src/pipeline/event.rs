use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::artifacts::KnowledgeEvidenceInput;

/// An external fact that can trigger knowledge production.
///
/// Events describe what happened; they do not prescribe how a stage processes
/// it. The pipeline dispatcher owns the event-to-job mapping.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnowledgeEvent {
    AgentStarted {
        agent_id: Uuid,
    },
    SessionEnded {
        agent_id: Uuid,
        session_id: Uuid,
    },
    MemoryNoteCreated {
        note_id: Uuid,
    },
    ConsolidationRequested {
        reason: String,
    },
    ObservationRecorded {
        input: KnowledgeEvidenceInput,
    },
    SkillRevisionProposed {
        input: crate::artifacts::KnowledgeSkillProposalInput,
    },
    KnowledgeDiagnosisRequested {
        target_path: String,
        reason: String,
    },
    KnowledgeRepairRequested {
        finding_id: Uuid,
    },
}

impl KnowledgeEvent {
    #[must_use]
    pub fn dedup_key(&self) -> Option<String> {
        match self {
            Self::AgentStarted { agent_id } => Some(format!("agent-started:{agent_id}")),
            Self::SessionEnded { session_id, .. } => Some(format!("session-ended:{session_id}")),
            Self::MemoryNoteCreated { note_id } => Some(format!("memory-note:{note_id}")),
            Self::ConsolidationRequested { .. } => None,
            Self::ObservationRecorded { input } => {
                Some(format!("observation-recorded:{}", input.content_hash()))
            }
            Self::SkillRevisionProposed { input } => Some(format!(
                "skill-revision-proposed:{}:{}",
                input.skill_name,
                input.content_hash()
            )),
            Self::KnowledgeDiagnosisRequested { target_path, .. } => {
                Some(format!("diagnose-knowledge-node-requested:{target_path}"))
            }
            Self::KnowledgeRepairRequested { finding_id } => {
                Some(format!("repair-knowledge-node-requested:{finding_id}"))
            }
        }
    }

    #[must_use]
    pub fn jobs(&self) -> Vec<super::job::KnowledgeJob> {
        match self {
            Self::AgentStarted { agent_id } | Self::SessionEnded { agent_id, .. } => {
                vec![super::job::KnowledgeJob::ExtractAgentSessions {
                    agent_id: *agent_id,
                }]
            }
            Self::MemoryNoteCreated { .. } => {
                vec![super::job::KnowledgeJob::ConsolidateKnowledge {
                    trigger: "memory_note_created".to_string(),
                }]
            }
            Self::ConsolidationRequested { reason } => {
                vec![super::job::KnowledgeJob::ConsolidateKnowledge {
                    trigger: reason.clone(),
                }]
            }
            Self::ObservationRecorded { input } => {
                vec![super::job::KnowledgeJob::IngestObservation {
                    input: input.clone(),
                }]
            }
            Self::SkillRevisionProposed { input } => {
                vec![super::job::KnowledgeJob::ProposeSkillRevision {
                    input: input.clone(),
                }]
            }
            Self::KnowledgeDiagnosisRequested {
                target_path,
                reason,
            } => {
                vec![super::job::KnowledgeJob::DiagnoseKnowledgeNode {
                    target_path: target_path.clone(),
                    family_hash: None,
                    trigger: reason.clone(),
                }]
            }
            Self::KnowledgeRepairRequested { finding_id } => {
                vec![super::job::KnowledgeJob::RepairKnowledgeNode {
                    finding_id: *finding_id,
                }]
            }
        }
    }
}
