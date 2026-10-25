use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::KnowledgePath;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Knowledge {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub knowledge_path: KnowledgePath,
    pub title: String,
    pub body_md: String,
    pub status: String,
    pub confidence: f64,
    pub source_hash: String,
    pub structural: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Knowledge {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Uuid,
        parent_id: Option<Uuid>,
        knowledge_path: KnowledgePath,
        title: String,
        body_md: String,
        status: String,
        confidence: f64,
        source_hash: String,
        structural: bool,
        created_at: i64,
        updated_at: i64,
    ) -> Self {
        Self {
            id,
            parent_id,
            knowledge_path,
            title,
            body_md,
            status,
            confidence,
            source_hash,
            structural,
            created_at,
            updated_at,
        }
    }

    #[must_use]
    pub fn leaf(
        knowledge_path: KnowledgePath,
        title: String,
        body_md: String,
        confidence: f64,
        source_hash: &str,
        now: i64,
    ) -> Self {
        Self::new(
            Uuid::new_v4(),
            None,
            knowledge_path,
            title,
            body_md,
            "active".to_string(),
            confidence,
            source_hash.to_string(),
            false,
            now,
            now,
        )
    }

    #[must_use]
    pub fn structural(knowledge_path: KnowledgePath, now: i64) -> Self {
        let title = title_from_segment(knowledge_path.leaf());
        Self::new(
            Uuid::new_v4(),
            None,
            knowledge_path,
            title,
            String::new(),
            "active".to_string(),
            1.0,
            String::new(),
            true,
            now,
            now,
        )
    }
}

fn title_from_segment(segment: &str) -> String {
    segment
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(capitalize_first)
        .collect::<Vec<_>>()
        .join(" ")
}

fn capitalize_first(part: &str) -> String {
    let mut chars = part.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_nodes_derive_display_titles() {
        let path = KnowledgePath::normalize("/engineering/software-testing").unwrap();
        let node = Knowledge::structural(path, 123);
        assert_eq!(node.title, "Software Testing");
        assert!(node.structural);
        assert_eq!(node.body_md, "");
    }
}
