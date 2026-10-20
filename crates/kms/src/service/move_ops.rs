//! Moving and re-parenting index subtrees.

use serde::Serialize;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::storage::repo;
use crate::storage::types::{Index, TargetType};

use super::KmsService;
use super::path::{addressing_hint, looks_like_bare_title};

// ─────────────────────────── result types ───────────────────────────

/// Outcome of a `move_children` call: where the moved nodes ended up.
#[derive(Debug, Clone, Serialize)]
pub struct MoveChildrenResult {
    pub location: String,
    pub new_group_id: Uuid,
    pub group_created: bool,
}

// ── Move operations ──
impl KmsService {
    pub async fn move_children(
        &self,
        source_path: &str,
        remount_path: &str,
        new_group_title: &str,
        child_titles: &[String],
    ) -> Result<MoveChildrenResult> {
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
                    return Err(Error::from(format!(
                        "cannot reuse '{new_group_title}' under '{remount_path}' as the \
                         destination group: it is already a {:?}-typed index (id {existing_id})",
                        existing.target_type
                    )));
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

        {
            let conn = &*self.storage.conn().await;
            for (i, child) in child_indices.iter().enumerate() {
                repo::index_reparent(conn, child.id, new_group_id, i as i64).await?;
            }

            if source_id != remount_id {
                repo::index_reindex_positions(conn, Some(source_id)).await?;
            }
            repo::index_reindex_positions(conn, Some(remount_id)).await?;
            repo::index_reindex_positions(conn, Some(new_group_id)).await?;
        }

        let location = self.render_location(new_group_id).await?;
        Ok(MoveChildrenResult {
            location,
            new_group_id,
            group_created,
        })
    }

    pub async fn move_index(&self, index_path: &str, new_parent_path: &str) -> Result<String> {
        let idx_id = self
            .resolve_path(index_path)
            .await
            .map_err(|e| addressing_hint("index_path", index_path, &e))?;
        let new_parent_id = self
            .resolve_path(new_parent_path)
            .await
            .map_err(|e| addressing_hint("new_parent_path", new_parent_path, &e))?;

        let new_parent_label;
        {
            let conn = &*self.storage.conn().await;
            let idx = repo::index_get(conn, idx_id).await?;
            if idx.parent_id.is_none() {
                return Err("cannot move the root index".into());
            }
            if idx.parent_id == Some(new_parent_id) {
                return Err(Error::from(format!(
                    "index at '{index_path}' is already under '{new_parent_path}'"
                )));
            }

            let old_parent_id = idx.parent_id;
            let target_children = repo::index_children_of(conn, Some(new_parent_id)).await?;
            let new_position = target_children.len() as i64;

            repo::index_reparent(conn, idx_id, new_parent_id, new_position).await?;

            if let Some(oid) = old_parent_id {
                repo::index_reindex_positions(conn, Some(oid)).await?;
            }
            repo::index_reindex_positions(conn, Some(new_parent_id)).await?;
            new_parent_label = repo::index_get(conn, new_parent_id)
                .await
                .map(|n| n.title.unwrap_or_else(|| new_parent_id.to_string()))
                .unwrap_or_else(|_| new_parent_id.to_string());
        }

        let location = self.render_location(idx_id).await?;
        Ok(format!(
            "moved '{index_path}' under '{new_parent_label}'\n{location}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::service::test_support::setup;
    use crate::storage::types::TargetType;

    #[tokio::test]
    async fn test_move_index() {
        let svc = setup().await;
        let root = svc.find_root().await.unwrap();

        let group_a = svc
            .create_index(root.id, Some("GroupA".into()), None, None)
            .await
            .unwrap();
        let group_b = svc
            .create_index(root.id, Some("GroupB".into()), None, None)
            .await
            .unwrap();
        let item = svc
            .create_index(
                group_a.id,
                Some("Item".into()),
                None,
                Some(TargetType::Group),
            )
            .await
            .unwrap();

        // Move Item from GroupA to GroupB.
        svc.move_index("/GroupA/Item", "/GroupB").await.unwrap();

        let b_children = svc.get_children(Some(group_b.id)).await.unwrap();
        assert_eq!(b_children.len(), 1);
        assert_eq!(b_children[0].id, item.id);

        let a_children = svc.get_children(Some(group_a.id)).await.unwrap();
        assert!(a_children.is_empty());
    }
}
