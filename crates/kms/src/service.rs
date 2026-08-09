//! KmsService — high-level knowledge management operations.
//!
//! Ported from dendrite's `service.rs`. Business logic is identical; only
//! the storage layer is swapped from sqlx to Turso, and the corpus
//! dependency is made optional via [`KmsDocumentStore`].

use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::Diagnostic;
use crate::Storage;
use crate::diagnostics;
use crate::language::Language;
use crate::storage::repo;
use crate::storage::types::{Entity, Index, Knowledge, KnowledgeType, Nomenclature, TargetType};
use crate::view::{IndexView, LocalView, SUBTREE_TITLES_LIMIT, SubtreeSummary};

// ─────────────────────────── document store trait ───────────────────────────

/// Trait for validating source-document references. Implemented by the
/// corpus crate (Phase 2); when `None`, source-document validation is
/// skipped.
#[async_trait]
pub trait KmsDocumentStore: Send + Sync {
    async fn document_exists(&self, doc_id: Uuid) -> bool;
}

// ─────────────────────────── result types ───────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct MoveChildrenResult {
    pub location: String,
    pub new_group_id: Uuid,
    pub group_created: bool,
}

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
    pub knowledge_type: String,
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
            knowledge_type: k.knowledge_type.as_str().to_string(),
            entities: k.entities,
            content: k.content,
            source_document_id: k.source_document_id,
            source_chunk_idx: k.source_chunk_idx,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EntityFilter {
    EmptyDefinition,
    NoNomenclature,
    All,
}

// ─────────────────────────── KmsService ───────────────────────────

struct Inner {
    pointer: RwLock<Uuid>,
}

#[derive(Clone)]
pub struct KmsService {
    inner: Arc<Inner>,
    storage: Storage,
    documents: Option<Arc<dyn KmsDocumentStore>>,
}

impl KmsService {
    /// Open a KMS at `db_path`. Pass `None` for `documents` to skip
    /// source-document validation (Phase 1 mode).
    pub async fn new(
        db_path: &str,
        documents: Option<Arc<dyn KmsDocumentStore>>,
    ) -> Result<Self, String> {
        let storage = Storage::new(db_path).await?;
        let root_id = ensure_root_index(&storage).await?;
        let inner = Arc::new(Inner {
            pointer: RwLock::new(root_id),
        });
        Ok(KmsService {
            inner,
            storage,
            documents,
        })
    }

    /// Create a KMS from an already-open `Storage` (e.g. in-memory for tests).
    pub async fn from_storage(storage: Storage) -> Result<Self, String> {
        let root_id = ensure_root_index(&storage).await?;
        let inner = Arc::new(Inner {
            pointer: RwLock::new(root_id),
        });
        Ok(KmsService {
            inner,
            storage,
            documents: None,
        })
    }

    pub fn conn(&self) -> &turso::Connection {
        self.storage.conn()
    }

    pub async fn get_pointer(&self) -> Uuid {
        *self.inner.pointer.read().await
    }

    async fn set_pointer(&self, id: Uuid) {
        *self.inner.pointer.write().await = id;
    }

    pub async fn find_root(&self) -> Result<Index, String> {
        repo::index_find_root(self.storage.conn())
            .await
            .map_err(|e| e.to_string())
    }

    // ── Entity ──

    pub async fn create_entity(
        &self,
        names: Vec<Nomenclature>,
        definition: &str,
    ) -> Result<(Entity, bool), String> {
        let conn = self.storage.conn();

        // Dedup: if an entity with the same name already exists, return it.
        let lookup_name = names
            .iter()
            .find(|n| n.lang == Language::ZH)
            .or_else(|| names.first())
            .map(|n| n.full.as_str())
            .unwrap_or("");

        if !lookup_name.is_empty() {
            if let Some(existing) = repo::entity_find_by_exact_name(conn, lookup_name)
                .await
                .map_err(|e| e.to_string())?
            {
                return Ok((existing, true));
            }
        }

        // Deduplicate input nomenclatures: keep only the first per (lang, full).
        let mut seen: Vec<(Language, String)> = Vec::new();
        let names: Vec<Nomenclature> = names
            .into_iter()
            .filter(|n| {
                let dup = seen.iter().any(|(l, f)| *l == n.lang && f == &n.full);
                if !dup {
                    seen.push((n.lang, n.full.clone()));
                }
                !dup
            })
            .collect();

        // Check each remaining nomenclature against existing DB records.
        let mut db_safe_names = Vec::new();
        for n in names {
            let exists = repo::entity_find_by_exact_name(conn, &n.full)
                .await
                .map_err(|e| e.to_string())?;
            if exists.is_none() {
                db_safe_names.push(n);
            }
        }
        let names = db_safe_names;

        if names.is_empty() {
            return Err("所有提供的命名已存在于数据库中，无法创建新实体".into());
        }

        let entity = Entity {
            id: Uuid::new_v4(),
            name: names,
            definition: definition.to_string(),
        };
        repo::entity_create(conn, &entity)
            .await
            .map_err(|e| e.to_string())?;
        Ok((entity, false))
    }

    pub async fn get_entity(&self, id: Uuid) -> Result<Entity, String> {
        repo::entity_get(self.storage.conn(), id)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn delete_entity(&self, id: Uuid) -> Result<(), String> {
        repo::entity_delete(self.storage.conn(), id)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn add_nomenclature(
        &self,
        entity_id: Uuid,
        lang: Language,
        full: String,
        abbr: Option<String>,
    ) -> Result<Entity, String> {
        let conn = self.storage.conn();
        let entity = repo::entity_get(conn, entity_id)
            .await
            .map_err(|e| e.to_string())?;
        if entity.name.iter().any(|n| n.lang == lang && n.full == full) {
            return Err(format!("命名 ({:?}, {}) 已存在于该实体中", lang, full));
        }
        let nom = Nomenclature {
            id: Uuid::new_v4(),
            lang,
            full: full.clone(),
            abbr,
        };
        repo::entity_add_nomenclature(conn, entity_id, &nom)
            .await
            .map_err(|e| e.to_string())?;
        repo::entity_get(conn, entity_id)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn update_nomenclature(
        &self,
        entity_id: Uuid,
        nomenclature_id: Uuid,
        lang: Language,
        full: String,
        abbr: Option<String>,
    ) -> Result<Entity, String> {
        let conn = self.storage.conn();
        let entity = repo::entity_get(conn, entity_id)
            .await
            .map_err(|e| e.to_string())?;
        if !entity.name.iter().any(|n| n.id == nomenclature_id) {
            return Err("该 nomenclature 不属于此实体".into());
        }
        if entity
            .name
            .iter()
            .any(|n| n.id != nomenclature_id && n.lang == lang && n.full == full)
        {
            return Err(format!(
                "命名 ({:?}, {}) 已存在于该实体的另一条记录中",
                lang, full
            ));
        }
        let nom = Nomenclature {
            id: nomenclature_id,
            lang,
            full,
            abbr,
        };
        repo::entity_update_nomenclature(conn, entity_id, &nom)
            .await
            .map_err(|e| e.to_string())?;
        repo::entity_get(conn, entity_id)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn delete_nomenclature(
        &self,
        entity_id: Uuid,
        nomenclature_id: Uuid,
    ) -> Result<Entity, String> {
        let conn = self.storage.conn();
        let entity = repo::entity_get(conn, entity_id)
            .await
            .map_err(|e| e.to_string())?;
        if entity.name.len() <= 1 {
            return Err("实体至少需要保留一条命名".into());
        }
        if !entity.name.iter().any(|n| n.id == nomenclature_id) {
            return Err("该 nomenclature 不属于此实体".into());
        }
        repo::entity_delete_nomenclature(conn, nomenclature_id)
            .await
            .map_err(|e| e.to_string())?;
        repo::entity_get(conn, entity_id)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn search_entity(&self, keyword: &str) -> Result<Vec<Entity>, String> {
        repo::entity_search_by_name(self.storage.conn(), keyword)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn list_entities(&self, filter: EntityFilter) -> Result<Vec<Entity>, String> {
        let all = repo::entity_list_all(self.storage.conn())
            .await
            .map_err(|e| e.to_string())?;
        match filter {
            EntityFilter::All => Ok(all),
            EntityFilter::EmptyDefinition => Ok(all
                .into_iter()
                .filter(|e| e.definition.is_empty())
                .collect()),
            EntityFilter::NoNomenclature => {
                Ok(all.into_iter().filter(|e| e.name.is_empty()).collect())
            }
        }
    }

    pub async fn resolve(&self, name: &str) -> Result<Uuid, String> {
        let conn = self.storage.conn();
        if let Some(entity) = repo::entity_find_by_exact_name(conn, name)
            .await
            .map_err(|e| e.to_string())?
        {
            return Ok(entity.id);
        }
        if let Some(idx) = repo::index_find_by_title(conn, name)
            .await
            .map_err(|e| e.to_string())?
        {
            return Ok(idx.id);
        }
        if let Some(knowledge) = repo::knowledge_find_by_title(conn, name)
            .await
            .map_err(|e| e.to_string())?
        {
            return Ok(knowledge.id);
        }
        Err(format!("cannot resolve: {name}"))
    }

    pub async fn resolve_index(&self, name: &str) -> Result<Uuid, String> {
        if let Some(idx) = repo::index_find_by_title(self.storage.conn(), name)
            .await
            .map_err(|e| e.to_string())?
        {
            return Ok(idx.id);
        }
        Err(format!("index not found: {name}"))
    }

    pub async fn resolve_knowledge(&self, title: &str) -> Result<Uuid, String> {
        if let Some(knowledge) = repo::knowledge_find_by_title(self.storage.conn(), title)
            .await
            .map_err(|e| e.to_string())?
        {
            return Ok(knowledge.id);
        }
        Err(format!("knowledge not found: {title}"))
    }

    pub async fn update_entity_by_ref(
        &self,
        name_ref: &str,
        new_definition: Option<&str>,
        new_names: Option<Vec<Nomenclature>>,
    ) -> Result<Entity, String> {
        let id = self.resolve(name_ref).await?;
        self.update_entity(id, new_definition, new_names).await
    }

    pub async fn update_entity_by_id(
        &self,
        id: Uuid,
        new_definition: Option<&str>,
        new_names: Option<Vec<Nomenclature>>,
    ) -> Result<Entity, String> {
        self.update_entity(id, new_definition, new_names).await
    }

    async fn update_entity(
        &self,
        id: Uuid,
        new_definition: Option<&str>,
        new_names: Option<Vec<Nomenclature>>,
    ) -> Result<Entity, String> {
        let conn = self.storage.conn();
        let mut entity = repo::entity_get(conn, id)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(definition) = new_definition {
            entity.definition = definition.to_string();
        }
        if let Some(names) = new_names {
            entity.name = names;
        }
        repo::entity_update(conn, &entity)
            .await
            .map_err(|e| e.to_string())?;
        Ok(entity)
    }

    // ── Knowledge ──

    pub async fn create_knowledge(
        &self,
        title: &str,
        knowledge_type: KnowledgeType,
        entities: Vec<Uuid>,
        content: Option<String>,
    ) -> Result<Knowledge, String> {
        let knowledge = Knowledge {
            id: Uuid::new_v4(),
            title: title.to_string(),
            knowledge_type,
            entities,
            content,
            source_document_id: None,
            source_chunk_idx: None,
        };
        repo::knowledge_create(self.storage.conn(), &knowledge)
            .await
            .map_err(|e| e.to_string())?;
        Ok(knowledge)
    }

    pub async fn get_knowledge(&self, id: Uuid) -> Result<Knowledge, String> {
        repo::knowledge_get(self.storage.conn(), id)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn get_knowledge_batch(
        &self,
        titles: Vec<String>,
    ) -> Result<Vec<BatchKnowledgeResult>, String> {
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
        knowledge_type: KnowledgeType,
        entities: Vec<Uuid>,
        content: Option<String>,
        source: Option<(Uuid, usize)>,
    ) -> Result<Knowledge, String> {
        let (source_document_id, source_chunk_idx) = match source {
            Some((doc_id, chunk_idx)) => {
                if let Some(docs) = &self.documents {
                    if !docs.document_exists(doc_id).await {
                        return Err(format!(
                            "source_document_id {doc_id} not found in document store"
                        ));
                    }
                }
                (Some(doc_id), Some(chunk_idx as i64))
            }
            None => (None, None),
        };
        let knowledge = Knowledge {
            id: Uuid::new_v4(),
            title: title.to_string(),
            knowledge_type,
            entities,
            content,
            source_document_id,
            source_chunk_idx,
        };
        repo::knowledge_create(self.storage.conn(), &knowledge)
            .await
            .map_err(|e| e.to_string())?;
        Ok(knowledge)
    }

    pub async fn create_knowledge_by_ref(
        &self,
        title: &str,
        knowledge_type: KnowledgeType,
        entity_refs: Vec<&str>,
        content: Option<String>,
    ) -> Result<Knowledge, String> {
        let mut entities = Vec::with_capacity(entity_refs.len());
        for r in entity_refs {
            let id = self.resolve(r).await.map_err(|_| {
                format!("entity '{r}' not found, please create it first with kms_create_entity")
            })?;
            entities.push(id);
        }
        self.create_knowledge(title, knowledge_type, entities, content)
            .await
    }

    pub async fn update_knowledge_by_ref(
        &self,
        title_ref: &str,
        new_content: Option<&str>,
        new_entities: Option<Vec<&str>>,
    ) -> Result<Knowledge, String> {
        let id = self.resolve_knowledge(title_ref).await?;
        let conn = self.storage.conn();
        let mut knowledge = repo::knowledge_get(conn, id)
            .await
            .map_err(|e| e.to_string())?;
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
        repo::knowledge_update(conn, &knowledge)
            .await
            .map_err(|e| e.to_string())?;
        Ok(knowledge)
    }

    pub async fn rename_knowledge(
        &self,
        old_title: &str,
        new_title: &str,
    ) -> Result<Knowledge, String> {
        let id = self.resolve_knowledge(old_title).await?;
        let conn = self.storage.conn();
        let mut knowledge = repo::knowledge_get(conn, id)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(existing) = repo::knowledge_find_by_title(conn, new_title)
            .await
            .map_err(|e| e.to_string())?
        {
            if existing.id != id {
                return Err(format!(
                    "knowledge title '{new_title}' already exists (id: {}); rename it first or choose a different title",
                    existing.id
                ));
            }
        }
        if repo::index_find_by_title(conn, new_title)
            .await
            .map_err(|e| e.to_string())?
            .is_some()
        {
            return Err(format!(
                "index with title '{new_title}' already exists; delete or rename the conflicting index first, then retry"
            ));
        }

        let referencing_indexes = repo::index_find_by_target(conn, id)
            .await
            .map_err(|e| e.to_string())?;
        for idx in &referencing_indexes {
            let mut updated = idx.clone();
            updated.title = Some(new_title.to_string());
            repo::index_update(conn, &updated)
                .await
                .map_err(|e| e.to_string())?;
        }

        knowledge.title = new_title.to_string();
        repo::knowledge_update(conn, &knowledge)
            .await
            .map_err(|e| e.to_string())?;
        Ok(knowledge)
    }

    pub async fn delete_knowledge(&self, title: &str) -> Result<(), String> {
        let id = self.resolve_knowledge(title).await?;
        let conn = self.storage.conn();

        let referencing_indexes = repo::index_find_by_target(conn, id)
            .await
            .map_err(|e| e.to_string())?;
        for idx in &referencing_indexes {
            repo::index_downgrade_to_group(conn, idx.id)
                .await
                .map_err(|e| e.to_string())?;
        }
        repo::knowledge_delete(conn, id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── Index ──

    pub async fn create_index_root(&self, title: &str) -> Result<Index, String> {
        let id = Uuid::new_v4();
        let entry = Index {
            id,
            title: Some(title.to_string()),
            target: None,
            target_type: TargetType::Group,
            parent_id: None,
            position: 0,
        };
        repo::index_create(self.storage.conn(), &entry)
            .await
            .map_err(|e| e.to_string())?;
        Ok(entry)
    }

    pub async fn create_index(
        &self,
        parent_id: Uuid,
        title: Option<String>,
        target: Option<Uuid>,
        target_type: Option<TargetType>,
    ) -> Result<Index, String> {
        let conn = self.storage.conn();
        let parent = repo::index_get(conn, parent_id)
            .await
            .map_err(|e| e.to_string())?;
        let siblings = repo::index_children_of(conn, Some(parent_id))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(new_title) = title.as_deref() {
            if let Some(conflict) = siblings
                .iter()
                .find(|c| c.title.as_deref() == Some(new_title))
            {
                let parent_label = parent.title.as_deref().unwrap_or("(unnamed parent)");
                return Err(format!(
                    "duplicate child title '{new_title}' under parent '{parent_label}' \
                     (conflicts with existing index {})",
                    conflict.id
                ));
            }
        }

        let id = Uuid::new_v4();
        let entry = Index {
            id,
            title,
            target,
            target_type: target_type.unwrap_or(TargetType::Group),
            parent_id: Some(parent_id),
            position: siblings.len() as i64,
        };
        repo::index_create(conn, &entry)
            .await
            .map_err(|e| e.to_string())?;
        Ok(entry)
    }

    pub async fn create_index_by_ref(
        &self,
        parent_ref: &str,
        title: Option<String>,
        target_ref: Option<&str>,
        target_type: Option<TargetType>,
    ) -> Result<Index, String> {
        let parent_id = self.resolve_index_ref(parent_ref).await?;
        let target = match target_type {
            Some(TargetType::Knowledge) => match target_ref {
                Some(r) => Some(self.resolve_knowledge(r).await?),
                None => None,
            },
            _ => None,
        };
        let title = title.or_else(|| target_ref.map(|s| s.to_string()));
        self.create_index(parent_id, title, target, target_type)
            .await
    }

    async fn resolve_index_ref(&self, parent_ref: &str) -> Result<Uuid, String> {
        let trimmed = parent_ref.trim();
        if trimmed.is_empty() {
            return Err("parent_ref must not be empty".to_string());
        }
        if trimmed.starts_with('/') || trimmed.contains('/') {
            return self.resolve_path(trimmed).await;
        }
        self.resolve_index(trimmed).await
    }

    pub async fn link_orphans(
        &self,
        parent_ref: &str,
        knowledge_titles: &[&str],
    ) -> Result<Vec<String>, String> {
        let parent_id = self.resolve_index(parent_ref).await?;
        let mut linked = Vec::new();
        for title in knowledge_titles {
            let target_id = match self.resolve_knowledge(title).await {
                Ok(id) => id,
                Err(_) => continue,
            };
            let idx = self
                .create_index(
                    parent_id,
                    Some(title.to_string()),
                    Some(target_id),
                    Some(TargetType::Knowledge),
                )
                .await?;
            linked.push(idx.title.unwrap_or_else(|| title.to_string()));
        }
        Ok(linked)
    }

    pub async fn delete_index(&self, title: &str) -> Result<(), String> {
        let conn = self.storage.conn();
        let idx = repo::index_find_by_title(conn, title)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("index '{title}' not found"))?;

        if idx.parent_id.is_none() {
            return Err("cannot delete root index".into());
        }
        if idx.target_type == TargetType::Knowledge {
            return Err(format!(
                "index '{title}' is a knowledge mount (target_type=knowledge); \
                 refusing to delete it because that would leave the Knowledge \
                 orphaned. Use `kms_delete_knowledge` to remove the Knowledge \
                 itself, or `kms_detach_knowledge` to unmount it."
            ));
        }

        let children = repo::index_children_of(conn, Some(idx.id))
            .await
            .map_err(|e| e.to_string())?;
        if !children.is_empty() {
            let preview: Vec<String> = children
                .iter()
                .take(5)
                .map(|c| c.title.clone().unwrap_or_else(|| "(unnamed)".to_string()))
                .collect();
            let suffix = if children.len() > preview.len() {
                format!(", …({} more)", children.len() - preview.len())
            } else {
                String::new()
            };
            return Err(format!(
                "index '{title}' is not empty ({} child(ren): {}{}). \
                 Refusing to delete. Move them first with kms_move_children / kms_move_index, \
                 delete them, then retry.",
                children.len(),
                preview.join(", "),
                suffix,
            ));
        }

        repo::index_delete(conn, idx.id)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(parent_id) = idx.parent_id {
            repo::index_reindex_positions(conn, Some(parent_id))
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn detach_knowledge_index(&self, title: &str) -> Result<Uuid, String> {
        let conn = self.storage.conn();
        let idx = repo::index_find_by_title(conn, title)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("index '{title}' not found"))?;

        if idx.parent_id.is_none() {
            return Err("cannot detach the root index".into());
        }
        if idx.target_type != TargetType::Knowledge {
            return Err(format!(
                "index '{title}' is a Group, not a knowledge mount; \
                 nothing to detach. Use `kms_delete_index` instead."
            ));
        }
        let knowledge_id = idx.target.ok_or_else(|| {
            format!(
                "index '{title}' is marked as knowledge-typed but has no target; \
                 this is a data inconsistency."
            )
        })?;

        let children = repo::index_children_of(conn, Some(idx.id))
            .await
            .map_err(|e| e.to_string())?;
        if !children.is_empty() {
            return Err(format!(
                "index '{title}' has {} child(ren); a knowledge mount must be a leaf.",
                children.len()
            ));
        }

        let parent_id = idx.parent_id;
        repo::index_delete(conn, idx.id)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(pid) = parent_id {
            repo::index_reindex_positions(conn, Some(pid))
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(knowledge_id)
    }

    pub async fn get_index(&self, id: Uuid) -> Result<Index, String> {
        repo::index_get(self.storage.conn(), id)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn get_children(&self, parent_id: Option<Uuid>) -> Result<Vec<Index>, String> {
        repo::index_children_of(self.storage.conn(), parent_id)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn find_child_by_title(
        &self,
        parent_id: Uuid,
        title: &str,
    ) -> Result<Option<Uuid>, String> {
        let children = self.get_children(Some(parent_id)).await?;
        Ok(children
            .iter()
            .find(|c| c.title.as_deref() == Some(title))
            .map(|c| c.id))
    }

    pub async fn get_entity_knowledge_under_index(
        &self,
        index_title: &str,
        entity_name: &str,
    ) -> Result<Vec<Knowledge>, String> {
        let index_id = self.resolve_index(index_title).await?;
        let entity_id = self.resolve(entity_name).await?;
        let knowledge_ids = repo::index_subtree_knowledge_ids(self.storage.conn(), index_id)
            .await
            .map_err(|e| e.to_string())?;

        let mut results = Vec::new();
        for kid in knowledge_ids {
            if let Ok(k) = self.get_knowledge(kid).await {
                if k.entities.contains(&entity_id) {
                    results.push(k);
                }
            }
        }
        Ok(results)
    }

    pub async fn get_entity_referencing_knowledge(
        &self,
        entity_id: Uuid,
    ) -> Result<Vec<Knowledge>, String> {
        repo::knowledge_find_by_entity(self.storage.conn(), entity_id)
            .await
            .map_err(|e| e.to_string())
    }

    // ── Path resolution ──

    pub async fn resolve_path(&self, path: &str) -> Result<Uuid, String> {
        let current = self.get_pointer().await;
        let trimmed = path.trim();
        if trimmed.is_empty() || trimmed == "." {
            return Ok(current);
        }
        if trimmed == ".." {
            let node = self.get_index(current).await?;
            return node
                .parent_id
                .ok_or_else(|| "already at root, cannot go to parent".to_string());
        }

        let (base_id, segments) = if trimmed.starts_with('/') {
            let root = self.find_root().await?;
            (root.id, trimmed[1..].split('/').collect::<Vec<_>>())
        } else if trimmed.starts_with("../") {
            let node = self.get_index(current).await?;
            match node.parent_id {
                Some(pid) => (pid, trimmed[3..].split('/').collect::<Vec<_>>()),
                None => return Err("already at root, cannot go to parent".into()),
            }
        } else if trimmed.contains('/') {
            (current, trimmed.split('/').collect::<Vec<_>>())
        } else {
            (current, vec![trimmed])
        };

        let mut pointer = base_id;
        for seg in segments {
            let seg = seg.trim();
            if seg.is_empty() {
                continue;
            }
            if seg == ".." {
                let node = self.get_index(pointer).await?;
                pointer = node
                    .parent_id
                    .ok_or_else(|| "already at root, cannot go to parent".to_string())?;
            } else {
                let children = self.get_children(Some(pointer)).await?;
                match children.iter().find(|c| c.title.as_deref() == Some(seg)) {
                    Some(child) => pointer = child.id,
                    None => return Err(format!("segment '{seg}' not found as child of node")),
                }
            }
        }
        Ok(pointer)
    }

    pub async fn navigate(&self, path: &str) -> Result<String, String> {
        let pointer = self.resolve_path(path).await?;
        self.set_pointer(pointer).await;
        self.render_location().await
    }

    async fn descend(&self, parent_id: Uuid, title: &str) -> Result<Uuid, String> {
        let children = self.get_children(Some(parent_id)).await?;
        children
            .iter()
            .find(|c| c.title.as_deref() == Some(title))
            .map(|c| c.id)
            .ok_or_else(|| format!("segment '{title}' not found as child of current node"))
    }

    // ── Move operations ──

    pub async fn move_children(
        &self,
        source_path: &str,
        remount_path: &str,
        new_group_title: &str,
        child_titles: &[String],
    ) -> Result<MoveChildrenResult, String> {
        if child_titles.is_empty() {
            return Err("child_titles must not be empty".into());
        }

        let source_id = self
            .resolve_path(source_path)
            .await
            .map_err(|e| addressing_hint("source_path", source_path, &e))?;
        let remount_id = self
            .resolve_path(remount_path)
            .await
            .map_err(|e| addressing_hint("remount_path", remount_path, &e))?;

        let children = self.get_children(Some(source_id)).await?;
        let mut child_indices: Vec<Index> = Vec::new();
        for title in child_titles {
            let found = children
                .iter()
                .find(|c| c.title.as_deref() == Some(title.as_str()))
                .ok_or_else(|| {
                    let mut msg = format!("'{title}' is not a direct child of '{source_path}'");
                    if looks_like_bare_title(source_path) {
                        msg.push_str(&format!(
                            " — note: source_path='{src}' looks like a bare title, \
                             not an absolute path. Pass an absolute path like \
                             '/parent/{src}' instead.",
                            src = source_path
                        ));
                    } else {
                        msg.push_str(" (verify source_path resolves to the parent you intended.)");
                    }
                    msg
                })?;
            child_indices.push(found.clone());
        }

        // Find-or-create destination group.
        let (new_group_id, group_created) = match self
            .find_child_by_title(remount_id, new_group_title)
            .await?
        {
            Some(existing_id) => {
                let existing = self.get_index(existing_id).await?;
                if existing.target_type != TargetType::Group {
                    return Err(format!(
                        "cannot reuse '{new_group_title}' under '{remount_path}' as the \
                         destination group: it is already a {:?}-typed index (id {existing_id})",
                        existing.target_type
                    ));
                }
                (existing_id, false)
            }
            None => {
                let created = self
                    .create_index(
                        remount_id,
                        Some(new_group_title.to_string()),
                        None,
                        Some(TargetType::Group),
                    )
                    .await?;
                (created.id, true)
            }
        };

        let conn = self.storage.conn();
        for (i, child) in child_indices.iter().enumerate() {
            repo::index_reparent(conn, child.id, new_group_id, i as i64)
                .await
                .map_err(|e| e.to_string())?;
        }

        if source_id != remount_id {
            repo::index_reindex_positions(conn, Some(source_id))
                .await
                .map_err(|e| e.to_string())?;
        }
        repo::index_reindex_positions(conn, Some(remount_id))
            .await
            .map_err(|e| e.to_string())?;
        repo::index_reindex_positions(conn, Some(new_group_id))
            .await
            .map_err(|e| e.to_string())?;

        self.set_pointer(new_group_id).await;
        let location = self.render_location().await?;
        Ok(MoveChildrenResult {
            location,
            new_group_id,
            group_created,
        })
    }

    pub async fn move_index(
        &self,
        index_path: &str,
        new_parent_path: &str,
    ) -> Result<String, String> {
        let idx_id = self
            .resolve_path(index_path)
            .await
            .map_err(|e| addressing_hint("index_path", index_path, &e))?;
        let new_parent_id = self
            .resolve_path(new_parent_path)
            .await
            .map_err(|e| addressing_hint("new_parent_path", new_parent_path, &e))?;

        let conn = self.storage.conn();
        let idx = repo::index_get(conn, idx_id)
            .await
            .map_err(|e| e.to_string())?;
        if idx.parent_id.is_none() {
            return Err("cannot move the root index".into());
        }
        if idx.parent_id == Some(new_parent_id) {
            return Err(format!(
                "index at '{index_path}' is already under '{new_parent_path}'"
            ));
        }

        let old_parent_id = idx.parent_id;
        let target_children = repo::index_children_of(conn, Some(new_parent_id))
            .await
            .map_err(|e| e.to_string())?;
        let new_position = target_children.len() as i64;

        repo::index_reparent(conn, idx_id, new_parent_id, new_position)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(oid) = old_parent_id {
            repo::index_reindex_positions(conn, Some(oid))
                .await
                .map_err(|e| e.to_string())?;
        }
        repo::index_reindex_positions(conn, Some(new_parent_id))
            .await
            .map_err(|e| e.to_string())?;

        self.set_pointer(idx_id).await;
        let location = self.render_location().await?;
        let new_parent_label = repo::index_get(conn, new_parent_id)
            .await
            .map(|n| n.title.unwrap_or_else(|| new_parent_id.to_string()))
            .unwrap_or_else(|_| new_parent_id.to_string());
        Ok(format!(
            "moved '{index_path}' under '{new_parent_label}'\n{location}"
        ))
    }

    pub async fn merge_subtree(
        &self,
        sub_root_id: Uuid,
        target_parent_id: Uuid,
    ) -> Result<usize, String> {
        if sub_root_id == target_parent_id {
            return Err("sub_root and target_parent must differ".into());
        }
        let conn = self.storage.conn();
        let sub_root = repo::index_get(conn, sub_root_id)
            .await
            .map_err(|e| e.to_string())?;
        if sub_root.parent_id.is_none() {
            return Err("cannot merge the system root".into());
        }

        let children = repo::index_children_of(conn, Some(sub_root_id))
            .await
            .map_err(|e| e.to_string())?;
        let existing_count = repo::index_children_of(conn, Some(target_parent_id))
            .await
            .map_err(|e| e.to_string())?
            .len();

        for (i, child) in children.iter().enumerate() {
            repo::index_reparent(
                conn,
                child.id,
                target_parent_id,
                (existing_count + i) as i64,
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        if !children.is_empty() {
            repo::index_reindex_positions(conn, Some(target_parent_id))
                .await
                .map_err(|e| e.to_string())?;
        }
        repo::index_delete(conn, sub_root_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(children.len())
    }

    // ── Diagnostics ──

    pub async fn diagnose(&self) -> Result<Vec<Diagnostic>, String> {
        diagnostics::run_diagnostics(&self.storage).await
    }

    // ── Pointer management ──

    pub fn with_pointer(&self, pointer: Uuid) -> KmsService {
        KmsService {
            inner: Arc::new(Inner {
                pointer: RwLock::new(pointer),
            }),
            storage: self.storage.clone(),
            documents: self.documents.clone(),
        }
    }

    // ── Rendering ──

    pub async fn render_location(&self) -> Result<String, String> {
        let current = self.get_pointer().await;
        let path = self.ancestor_path(current).await?;
        let current_id = path.last().map(|n| n.id).unwrap_or(current);
        let children = self.get_children(Some(current_id)).await?;

        let mut s = String::new();
        for (i, node) in path.iter().enumerate() {
            let title = node.title.as_deref().unwrap_or("(unnamed)");
            let is_root = node.parent_id.is_none();
            let is_current = node.id == current;
            let is_last = i == path.len() - 1;

            if i > 0 {
                s.push_str(if is_last {
                    "  └── "
                } else {
                    "  ├── "
                });
            } else {
                s.push_str("## ");
            }
            if is_current {
                s.push_str(&format!("**{title}**"));
            } else {
                s.push_str(title);
            }
            if is_root && i == 0 {
                s.push_str(" (system root, read-only)");
            }
            s.push('\n');
        }

        if path.is_empty() {
            s.push_str("## (pointer not initialized)\n");
        } else if children.is_empty() {
            s.push_str("      (empty)\n");
        } else {
            let last = children.len() - 1;
            for (i, c) in children.iter().enumerate() {
                let t = c.title.as_deref().unwrap_or("(unnamed)");
                let connector = if i == last {
                    "  └── "
                } else {
                    "  ├── "
                };
                let suffix = match c.target_type {
                    TargetType::Group => "",
                    TargetType::Knowledge => " [knowledge]",
                };
                s.push_str(&format!("{connector}{t}{suffix}\n"));
            }
        }
        Ok(s)
    }

    pub async fn render_full_tree(&self) -> Result<String, String> {
        let root = self.find_root().await?;
        let mut s = String::new();
        let mut stack: Vec<(Index, String, String)> = vec![(root, String::new(), String::new())];

        while let Some((node, indent, connector)) = stack.pop() {
            let title = node.title.as_deref().unwrap_or("(unnamed)");
            let is_root = node.parent_id.is_none();

            if is_root {
                s.push_str(&format!("## {title} (system root)\n"));
            } else {
                let suffix = match node.target_type {
                    TargetType::Group => "",
                    TargetType::Knowledge => " [knowledge]",
                };
                s.push_str(&format!("{indent}{connector}{title}{suffix}\n"));
            }

            let children = match self.get_children(Some(node.id)).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            if children.is_empty() {
                continue;
            }
            let n = children.len();
            for i in (0..n).rev() {
                let child = &children[i];
                let is_last = i == n - 1;
                let c_connector = if is_last { "└── " } else { "├── " };
                let branch = if is_last { "   " } else { "│  " };
                let c_indent = if is_root {
                    branch.to_string()
                } else {
                    format!("{indent}{branch}")
                };
                stack.push((child.clone(), c_indent, c_connector.to_string()));
            }
        }
        Ok(s)
    }

    async fn ancestor_path(&self, target_id: Uuid) -> Result<Vec<Index>, String> {
        let mut path = Vec::new();
        let mut id = target_id;
        loop {
            let node = self.get_index(id).await?;
            let parent_id = node.parent_id;
            path.push(node);
            match parent_id {
                Some(pid) => id = pid,
                None => {
                    path.reverse();
                    return Ok(path);
                }
            }
        }
    }

    // ── Local-view (stateless) API ──

    pub async fn get_local_view(&self, node_id: Uuid) -> Result<LocalView, String> {
        let conn = self.storage.conn();

        // 1) ancestor path
        let path_rows = repo::index_ancestor_path_rows(conn, node_id)
            .await
            .map_err(|e| e.to_string())?;

        let path: Vec<Index> = path_rows
            .iter()
            .map(|r| {
                Ok::<Index, String>(Index {
                    id: Uuid::parse_str(&r.id).map_err(|e| e.to_string())?,
                    title: r.title.clone(),
                    target: r.target.as_deref().and_then(|t| Uuid::parse_str(t).ok()),
                    target_type: TargetType::from_str(r.target_type.as_deref().unwrap_or("group")),
                    parent_id: r.parent_id.as_deref().and_then(|p| Uuid::parse_str(p).ok()),
                    position: r.position,
                })
            })
            .collect::<Result<_, _>>()?;

        let node = path
            .first()
            .cloned()
            .ok_or_else(|| format!("node {node_id} not found"))?;

        // 2) direct children
        let child_rows = repo::index_child_rows(conn, node_id)
            .await
            .map_err(|e| e.to_string())?;
        let mut children: Vec<IndexView> = Vec::with_capacity(child_rows.len());
        for r in child_rows {
            let id = Uuid::parse_str(&r.id).map_err(|e| e.to_string())?;
            let title = r.title.unwrap_or_else(|| "(unnamed)".to_string());
            let target_type = TargetType::from_str(r.target_type.as_deref().unwrap_or("group"));
            children.push(IndexView {
                id,
                title,
                target_type,
                position: r.position,
            });
        }

        // 3) subtree statistics
        let stats = repo::index_subtree_stats(conn, node_id, SUBTREE_TITLES_LIMIT)
            .await
            .map_err(|e| e.to_string())?;

        // 4) sibling count
        let sibling_count = repo::index_sibling_count(conn, node_id)
            .await
            .map_err(|e| e.to_string())?;

        Ok(LocalView {
            node,
            path,
            children,
            sibling_count,
            subtree_summary: SubtreeSummary {
                total_nodes: stats.total_nodes,
                knowledge_count: stats.knowledge_count,
                group_count: stats.group_count,
                max_depth: stats.max_depth,
                knowledge_titles: stats.knowledge_titles,
                truncated: stats.truncated,
            },
        })
    }

    pub async fn get_local_view_by_path(&self, path: &str) -> Result<LocalView, String> {
        let target_id = self.resolve_path_id(path).await?;
        self.get_local_view(target_id).await
    }

    pub async fn get_subtree_knowledge(&self, node_id: Uuid) -> Result<Vec<Knowledge>, String> {
        let ids = repo::index_subtree_knowledge_ids(self.storage.conn(), node_id)
            .await
            .map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(ids.len());
        for kid in ids {
            match self.get_knowledge(kid).await {
                Ok(k) => out.push(k),
                Err(_) => continue,
            }
        }
        Ok(out)
    }

    pub async fn get_subtree_knowledge_by_path(
        &self,
        path: &str,
    ) -> Result<Vec<Knowledge>, String> {
        let target_id = self.resolve_path_id(path).await?;
        self.get_subtree_knowledge(target_id).await
    }

    pub async fn search_knowledge_titles(
        &self,
        node_id: Uuid,
        keyword: &str,
    ) -> Result<Vec<Knowledge>, String> {
        let all = self.get_subtree_knowledge(node_id).await?;
        let kw = keyword.to_lowercase();
        Ok(all
            .into_iter()
            .filter(|k| k.title.to_lowercase().contains(&kw))
            .collect())
    }

    pub async fn search_knowledge_content(
        &self,
        node_id: Uuid,
        keyword: &str,
        top_k: usize,
    ) -> Result<Vec<KnowledgeContentHit>, String> {
        let all = self.get_subtree_knowledge(node_id).await?;
        let kw = keyword.to_lowercase();
        let mut hits: Vec<KnowledgeContentHit> = all
            .into_iter()
            .filter_map(|k| {
                let content = k.content.as_ref()?;
                let lower = content.to_lowercase();
                let count = lower.matches(&kw).count();
                if count == 0 {
                    return None;
                }
                let snippet = extract_content_snippet(content, &lower, &kw, 200);
                Some(KnowledgeContentHit {
                    knowledge: k.into(),
                    snippet,
                    match_count: count,
                })
            })
            .collect();
        hits.sort_by(|a, b| {
            b.match_count
                .cmp(&a.match_count)
                .then_with(|| a.knowledge.title.cmp(&b.knowledge.title))
        });
        hits.truncate(top_k);
        Ok(hits)
    }

    async fn resolve_path_id(&self, path: &str) -> Result<Uuid, String> {
        if path.is_empty() {
            return Err("path is empty".into());
        }

        if let Some(stripped) = path.strip_prefix('/') {
            let root = self.find_root().await?;
            if stripped.is_empty() {
                return Ok(root.id);
            }
            let mut id = root.id;
            for seg in stripped.split('/') {
                let seg = seg.trim();
                if seg.is_empty() || seg == "." {
                    continue;
                }
                id = self.descend(id, seg).await?;
            }
            return Ok(id);
        }

        if path == ".." {
            return Err(
                "'..' requires a current pointer; use an absolute path or a sub-segment".into(),
            );
        }

        if let Some(stripped) = path.strip_prefix("../") {
            let mut id = self.get_pointer().await;
            for _ in 0..path.matches("../").count() {
                let node = self.get_index(id).await?;
                id = node
                    .parent_id
                    .ok_or_else(|| "already at root, cannot go to parent".to_string())?;
            }
            for seg in stripped.split('/') {
                let seg = seg.trim();
                if seg.is_empty() {
                    continue;
                }
                if seg == ".." {
                    let node = self.get_index(id).await?;
                    id = node
                        .parent_id
                        .ok_or_else(|| "already at root, cannot go to parent".to_string())?;
                } else {
                    id = self.descend(id, seg).await?;
                }
            }
            return Ok(id);
        }

        if path.contains('/') {
            let mut id = self.get_pointer().await;
            for seg in path.split('/') {
                let seg = seg.trim();
                if seg.is_empty() {
                    continue;
                }
                id = self.descend(id, seg).await?;
            }
            return Ok(id);
        }

        let id = self.get_pointer().await;
        self.descend(id, path.trim()).await
    }
}

// ─────────────────────────── free functions ───────────────────────────

async fn ensure_root_index(storage: &Storage) -> Result<Uuid, String> {
    let conn = storage.conn();

    let mut rows = conn
        .query(
            "SELECT id FROM indexes WHERE parent_id IS NULL LIMIT 1",
            turso::params_from_iter([] as [turso::Value; 0]),
        )
        .await
        .map_err(|e| e.to_string())?;

    match rows.next().await.map_err(|e| e.to_string())? {
        Some(row) => {
            let id_str = match row.get_value(0) {
                Ok(turso::Value::Text(s)) => s,
                _ => return Err("root index id is not text".into()),
            };
            Uuid::parse_str(&id_str).map_err(|e| e.to_string())
        }
        None => {
            let id = Uuid::new_v4();
            let entry = Index {
                id,
                title: Some("Root".to_string()),
                target: None,
                target_type: TargetType::Group,
                parent_id: None,
                position: 0,
            };
            repo::index_create(conn, &entry)
                .await
                .map_err(|e| e.to_string())?;
            Ok(id)
        }
    }
}

fn looks_like_bare_title(p: &str) -> bool {
    let t = p.trim();
    !t.is_empty() && !t.starts_with('/') && !t.contains('/') && t != ".." && t != "."
}

fn addressing_hint(param_name: &str, value: &str, err: &str) -> String {
    if looks_like_bare_title(value) {
        format!(
            "{err}\n\nhint: `{param}` expects an ABSOLUTE PATH (starting with `/`), not a bare title. \
             '{value}' was resolved against the implicit pointer and not found there. \
             Use an absolute path like '/parent/{value}', or call `kms_local` / \
             `kms_search_subtree('/', '{value}')` first to discover the correct full path.",
            err = err,
            param = param_name,
            value = value,
        )
    } else {
        format!("{err} (parameter `{param_name}` was {value:?})")
    }
}

fn extract_content_snippet(content: &str, lower: &str, kw: &str, window: usize) -> String {
    let pos = lower.find(kw).unwrap_or(0);
    let char_indices: Vec<usize> = content.char_indices().map(|(i, _)| i).collect();
    let start = char_indices
        .iter()
        .rev()
        .find(|&&ci| ci <= pos.saturating_sub(window))
        .copied()
        .unwrap_or(0);
    let end = char_indices
        .iter()
        .find(|&&ci| ci >= (pos + kw.len() + window).min(content.len()))
        .copied()
        .unwrap_or(content.len());

    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < content.len() { "…" } else { "" };
    format!("{prefix}{}{suffix}", &content[start..end])
}
