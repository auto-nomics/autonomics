//! Name-to-id resolution and index path walking.

use uuid::Uuid;

use crate::error::{Error, Result};
use crate::storage::repo;

use super::KmsService;

// ── Name resolution ──
impl KmsService {
    pub async fn resolve(&self, name: &str) -> Result<Uuid> {
        let conn = &*self.storage.conn().await;
        if let Some(entity) = repo::entity_find_by_exact_name(conn, name).await? {
            return Ok(entity.id);
        }
        if let Some(idx) = repo::index_find_by_title(conn, name).await? {
            return Ok(idx.id);
        }
        if let Some(knowledge) = repo::knowledge_find_by_title(conn, name).await? {
            return Ok(knowledge.id);
        }
        Err(Error::from(format!("cannot resolve: {name}")))
    }

    pub async fn resolve_index(&self, name: &str) -> Result<Uuid> {
        if let Some(idx) = repo::index_find_by_title(&*self.storage.conn().await, name).await? {
            return Ok(idx.id);
        }
        Err(Error::from(format!("index not found: {name}")))
    }

    pub async fn resolve_knowledge(&self, title: &str) -> Result<Uuid> {
        if let Some(knowledge) =
            repo::knowledge_find_by_title(&*self.storage.conn().await, title).await?
        {
            return Ok(knowledge.id);
        }
        Err(Error::from(format!("knowledge not found: {title}")))
    }
}

// ── Path resolution ──
impl KmsService {
    /// Resolve an index path. Paths are rooted at the system root when they
    /// begin with `/`; otherwise they are resolved as sub-segments of `/`.
    pub async fn resolve_path(&self, path: &str) -> Result<Uuid> {
        let root = self.find_root().await?;
        let trimmed = path.trim();
        if trimmed.is_empty() || trimmed == "/" || trimmed == "." {
            return Ok(root.id);
        }

        let normalized = if let Some(stripped) = trimmed.strip_prefix('/') {
            stripped
        } else {
            trimmed
        };

        let mut pointer = root.id;
        for segment in normalized.split('/') {
            let segment = segment.trim();
            if segment.is_empty() || segment == "." {
                continue;
            }
            if segment == ".." {
                let node = self.get_index(pointer).await?;
                pointer = node
                    .parent_id
                    .ok_or_else(|| "already at root, cannot go to parent".to_string())?;
            } else {
                pointer = self.descend(pointer, segment).await?;
            }
        }
        Ok(pointer)
    }

    async fn descend(&self, parent_id: Uuid, title: &str) -> Result<Uuid> {
        let children = self.get_children(Some(parent_id)).await?;
        children
            .iter()
            .find(|c| c.title.as_deref() == Some(title))
            .map(|c| c.id)
            .ok_or_else(|| {
                Error::from(format!(
                    "segment '{title}' not found as child of current node"
                ))
            })
    }

    pub(super) async fn resolve_path_id(&self, path: &str) -> Result<Uuid> {
        if path.trim().is_empty() {
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

        if let Some(stripped) = path.strip_prefix("../") {
            let root = self.find_root().await?;
            let mut id = root.id;
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
            let root = self.find_root().await?;
            let mut id = root.id;
            for seg in path.split('/') {
                let seg = seg.trim();
                if seg.is_empty() {
                    continue;
                }
                id = self.descend(id, seg).await?;
            }
            return Ok(id);
        }

        let root = self.find_root().await?;
        self.descend(root.id, path.trim()).await
    }
}

pub(super) fn looks_like_bare_title(p: &str) -> bool {
    let t = p.trim();
    !t.is_empty() && !t.starts_with('/') && !t.contains('/') && t != ".." && t != "."
}

pub(super) fn addressing_hint(param_name: &str, value: &str, err: &Error) -> String {
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
