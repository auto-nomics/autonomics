//! Knowledge CRUD, batch reads, and title renaming.

use serde::Serialize;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::storage::repo;
use crate::storage::types::Knowledge;

use super::KmsService;

#[derive(Debug, Clone, Serialize)]
pub struct BatchKnowledgeResult {
    pub title: String,
    pub status: BatchStatus,
    pub knowledge: Option<KnowledgeView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    Ok,
    NotFound,
}

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeContentHit {
    pub knowledge: KnowledgeView,
    pub snippet: String,
    pub match_count: usize,
}

/// Serializable, agent-facing projection of a `Knowledge` row.
#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeView {
    pub id: Uuid,
    pub title: String,
    pub entities: Vec<Uuid>,
    pub content: Option<String>,
    pub source_document_id: Option<Uuid>,
    pub source_chunk_idx: Option<i64>,
}

impl From<Knowledge> for KnowledgeView {
    fn from(k: Knowledge) -> Self {
        Self {
            id: k.id,
            title: k.title,
            entities: k.entities,
            content: k.content,
            source_document_id: k.source_document_id,
            source_chunk_idx: k.source_chunk_idx,
        }
    }
}

// ── Knowledge ──
impl KmsService {
    pub async fn create_knowledge(
        &self,
        title: &str,
        entities: Vec<Uuid>,
        content: Option<String>,
    ) -> Result<Knowledge> {
        let knowledge = Knowledge {
            id: Uuid::new_v4(),
            title: title.to_string(),
            entities,
            content,
            source_document_id: None,
            source_chunk_idx: None,
        };
        repo::knowledge_create(&*self.storage.conn().await, &knowledge).await?;
        Ok(knowledge)
    }

    pub async fn get_knowledge(&self, id: Uuid) -> Result<Knowledge> {
        Ok(repo::knowledge_get(&*self.storage.conn().await, id).await?)
    }

    pub async fn get_knowledge_batch(
        &self,
        titles: Vec<String>,
    ) -> Result<Vec<BatchKnowledgeResult>> {
        let mut out = Vec::with_capacity(titles.len());
        for title in titles {
            let entry = match self.resolve_knowledge(&title).await {
                Ok(id) => match self.get_knowledge(id).await {
                    Ok(k) => BatchKnowledgeResult {
                        title,
                        status: BatchStatus::Ok,
                        knowledge: Some(k.into()),
                    },
                    Err(_) => BatchKnowledgeResult {
                        title,
                        status: BatchStatus::NotFound,
                        knowledge: None,
                    },
                },
                Err(_) => BatchKnowledgeResult {
                    title,
                    status: BatchStatus::NotFound,
                    knowledge: None,
                },
            };
            out.push(entry);
        }
        Ok(out)
    }

    pub async fn create_knowledge_with_source(
        &self,
        title: &str,
        entities: Vec<Uuid>,
        content: Option<String>,
        source: Option<(Uuid, usize)>,
    ) -> Result<Knowledge> {
        let (source_document_id, source_chunk_idx) = match source {
            Some((doc_id, chunk_idx)) => {
                if let Some(docs) = &self.documents {
                    if !docs.document_exists(doc_id).await {
                        return Err(Error::from(format!(
                            "source_document_id {doc_id} not found in document store"
                        )));
                    }
                }
                (Some(doc_id), Some(chunk_idx as i64))
            }
            None => (None, None),
        };
        let knowledge = Knowledge {
            id: Uuid::new_v4(),
            title: title.to_string(),
            entities,
            content,
            source_document_id,
            source_chunk_idx,
        };
        repo::knowledge_create(&*self.storage.conn().await, &knowledge).await?;
        Ok(knowledge)
    }

    pub async fn create_knowledge_by_ref(
        &self,
        title: &str,
        entity_refs: Vec<&str>,
        content: Option<String>,
    ) -> Result<Knowledge> {
        let mut entities = Vec::with_capacity(entity_refs.len());
        for r in entity_refs {
            let id = self.resolve(r).await.map_err(|_| {
                format!("entity '{r}' not found, please create it first with kms_create_entity")
            })?;
            entities.push(id);
        }
        self.create_knowledge(title, entities, content).await
    }

    pub async fn update_knowledge_by_ref(
        &self,
        title_ref: &str,
        new_content: Option<&str>,
        new_entities: Option<Vec<&str>>,
    ) -> Result<Knowledge> {
        let id = self.resolve_knowledge(title_ref).await?;
        let conn = &*self.storage.conn().await;
        let mut knowledge = repo::knowledge_get(conn, id).await?;
        if let Some(content) = new_content {
            knowledge.content = Some(content.to_string());
        }
        if let Some(entity_refs) = new_entities {
            let mut entities = Vec::with_capacity(entity_refs.len());
            for r in entity_refs {
                entities.push(self.resolve(r).await?);
            }
            knowledge.entities = entities;
        }
        repo::knowledge_update(conn, &knowledge).await?;
        Ok(knowledge)
    }

    pub async fn rename_knowledge(&self, old_title: &str, new_title: &str) -> Result<Knowledge> {
        let id = self.resolve_knowledge(old_title).await?;
        let conn = &*self.storage.conn().await;
        let mut knowledge = repo::knowledge_get(conn, id).await?;

        if let Some(existing) = repo::knowledge_find_by_title(conn, new_title).await? {
            if existing.id != id {
                return Err(Error::from(format!(
                    "knowledge title '{new_title}' already exists (id: {}); rename it first or choose a different title",
                    existing.id
                )));
            }
        }
        if repo::index_find_by_title(conn, new_title).await?.is_some() {
            return Err(Error::from(format!(
                "index with title '{new_title}' already exists; delete or rename the conflicting index first, then retry"
            )));
        }

        let referencing_indexes = repo::index_find_by_target(conn, id).await?;
        for idx in &referencing_indexes {
            let mut updated = idx.clone();
            updated.title = Some(new_title.to_string());
            repo::index_update(conn, &updated).await?;
        }

        knowledge.title = new_title.to_string();
        repo::knowledge_update(conn, &knowledge).await?;
        Ok(knowledge)
    }

    pub async fn delete_knowledge(&self, title: &str) -> Result<()> {
        let id = self.resolve_knowledge(title).await?;
        let conn = &*self.storage.conn().await;

        let referencing_indexes = repo::index_find_by_target(conn, id).await?;
        for idx in &referencing_indexes {
            repo::index_downgrade_to_group(conn, idx.id).await?;
        }
        repo::knowledge_delete(conn, id).await?;
        Ok(())
    }
}

// ── Knowledge lookup by entity ──
impl KmsService {
    pub async fn get_entity_referencing_knowledge(
        &self,
        entity_id: Uuid,
    ) -> Result<Vec<Knowledge>> {
        Ok(repo::knowledge_find_by_entity(&*self.storage.conn().await, entity_id).await?)
    }
}
