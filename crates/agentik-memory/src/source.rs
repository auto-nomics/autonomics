use async_trait::async_trait;
use uuid::Uuid;

use crate::error::MemoryError;
use agentik_sdk::types::messages::Message;

#[derive(Debug, Clone)]
pub struct MemorySessionRecord {
    pub session_id: Uuid,
    pub title: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

/// Read-only view of persisted agent rollouts consumed by memory extraction.
#[async_trait]
pub trait MemorySource: Send + Sync {
    async fn list_session_records(
        &self,
        agent_id: Uuid,
    ) -> Result<Vec<MemorySessionRecord>, MemoryError>;

    async fn get_messages_since_for_session(
        &self,
        session_id: Uuid,
        sequence: i64,
    ) -> Result<Vec<Message>, MemoryError>;
}
