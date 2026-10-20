//! KmsService — high-level knowledge management operations.
//!
//! Ported from dendrite's `service.rs`. Business logic is identical; only
//! the storage layer is swapped from sqlx to Turso, and the corpus
//! dependency is made optional via [`KmsDocumentStore`].

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::Diagnostic;
use crate::Storage;
use crate::diagnostics;
use crate::error::Result;
use crate::storage::repo;
use crate::storage::types::{Index, TargetType};

mod entity;
mod index;
mod knowledge;
mod local_view;
mod move_ops;
mod path;
mod render;

pub use entity::EntityFilter;
pub use knowledge::{BatchKnowledgeResult, BatchStatus, KnowledgeContentHit, KnowledgeView};
pub use move_ops::MoveChildrenResult;

// ─────────────────────────── document store trait ───────────────────────────

/// Trait for validating source-document references. Implemented by the
/// corpus crate (Phase 2); when `None`, source-document validation is
/// skipped.
#[async_trait]
pub trait KmsDocumentStore: Send + Sync {
    async fn document_exists(&self, doc_id: Uuid) -> bool;
}

// ─────────────────────────── KmsService ───────────────────────────

#[derive(Clone)]
pub struct KmsService {
    storage: Storage,
    documents: Option<Arc<dyn KmsDocumentStore>>,
}

impl KmsService {
    /// Open a KMS at `db_path`. Pass `None` for `documents` to skip
    /// source-document validation (Phase 1 mode).
    pub async fn new(db_path: &str, documents: Option<Arc<dyn KmsDocumentStore>>) -> Result<Self> {
        let storage = Storage::new(db_path).await?;
        ensure_root_index(&storage).await?;
        Ok(KmsService { storage, documents })
    }

    /// Create a KMS from an already-open `Storage` (e.g. in-memory for tests).
    pub async fn from_storage(storage: Storage) -> Result<Self> {
        ensure_root_index(&storage).await?;
        Ok(KmsService {
            storage,
            documents: None,
        })
    }

    pub async fn find_root(&self) -> Result<Index> {
        let index = repo::index_find_root(&*self.storage.conn().await).await?;
        Ok(index)
    }

    /// Delete a knowledge row by ID without touching its index mounts.
    /// Intended for maintenance rollback after a failed compound operation.
    pub async fn delete_knowledge_by_id(&self, id: Uuid) -> Result<()> {
        Ok(repo::knowledge_delete(&*self.storage.conn().await, id).await?)
    }

    /// Delete an index row by ID without business-tree guards.
    /// Intended for maintenance rollback after a failed compound operation.
    pub async fn delete_index_by_id(&self, id: Uuid) -> Result<()> {
        Ok(repo::index_delete(&*self.storage.conn().await, id).await?)
    }

    // ── Diagnostics ──

    pub async fn diagnose(&self) -> Result<Vec<Diagnostic>> {
        Ok(diagnostics::run_diagnostics(&self.storage).await?)
    }
}

async fn ensure_root_index(storage: &Storage) -> Result<Uuid> {
    let conn = &*storage.conn().await;

    let mut rows = conn
        .query(
            "SELECT id FROM indexes WHERE parent_id IS NULL LIMIT 1",
            turso::params_from_iter([] as [turso::Value; 0]),
        )
        .await?;

    match rows.next().await? {
        Some(row) => {
            let id_str = match row.get_value(0) {
                Ok(turso::Value::Text(s)) => s,
                _ => return Err("root index id is not text".into()),
            };
            Ok(Uuid::parse_str(&id_str)?)
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
            repo::index_create(conn, &entry).await?;
            Ok(id)
        }
    }
}
