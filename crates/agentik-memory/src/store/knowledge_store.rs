use async_trait::async_trait;
use uuid::Uuid;

use crate::knowledge::Knowledge;
use crate::store::error::Result;

#[async_trait]
pub trait KnowledgeStore {
    async fn create_knowledge(&self, knowledge: Knowledge) -> Result<Knowledge>;

    async fn get_knowledge(&self, id: Uuid) -> Result<Option<Knowledge>>;

    async fn update_knowledge(&self, knowledge: Knowledge) -> Result<()>;

    async fn list_knowledge(&self, limit: usize) -> Result<Vec<Knowledge>>;
}
