use async_trait::async_trait;
use turso::{Value, params_from_iter};
use uuid::Uuid;

use super::{
    Error, Result, TursoKmsStorage, db_err, row_to_diagnostic_finding, row_to_knowledge, uuid_value,
};
use crate::knowledge::Knowledge;
use crate::store::KnowledgeDiagnosticFinding;
use crate::store::knowledge_store::KnowledgeStore;

#[async_trait]
impl KnowledgeStore for TursoKmsStorage {
    async fn create_knowledge(&self, knowledge: Knowledge) -> Result<Knowledge> {
        self.ensure_knowledge_node(&knowledge).await?;
        let path = knowledge.knowledge_path.as_str();
        let mut rows = self
            .conn
            .query(
                "SELECT id, parent_id, knowledge_path, title, body_md, status,
                        confidence, source_hash, structural, created_at, updated_at
                 FROM knowledge_nodes
                 WHERE knowledge_path = ?1 AND status = 'active'",
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

    async fn get_knowledge_by_path(&self, path: &str) -> Result<Option<Knowledge>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, parent_id, knowledge_path, title, body_md, status,
                        confidence, source_hash, structural, created_at, updated_at
                 FROM knowledge_nodes
                 WHERE knowledge_path = ?1 AND status = 'active'",
                params_from_iter([Value::Text(path.to_string())]),
            )
            .await
            .map_err(db_err)?;
        Ok(match rows.next().await.map_err(db_err)? {
            Some(row) => Some(row_to_knowledge(&row)?),
            None => None,
        })
    }

    async fn list_child_knowledge(&self, parent_path: &str) -> Result<Vec<Knowledge>> {
        self.collect(
            "SELECT child.id, child.parent_id, child.knowledge_path, child.title,
                    child.body_md, child.status, child.confidence, child.source_hash,
                    child.structural, child.created_at, child.updated_at
             FROM knowledge_nodes AS child
             JOIN knowledge_nodes AS parent ON child.parent_id = parent.id
             WHERE parent.knowledge_path = ?1
               AND parent.status = 'active'
               AND child.status = 'active'
             ORDER BY child.knowledge_path ASC",
            vec![Value::Text(parent_path.to_string())],
            row_to_knowledge,
        )
        .await
    }

    async fn insert_diagnostic_finding(
        &self,
        finding: KnowledgeDiagnosticFinding,
    ) -> Result<KnowledgeDiagnosticFinding> {
        let proposed_paths = serde_json::to_string(&finding.proposed_paths)
            .map_err(|e| Error::Storage(format!("serialize diagnostic proposed paths: {e}")))?;
        self.conn
            .execute(
                "INSERT INTO knowledge_diagnostic_findings
                    (id, target_path, action, severity, status, message, recommendation,
                     proposed_paths_json, confidence, family_hash, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params_from_iter([
                    uuid_value(finding.id),
                    Value::Text(finding.target_path.clone()),
                    Value::Text(finding.action.clone()),
                    Value::Text(finding.severity.clone()),
                    Value::Text(finding.status.clone()),
                    Value::Text(finding.message.clone()),
                    Value::Text(finding.recommendation.clone()),
                    Value::Text(proposed_paths),
                    Value::Real(finding.confidence),
                    Value::Text(finding.family_hash.clone()),
                    Value::Integer(finding.created_at),
                ]),
            )
            .await
            .map_err(db_err)?;
        Ok(finding)
    }

    async fn list_diagnostic_findings(
        &self,
        target_path: &str,
        limit: usize,
    ) -> Result<Vec<KnowledgeDiagnosticFinding>> {
        self.collect(
            "SELECT id, target_path, action, severity, status, message, recommendation,
                    proposed_paths_json, confidence, family_hash, created_at
             FROM knowledge_diagnostic_findings
             WHERE target_path = ?1
             ORDER BY created_at DESC LIMIT ?2",
            vec![
                Value::Text(target_path.to_string()),
                Value::Integer(limit as i64),
            ],
            row_to_diagnostic_finding,
        )
        .await
    }

    async fn get_diagnostic_finding(
        &self,
        finding_id: Uuid,
    ) -> Result<Option<KnowledgeDiagnosticFinding>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, target_path, action, severity, status, message, recommendation,
                        proposed_paths_json, confidence, family_hash, created_at
                 FROM knowledge_diagnostic_findings WHERE id = ?1",
                params_from_iter([uuid_value(finding_id)]),
            )
            .await
            .map_err(db_err)?;
        Ok(match rows.next().await.map_err(db_err)? {
            Some(row) => Some(row_to_diagnostic_finding(&row)?),
            None => None,
        })
    }

    async fn set_diagnostic_finding_status(&self, finding_id: Uuid, status: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE knowledge_diagnostic_findings
                 SET status = ?1
                 WHERE id = ?2",
                params_from_iter([Value::Text(status.to_string()), uuid_value(finding_id)]),
            )
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn apply_knowledge_repair(
        &self,
        nodes: Vec<Knowledge>,
        archive_paths: Vec<String>,
    ) -> Result<()> {
        let now = crate::now_ms();
        self.conn.execute("BEGIN", ()).await.map_err(db_err)?;
        let result = async {
            for node in nodes {
                self.ensure_knowledge_node(&node).await?;
            }
            for path in archive_paths {
                self.conn
                    .execute(
                        "UPDATE knowledge_nodes
                         SET status = 'superseded', updated_at = ?1
                         WHERE knowledge_path = ?2 AND status = 'active'",
                        params_from_iter([Value::Integer(now), Value::Text(path)]),
                    )
                    .await
                    .map_err(db_err)?;
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                self.conn.execute("COMMIT", ()).await.map_err(db_err)?;
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }
}
