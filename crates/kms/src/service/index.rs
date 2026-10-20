//! Index tree CRUD: groups, knowledge mounts, and subtree merges.

use uuid::Uuid;

use crate::error::{Error, Result};
use crate::storage::repo;
use crate::storage::types::{Index, Knowledge, TargetType};

use super::KmsService;

// ── Index ──
impl KmsService {
    pub async fn create_index_root(&self, title: &str) -> Result<Index> {
        let id = Uuid::new_v4();
        let entry = Index {
            id,
            title: Some(title.to_string()),
            target: None,
            target_type: TargetType::Group,
            parent_id: None,
            position: 0,
        };
        repo::index_create(&*self.storage.conn().await, &entry).await?;
        Ok(entry)
    }

    pub async fn create_index(
        &self,
        parent_id: Uuid,
        title: Option<String>,
        target: Option<Uuid>,
        target_type: Option<TargetType>,
    ) -> Result<Index> {
        let conn = &*self.storage.conn().await;
        let parent = repo::index_get(conn, parent_id).await?;
        let siblings = repo::index_children_of(conn, Some(parent_id)).await?;

        if let Some(new_title) = title.as_deref() {
            if let Some(conflict) = siblings
                .iter()
                .find(|c| c.title.as_deref() == Some(new_title))
            {
                let parent_label = parent.title.as_deref().unwrap_or("(unnamed parent)");
                return Err(Error::from(format!(
                    "duplicate child title '{new_title}' under parent '{parent_label}' \
                     (conflicts with existing index {})",
                    conflict.id
                )));
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
        repo::index_create(conn, &entry).await?;
        Ok(entry)
    }

    pub async fn create_index_by_ref(
        &self,
        parent_ref: &str,
        title: Option<String>,
        target_ref: Option<&str>,
        target_type: Option<TargetType>,
    ) -> Result<Index> {
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

    async fn resolve_index_ref(&self, parent_ref: &str) -> Result<Uuid> {
        let trimmed = parent_ref.trim();
        if trimmed.is_empty() {
            return Err("parent_ref must not be empty".into());
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
    ) -> Result<Vec<String>> {
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

    pub async fn delete_index(&self, title: &str) -> Result<()> {
        let conn = &*self.storage.conn().await;
        let idx = repo::index_find_by_title(conn, title)
            .await?
            .ok_or_else(|| format!("index '{title}' not found"))?;

        if idx.parent_id.is_none() {
            return Err("cannot delete root index".into());
        }
        if idx.target_type == TargetType::Knowledge {
            return Err(Error::from(format!(
                "index '{title}' is a knowledge mount (target_type=knowledge); \
                 refusing to delete it because that would leave the Knowledge \
                 orphaned. Use `kms_delete_knowledge` to remove the Knowledge \
                 itself, or `kms_detach_knowledge` to unmount it."
            )));
        }

        let children = repo::index_children_of(conn, Some(idx.id)).await?;
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
            return Err(Error::from(format!(
                "index '{title}' is not empty ({} child(ren): {}{}). \
                 Refusing to delete. Move them first with kms_move_children / kms_move_index, \
                 delete them, then retry.",
                children.len(),
                preview.join(", "),
                suffix,
            )));
        }

        repo::index_delete(conn, idx.id).await?;

        if let Some(parent_id) = idx.parent_id {
            repo::index_reindex_positions(conn, Some(parent_id)).await?;
        }
        Ok(())
    }

    pub async fn detach_knowledge_index(&self, title: &str) -> Result<Uuid> {
        let conn = &*self.storage.conn().await;
        let idx = repo::index_find_by_title(conn, title)
            .await?
            .ok_or_else(|| format!("index '{title}' not found"))?;

        if idx.parent_id.is_none() {
            return Err("cannot detach the root index".into());
        }
        if idx.target_type != TargetType::Knowledge {
            return Err(Error::from(format!(
                "index '{title}' is a Group, not a knowledge mount; \
                 nothing to detach. Use `kms_delete_index` instead."
            )));
        }
        let knowledge_id = idx.target.ok_or_else(|| {
            format!(
                "index '{title}' is marked as knowledge-typed but has no target; \
                 this is a data inconsistency."
            )
        })?;

        let children = repo::index_children_of(conn, Some(idx.id)).await?;
        if !children.is_empty() {
            return Err(Error::from(format!(
                "index '{title}' has {} child(ren); a knowledge mount must be a leaf.",
                children.len()
            )));
        }

        let parent_id = idx.parent_id;
        repo::index_delete(conn, idx.id).await?;
        if let Some(pid) = parent_id {
            repo::index_reindex_positions(conn, Some(pid)).await?;
        }
        Ok(knowledge_id)
    }

    pub async fn get_index(&self, id: Uuid) -> Result<Index> {
        Ok(repo::index_get(&*self.storage.conn().await, id).await?)
    }

    pub async fn get_children(&self, parent_id: Option<Uuid>) -> Result<Vec<Index>> {
        Ok(repo::index_children_of(&*self.storage.conn().await, parent_id).await?)
    }

    pub async fn find_child_by_title(&self, parent_id: Uuid, title: &str) -> Result<Option<Uuid>> {
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
    ) -> Result<Vec<Knowledge>> {
        let index_id = self.resolve_index(index_title).await?;
        let entity_id = self.resolve(entity_name).await?;
        let knowledge_ids =
            repo::index_subtree_knowledge_ids(&*self.storage.conn().await, index_id).await?;

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
}

// ── Subtree merge ──
impl KmsService {
    pub async fn merge_subtree(&self, sub_root_id: Uuid, target_parent_id: Uuid) -> Result<usize> {
        if sub_root_id == target_parent_id {
            return Err("sub_root and target_parent must differ".into());
        }
        let conn = &*self.storage.conn().await;
        let sub_root = repo::index_get(conn, sub_root_id).await?;
        if sub_root.parent_id.is_none() {
            return Err("cannot merge the system root".into());
        }

        let children = repo::index_children_of(conn, Some(sub_root_id)).await?;
        let existing_count = repo::index_children_of(conn, Some(target_parent_id))
            .await?
            .len();

        for (i, child) in children.iter().enumerate() {
            repo::index_reparent(
                conn,
                child.id,
                target_parent_id,
                (existing_count + i) as i64,
            )
            .await?;
        }
        if !children.is_empty() {
            repo::index_reindex_positions(conn, Some(target_parent_id)).await?;
        }
        repo::index_delete(conn, sub_root_id).await?;
        Ok(children.len())
    }
}

#[cfg(test)]
mod tests {
    use crate::service::test_support::{make_name, setup};
    use crate::storage::types::TargetType;

    #[tokio::test]
    async fn test_duplicate_child_title_rejected() {
        let svc = setup().await;
        let root = svc.find_root().await.unwrap();

        svc.create_index(root.id, Some("Same Title".into()), None, None)
            .await
            .unwrap();

        let err = svc
            .create_index(root.id, Some("Same Title".into()), None, None)
            .await;
        assert!(err.is_err());
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("duplicate child title")
        );
    }

    #[tokio::test]
    async fn test_delete_index_cascades_children() {
        let svc = setup().await;
        let root = svc.find_root().await.unwrap();

        let parent = svc
            .create_index(root.id, Some("Parent".into()), None, None)
            .await
            .unwrap();
        let _child = svc
            .create_index(
                parent.id,
                Some("Child".into()),
                None,
                Some(TargetType::Group),
            )
            .await
            .unwrap();

        // Non-empty: should refuse.
        let err = svc.delete_index("Parent").await;
        assert!(err.is_err());

        // Delete child first, then parent.
        svc.delete_index("Child").await.unwrap();
        svc.delete_index("Parent").await.unwrap();
    }

    #[tokio::test]
    async fn test_detach_and_link_orphan() {
        let svc = setup().await;
        let root = svc.find_root().await.unwrap();
        let (entity, _) = svc
            .create_entity(vec![make_name("Vue")], "Progressive framework")
            .await
            .unwrap();

        let knowledge = svc
            .create_knowledge(
                "Vue · 响应式原理",
                vec![entity.id],
                Some("Reactive...".into()),
            )
            .await
            .unwrap();

        let group = svc
            .create_index(root.id, Some("Frameworks".into()), None, None)
            .await
            .unwrap();
        svc.create_index(
            group.id,
            Some("Vue · 响应式原理".into()),
            Some(knowledge.id),
            Some(TargetType::Knowledge),
        )
        .await
        .unwrap();

        // Detach.
        let orphaned_id = svc
            .detach_knowledge_index("Vue · 响应式原理")
            .await
            .unwrap();
        assert_eq!(orphaned_id, knowledge.id);

        // The knowledge should now be orphaned.
        let diags = svc.diagnose().await.unwrap();
        assert!(diags.iter().any(|d| d.code == "knowledge.orphan"));

        // Re-link.
        svc.link_orphans("Frameworks", &["Vue · 响应式原理"])
            .await
            .unwrap();
        let diags2 = svc.diagnose().await.unwrap();
        assert!(!diags2.iter().any(|d| d.code == "knowledge.orphan"));
    }
}
