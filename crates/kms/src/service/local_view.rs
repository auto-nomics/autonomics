//! Stateless local-view API: node snapshots, subtree queries, and search.

use uuid::Uuid;

use crate::error::{Error, Result};
use crate::storage::repo;
use crate::storage::types::{Index, Knowledge, TargetType};
use crate::view::{IndexView, LocalView, SUBTREE_TITLES_LIMIT, SubtreeSummary};

use super::KmsService;
use super::knowledge::KnowledgeContentHit;

// ── Local-view (stateless) API ──
impl KmsService {
    pub async fn get_local_view(&self, node_id: Uuid) -> Result<LocalView> {
        let conn = &*self.storage.conn().await;

        // 1) ancestor path
        let path_rows = repo::index_ancestor_path_rows(conn, node_id).await?;

        let path: Vec<Index> = path_rows
            .iter()
            .map(|r| {
                Ok::<Index, Error>(Index {
                    id: Uuid::parse_str(&r.id)?,
                    title: r.title.clone(),
                    target: r.target.as_deref().and_then(|t| Uuid::parse_str(t).ok()),
                    target_type: TargetType::from_str(r.target_type.as_deref().unwrap_or("group")),
                    parent_id: r.parent_id.as_deref().and_then(|p| Uuid::parse_str(p).ok()),
                    position: r.position,
                })
            })
            .collect::<std::result::Result<Vec<_>, Error>>()?;

        let node = path
            .first()
            .cloned()
            .ok_or_else(|| format!("node {node_id} not found"))?;

        // 2) direct children
        let child_rows = repo::index_child_rows(conn, node_id).await?;
        let mut children: Vec<IndexView> = Vec::with_capacity(child_rows.len());
        for r in child_rows {
            let id = Uuid::parse_str(&r.id)?;
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
        let stats = repo::index_subtree_stats(conn, node_id, SUBTREE_TITLES_LIMIT).await?;

        // 4) sibling count
        let sibling_count = repo::index_sibling_count(conn, node_id).await?;

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

    pub async fn get_local_view_by_path(&self, path: &str) -> Result<LocalView> {
        let target_id = self.resolve_path_id(path).await?;
        self.get_local_view(target_id).await
    }

    pub async fn get_subtree_knowledge(&self, node_id: Uuid) -> Result<Vec<Knowledge>> {
        let ids = repo::index_subtree_knowledge_ids(&*self.storage.conn().await, node_id).await?;
        let mut out = Vec::with_capacity(ids.len());
        for kid in ids {
            match self.get_knowledge(kid).await {
                Ok(k) => out.push(k),
                Err(_) => continue,
            }
        }
        Ok(out)
    }

    pub async fn get_subtree_knowledge_by_path(&self, path: &str) -> Result<Vec<Knowledge>> {
        let target_id = self.resolve_path_id(path).await?;
        self.get_subtree_knowledge(target_id).await
    }

    pub async fn search_knowledge_titles(
        &self,
        node_id: Uuid,
        keyword: &str,
    ) -> Result<Vec<Knowledge>> {
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
    ) -> Result<Vec<KnowledgeContentHit>> {
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

#[cfg(test)]
mod tests {
    use crate::service::test_support::{make_name, setup};
    use crate::storage::types::TargetType;

    #[tokio::test]
    async fn test_local_view_root() {
        let svc = setup().await;
        let root = svc.find_root().await.unwrap();

        let view = svc.get_local_view(root.id).await.unwrap();
        assert_eq!(view.node.id, root.id);
        assert!(view.path.len() == 1); // just the root itself
    }

    #[tokio::test]
    async fn test_local_view_by_path() {
        let svc = setup().await;
        let root = svc.find_root().await.unwrap();
        svc.create_index(root.id, Some("TopicA".into()), None, None)
            .await
            .unwrap();

        let view = svc.get_local_view_by_path("/TopicA").await.unwrap();
        assert_eq!(view.node.title.as_deref(), Some("TopicA"));
    }

    #[tokio::test]
    async fn test_subtree_knowledge() {
        let svc = setup().await;
        let root = svc.find_root().await.unwrap();
        let (entity, _) = svc
            .create_entity(vec![make_name("React")], "UI library")
            .await
            .unwrap();

        let knowledge = svc
            .create_knowledge(
                "React · 核心概念",
                vec![entity.id],
                Some("Virtual DOM...".into()),
            )
            .await
            .unwrap();

        let frontend_group = svc
            .create_index(root.id, Some("Frontend".into()), None, None)
            .await
            .unwrap();
        svc.create_index(
            frontend_group.id,
            Some("React · 核心概念".into()),
            Some(knowledge.id),
            Some(TargetType::Knowledge),
        )
        .await
        .unwrap();

        let subtree = svc.get_subtree_knowledge(frontend_group.id).await.unwrap();
        assert_eq!(subtree.len(), 1);
        assert_eq!(subtree[0].title, "React · 核心概念");
    }
}
