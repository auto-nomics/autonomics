//! Wiki worker 端到端:handle → 命令通道 → worker → TursoWikiStorage。

use std::sync::Arc;

use wiki::service::start;
use wiki::storage::turso_wiki_storage::TursoWikiStorage;
use wiki::storage::types::{KnowledgeEntryData, WikiEntryStorage};

fn entry(id: uuid::Uuid, title: &str) -> KnowledgeEntryData {
    KnowledgeEntryData {
        id: id.to_string(),
        title: title.to_string(),
        alias: vec![],
        tags: vec![],
        content: format!("# {title}"),
        ctime: 1_000,
        mtime: 1_000,
    }
}

#[tokio::test]
async fn handle_roundtrips_through_worker() {
    let storage = Arc::new(TursoWikiStorage::open_in_memory().await.unwrap());
    let handle = start(storage);

    let id = uuid::Uuid::new_v4();
    handle.create_entry(entry(id, "Home")).await.unwrap();

    let got = handle.get_entry(id).await.unwrap();
    assert_eq!(got.title, "Home");

    // 索引查询走同一循环。
    let out = handle.get_out_links_batch(&[id.to_string()]).await.unwrap();
    assert_eq!(out, vec![Vec::<String>::new()]);

    handle.update_entry(entry(id, "Home-v2")).await.unwrap();
    let got = handle.get_entry(id).await.unwrap();
    assert_eq!(got.title, "Home-v2");

    // 存储层错误经回执原样透传到调用方。
    handle.delete_entry(id).await.unwrap();
    assert!(matches!(
        handle.get_entry(id).await,
        Err(wiki::error::Error::Storage(
            wiki::storage::error::Error::NotFound(_)
        ))
    ));
}

#[tokio::test]
async fn cloned_handles_share_one_worker() {
    let storage = Arc::new(TursoWikiStorage::open_in_memory().await.unwrap());
    let handle = start(storage);
    let other = handle.clone();

    let id = uuid::Uuid::new_v4();
    handle.create_entry(entry(id, "A")).await.unwrap();
    let got = other.get_entry(id).await.unwrap();
    assert_eq!(got.title, "A");
}

#[tokio::test]
async fn search_entries_backfills_body_with_score() {
    let storage = Arc::new(TursoWikiStorage::open_in_memory().await.unwrap());
    let target = uuid::Uuid::new_v4();
    let filler = uuid::Uuid::new_v4();
    {
        let s = storage.clone();
        s.create_entry(KnowledgeEntryData {
            id: target.to_string(),
            title: "队列研究".to_string(),
            alias: vec![],
            tags: vec!["epi".to_string()],
            content: "cohort study 设计与偏倚控制".to_string(),
            ctime: 1_000,
            mtime: 1_000,
        })
        .await
        .unwrap();
        s.create_entry(KnowledgeEntryData {
            id: filler.to_string(),
            title: "无关词条".to_string(),
            alias: vec![],
            tags: vec![],
            content: "无关内容".to_string(),
            ctime: 1_000,
            mtime: 1_000,
        })
        .await
        .unwrap();
    }

    let handle = start(storage);
    let results = handle.search_entries("cohort 队列", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    let (entry, score) = &results[0];
    assert_eq!(entry.id, target.to_string());
    assert_eq!(entry.tags, vec!["epi".to_string()]);
    assert!(score > &0.0);

    // 过滤检索与版本回滚也走 worker。
    let hits = handle
        .search_filtered(
            "cohort",
            10,
            &wiki::storage::types::SearchFilter {
                tags: vec!["epi".to_string()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    let versions = handle.list_versions(target).await.unwrap();
    assert_eq!(versions.len(), 1);
}
