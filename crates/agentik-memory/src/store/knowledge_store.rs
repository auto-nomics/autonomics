use async_trait::async_trait;

use crate::knowledge::Knowledge;
use crate::store::error::Result;

#[async_trait]
pub trait KnowledgeStore {
    async fn create_knowledge(&self, knowledge: Knowledge) -> Result<Knowledge>;
}
