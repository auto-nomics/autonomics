use async_trait::async_trait;
use uuid::Uuid;

use crate::error::Error;
use agentik_sdk::types::messages::Message;

#[derive(Debug, Clone)]
pub struct MemorySessionRecord {
    pub session_id: Uuid,
    pub title: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

/// A source with no rollouts, for builders and tests that only need memory
/// read/write tools without starting an extraction-capable runtime.
pub struct EmptyMemorySource;

#[async_trait]
impl MemorySource for EmptyMemorySource {
    async fn list_session_records(
        &self,
        _agent_id: Uuid,
    ) -> Result<Vec<MemorySessionRecord>, Error> {
        Ok(Vec::new())
    }

    async fn get_messages_since_for_session(
        &self,
        _session_id: Uuid,
        _sequence: i64,
    ) -> Result<Vec<Message>, Error> {
        Ok(Vec::new())
    }
}

/// Read-only view of persisted agent rollouts consumed by memory extraction.
#[async_trait]
pub trait MemorySource: Send + Sync {
    async fn list_session_records(&self, agent_id: Uuid)
    -> Result<Vec<MemorySessionRecord>, Error>;

    async fn get_messages_since_for_session(
        &self,
        session_id: Uuid,
        sequence: i64,
    ) -> Result<Vec<Message>, Error>;
}
