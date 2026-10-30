//! Wiki worker 端到端:handle → 命令通道 → worker → TursoWikiStorage。

use std::sync::Arc;

use wiki::service::start;
use wiki::storage::turso_wiki_storage::TursoWikiStorage;
use wiki::storage::types::KnowledgeEntryData;

fn entry(id: uuid::Uuid, title: &str) -> KnowledgeEntryData {
    KnowledgeEntryData {
        id: id.to_string(),
        title: title.to_string(),
        alias: vec![],
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
