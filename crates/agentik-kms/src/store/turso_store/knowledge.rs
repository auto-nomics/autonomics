use async_trait::async_trait;
use turso::{Value, params_from_iter};
use uuid::Uuid;

use super::{Error, Result, TursoMemoryStorage, db_err, row_to_knowledge, uuid_value};
use crate::knowledge::Knowledge;
use crate::store::knowledge_store::KnowledgeStore;

#[async_trait]
impl KnowledgeStore for TursoMemoryStorage {
    async fn create_knowledge(&self, knowledge: Knowledge) -> Result<Knowledge> {
        self.ensure_knowledge_node(&knowledge).await?;
        let path = knowledge.knowledge_path.as_str();
        let mut rows = self
            .conn
            .query(
                "SELECT id, parent_id, knowledge_path, title, body_md, status,
                        confidence, source_hash, structural, created_at, updated_at
                 FROM knowledge_nodes WHERE knowledge_path = ?1",
                params_from_iter([Value::Text(path)]),
            )
            .await
            .map_err(db_err)?;
        match rows.next().await.map_err(db_err)? {
            Some(row) => Ok(row_to_knowledge(&row)?),
            None => Err(Error::Storage(
                "knowledge node vanished immediately after upsert".into(),
            )),
        }
    }

    async fn get_knowledge(&self, id: Uuid) -> Result<Option<Knowledge>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, parent_id, knowledge_path, title, body_md, status,
                        confidence, source_hash, structural, created_at, updated_at
                 FROM knowledge_nodes WHERE id = ?1",
                params_from_iter([uuid_value(id)]),
            )
            .await
            .map_err(db_err)?;
        match rows.next().await.map_err(db_err)? {
            Some(row) => Ok(Some(row_to_knowledge(&row)?)),
            None => Ok(None),
        }
    }

    async fn update_knowledge(&self, knowledge: Knowledge) -> Result<()> {
        let ensured_id = self.ensure_knowledge_node(&knowledge).await?;
        if ensured_id != knowledge.id {
            return Err(Error::Storage(format!(
                "knowledge path is already owned by node {ensured_id}"
            )));
        }
        Ok(())
    }

    async fn list_knowledge(&self, limit: usize) -> Result<Vec<Knowledge>> {
        self.collect(
            "SELECT id, parent_id, knowledge_path, title, body_md, status,
                    confidence, source_hash, structural, created_at, updated_at
             FROM knowledge_nodes WHERE status = 'active'
             ORDER BY updated_at DESC, knowledge_path ASC LIMIT ?1",
            vec![Value::Integer(limit as i64)],
            row_to_knowledge,
        )
        .await
    }
}
