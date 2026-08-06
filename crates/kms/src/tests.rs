//! Integration tests for the KMS crate (Turso-backed).
//!
//! These tests exercise the full KmsService API against an in-memory Turso
//! database, covering entity CRUD, knowledge CRUD, index tree operations,
//! diagnostics, and the local-view API. Ported and adapted from dendrite's
//! test suite.

#![cfg(test)]

use uuid::Uuid;

use crate::*;
use crate::language::Language;
use crate::storage::types::{KnowledgeType, Nomenclature, TargetType};

fn make_name(full: &str) -> Nomenclature {
    Nomenclature {
        id: Uuid::new_v4(),
        lang: Language::ZH,
        full: full.to_string(),
        abbr: None,
    }
}

async fn setup() -> KmsService {
    let storage = Storage::open_in_memory().await.unwrap();
    KmsService::from_storage(storage).await.unwrap()
}

// ── Entity tests ──

#[tokio::test]
async fn test_create_and_get_entity() {
    let svc = setup().await;
    let (entity, existed) = svc
        .create_entity(vec![make_name("Python")], "A programming language")
        .await
        .unwrap();
    assert!(!existed);
    assert_eq!(entity.definition, "A programming language");
    assert_eq!(entity.name.len(), 1);

    let got = svc.get_entity(entity.id).await.unwrap();
    assert_eq!(got.id, entity.id);
    assert_eq!(got.definition, "A programming language");
}

#[tokio::test]
async fn test_entity_dedup_on_exact_name() {
    let svc = setup().await;
    let (e1, existed1) = svc
        .create_entity(vec![make_name("Rust")], "Systems language")
        .await
        .unwrap();
    assert!(!existed1);

    let (e2, existed2) = svc
        .create_entity(vec![make_name("Rust")], "Different definition")
        .await
        .unwrap();
    assert!(existed2);
    assert_eq!(e1.id, e2.id);
}

#[tokio::test]
async fn test_update_entity() {
    let svc = setup().await;
    let (entity, _) = svc
        .create_entity(vec![make_name("Go")], "original")
        .await
        .unwrap();

    let updated = svc
        .update_entity_by_id(entity.id, Some("updated definition"), None)
        .await
        .unwrap();
    assert_eq!(updated.definition, "updated definition");
}

#[tokio::test]
async fn test_delete_entity() {
    let svc = setup().await;
    let (entity, _) = svc
        .create_entity(vec![make_name("Java")], "JVM language")
        .await
        .unwrap();

    svc.delete_entity(entity.id).await.unwrap();
    assert!(svc.get_entity(entity.id).await.is_err());
}

#[tokio::test]
async fn test_search_entity() {
    let svc = setup().await;
    svc.create_entity(vec![make_name("TypeScript")], "Typed JS")
        .await
        .unwrap();
    svc.create_entity(vec![make_name("JavaScript")], "Dynamic JS")
        .await
        .unwrap();

    let results = svc.search_entity("Type").await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name[0].full, "TypeScript");
}

// ── Knowledge tests ──

#[tokio::test]
async fn test_create_and_get_knowledge() {
    let svc = setup().await;
    let (entity, _) = svc
        .create_entity(vec![make_name("Docker")], "Container platform")
        .await
        .unwrap();

    let knowledge = svc
        .create_knowledge(
            "Docker · 安装指南",
            KnowledgeType::Aspect,
            vec![entity.id],
            Some("安装步骤...".into()),
        )
        .await
        .unwrap();

    let got = svc.get_knowledge(knowledge.id).await.unwrap();
    assert_eq!(got.title, "Docker · 安装指南");
    assert_eq!(got.knowledge_type, KnowledgeType::Aspect);
    assert_eq!(got.entities, vec![entity.id]);
}

#[tokio::test]
async fn test_delete_knowledge() {
    let svc = setup().await;
    svc.create_knowledge(
        "Test · 删除",
        KnowledgeType::Aspect,
        vec![],
        Some("content".into()),
    )
    .await
    .unwrap();

    svc.delete_knowledge("Test · 删除").await.unwrap();
    assert!(svc.resolve_knowledge("Test · 删除").await.is_err());
}

// ── Index tests ──

#[tokio::test]
async fn test_create_index_and_navigate() {
    let svc = setup().await;
    let root = svc.find_root().await.unwrap();

    let lang_group = svc
        .create_index(root.id, Some("编程语言".into()), None, None)
        .await
        .unwrap();

    let _python_group = svc
        .create_index(
            lang_group.id,
            Some("Python".into()),
            None,
            Some(TargetType::Group),
        )
        .await
        .unwrap();

    let result = svc.navigate("/编程语言/Python").await.unwrap();
    assert!(result.contains("Python"));
}

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
    assert!(err.unwrap_err().contains("duplicate child title"));
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

// ── Diagnostics tests ──

#[tokio::test]
async fn test_diagnostics_empty_definition() {
    let svc = setup().await;
    svc.create_entity(vec![make_name("EmptyEntity")], "")
        .await
        .unwrap();

    let diags = svc.diagnose().await.unwrap();
    let has_empty_def = diags
        .iter()
        .any(|d| d.code == "entity.empty_definition");
    assert!(has_empty_def);
}

#[tokio::test]
async fn test_diagnostics_clean() {
    let svc = setup().await;
    svc.create_entity(vec![make_name("Clean")], "A well-defined entity")
        .await
        .unwrap();

    let diags = svc.diagnose().await.unwrap();
    // Should have no entity-related issues.
    let entity_issues: Vec<_> = diags
        .iter()
        .filter(|d| d.code.starts_with("entity."))
        .collect();
    assert!(entity_issues.is_empty());
}

// ── Local view tests ──

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
            KnowledgeType::Aspect,
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

// ── Render tests ──

#[tokio::test]
async fn test_render_full_tree() {
    let svc = setup().await;
    let root = svc.find_root().await.unwrap();

    svc.create_index(root.id, Some("Languages".into()), None, None)
        .await
        .unwrap();
    svc.create_index(root.id, Some("Tools".into()), None, None)
        .await
        .unwrap();

    let tree = svc.render_full_tree().await.unwrap();
    assert!(tree.contains("Root"));
    assert!(tree.contains("Languages"));
    assert!(tree.contains("Tools"));
}

// ── Knowledge link/unlink tests ──

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
            KnowledgeType::Aspect,
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
    let orphaned_id = svc.detach_knowledge_index("Vue · 响应式原理").await.unwrap();
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
