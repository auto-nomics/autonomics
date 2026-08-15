//! Smoke test: verify all 27 kms_* tools register and a representative
//! subset execute correctly against an in-memory KMS.

use std::sync::Arc;

use agentik_core::tools::ToolRegistration;
use agentik_types::tools::ToolResultContent;
use kms::KmsService;
use kms_tools::{kms_readonly_registrations, kms_write_registrations};
use serde_json::json;

async fn setup() -> (Arc<KmsService>, Vec<ToolRegistration>) {
    let storage = kms::Storage::open_in_memory().await.unwrap();
    let svc = Arc::new(KmsService::from_storage(storage).await.unwrap());
    let mut tools = kms_readonly_registrations(svc.clone());
    tools.extend(kms_write_registrations(svc.clone()));
    (svc, tools)
}

fn find_tool<'a>(tools: &'a [ToolRegistration], name: &str) -> &'a ToolRegistration {
    tools
        .iter()
        .find(|t| t.definition.name == name)
        .unwrap_or_else(|| {
            panic!("tool '{name}' not found in registrations");
        })
}

#[tokio::test]
async fn test_tool_count() {
    let (_svc, tools) = setup().await;
    // 10 read tools + 17 write tools.
    assert_eq!(tools.len(), 27, "expected 10 read + 17 write tools");
}

#[tokio::test]
async fn test_readonly_subset() {
    let storage = kms::Storage::open_in_memory().await.unwrap();
    let svc = Arc::new(KmsService::from_storage(storage).await.unwrap());
    let ro = kms_readonly_registrations(svc.clone());
    assert_eq!(ro.len(), 10, "expected 10 readonly tools");
    let mut ro_names: Vec<_> = ro.iter().map(|t| t.definition.name.as_str()).collect();
    ro_names.sort_unstable();
    assert_eq!(
        ro_names,
        vec![
            "kms_get_entity",
            "kms_get_entity_knowledge",
            "kms_get_knowledge",
            "kms_get_knowledge_batch",
            "kms_list_entities",
            "kms_local",
            "kms_search_content",
            "kms_search_entity",
            "kms_search_subtree",
            "kms_subtree_knowledge",
        ]
    );

    let writes = kms_write_registrations(svc);
    assert_eq!(writes.len(), 17, "expected 17 write tools");
    let mut write_names: Vec<_> = writes.iter().map(|t| t.definition.name.as_str()).collect();
    write_names.sort_unstable();
    assert_eq!(
        write_names,
        vec![
            "kms_add_nomenclature",
            "kms_create_entity",
            "kms_create_index",
            "kms_create_knowledge",
            "kms_delete_entity",
            "kms_delete_index",
            "kms_delete_knowledge",
            "kms_delete_nomenclature",
            "kms_detach_knowledge",
            "kms_link_orphans",
            "kms_merge_subtree",
            "kms_move_children",
            "kms_move_index",
            "kms_rename_knowledge",
            "kms_update_entity",
            "kms_update_knowledge",
            "kms_update_nomenclature",
        ]
    );
    assert!(
        ro.iter().all(|t| !writes
            .iter()
            .any(|w| w.definition.name == t.definition.name)),
        "read and write tool sets must not overlap"
    );

    for tool in ro {
        assert!(
            !tool.definition.description.contains("Create")
                && !tool.definition.description.contains("Delete")
                && !tool.definition.description.contains("Update"),
            "read-only tool description suggests mutation: {}",
            tool.definition.description
        );
    }
}

#[tokio::test]
async fn test_create_entity_via_tool() {
    let (_svc, tools) = setup().await;
    let tool = find_tool(&tools, "kms_create_entity");

    let result = tool
        .implementation
        .execute(json!({
            "names": [{"lang": "ZH", "full": "Python", "abbr": "py"}],
            "definition": "A programming language"
        }))
        .await
        .unwrap();

    match result.content {
        ToolResultContent::Json(v) => {
            assert_eq!(v["name"], "Python");
            assert_eq!(v["existed"], false);
        }
        other => panic!("expected JSON result, got {other:?}"),
    }
}

#[tokio::test]
async fn test_create_and_get_knowledge_via_tool() {
    let (_svc, tools) = setup().await;

    // First create an entity.
    let entity_tool = find_tool(&tools, "kms_create_entity");
    entity_tool
        .implementation
        .execute(json!({
            "names": [{"lang": "ZH", "full": "Rust"}],
            "definition": "Systems programming language"
        }))
        .await
        .unwrap();

    // Create knowledge.
    let k_tool = find_tool(&tools, "kms_create_knowledge");
    let result = k_tool
        .implementation
        .execute(json!({
            "title": "Rust · 内存安全",
            "knowledge_type": "aspect",
            "entities": ["Rust"],
            "content": "Rust achieves memory safety via [[Ownership]] and [[Borrow checker]]."
        }))
        .await
        .unwrap();

    match result.content {
        ToolResultContent::Json(v) => {
            assert_eq!(v["title"], "Rust · 内存安全");
        }
        other => panic!("expected JSON, got {other:?}"),
    }

    // Get knowledge.
    let get_tool = find_tool(&tools, "kms_get_knowledge");
    let result = get_tool
        .implementation
        .execute(json!({ "title": "Rust · 内存安全" }))
        .await
        .unwrap();

    match result.content {
        ToolResultContent::Json(v) => {
            assert_eq!(v["title"], "Rust · 内存安全");
            assert!(v["content"].as_str().unwrap().contains("Ownership"));
        }
        other => panic!("expected JSON, got {other:?}"),
    }
}

#[tokio::test]
async fn test_create_index_and_local() {
    let (_svc, tools) = setup().await;

    // Create index.
    let idx_tool = find_tool(&tools, "kms_create_index");
    idx_tool
        .implementation
        .execute(json!({
            "parent_ref": "/",
            "title": "编程语言"
        }))
        .await
        .unwrap();

    // Use kms_local to inspect.
    let local_tool = find_tool(&tools, "kms_local");
    let result = local_tool
        .implementation
        .execute(json!({ "path": "/" }))
        .await
        .unwrap();

    match result.content {
        ToolResultContent::Json(v) => {
            let children = v["children"].as_array().unwrap();
            assert_eq!(children.len(), 1);
            assert_eq!(children[0]["title"], "编程语言");
            assert_eq!(children[0]["type"], "group");
        }
        other => panic!("expected JSON, got {other:?}"),
    }
}

#[tokio::test]
async fn test_vague_title_rejected() {
    let (_svc, tools) = setup().await;
    let entity_tool = find_tool(&tools, "kms_create_entity");
    entity_tool
        .implementation
        .execute(json!({
            "names": [{"lang": "ZH", "full": "Docker"}],
            "definition": "Container platform"
        }))
        .await
        .unwrap();

    let k_tool = find_tool(&tools, "kms_create_knowledge");
    let result = k_tool
        .implementation
        .execute(json!({
            "title": "Docker · 概述",
            "knowledge_type": "aspect",
            "entities": ["Docker"],
            "content": "Some content"
        }))
        .await;

    assert!(result.is_err(), "vague title should be rejected");
}

#[tokio::test]
async fn test_all_tool_names_start_with_kms() {
    let (_svc, tools) = setup().await;
    for tool in &tools {
        assert!(
            tool.definition.name.starts_with("kms_"),
            "tool '{}' should start with kms_",
            tool.definition.name
        );
    }
}
