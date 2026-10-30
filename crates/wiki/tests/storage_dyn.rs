//! 验证 `dyn WikiStorage`（supertrait 组合）既能通过 trait object 使用，
//! 也能通过 blanket impl 自动满足约束。

use std::sync::Arc;

use async_trait::async_trait;
use wiki::service::start;
use wiki::storage::error::Result;
use wiki::storage::types::{
    KnowledgeEntryData, SearchHit, WikiEntryStorage, WikiIndexStorage, WikiSearchStorage,
    WikiStorage,
};

/// 只实现两个基础 trait，不显式写 `impl WikiStorage for _`。
struct FakeStorage {
    entries: std::sync::Mutex<Vec<KnowledgeEntryData>>,
}

impl FakeStorage {
    fn new() -> Self {
        Self {
            entries: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl WikiEntryStorage for FakeStorage {
    async fn create_entry(&self, entry: KnowledgeEntryData) -> Result<()> {
        self.entries.lock().unwrap().push(entry);
        Ok(())
    }

    async fn delete_entry(&self, _id: uuid::Uuid) -> Result<()> {
        Ok(())
    }

    async fn get_entry(&self, _id: uuid::Uuid) -> Result<KnowledgeEntryData> {
        let entries = self.entries.lock().unwrap();
        entries.first().cloned().ok_or_else(|| "empty".into())
    }

    async fn get_entry_batch(&self, _ids: Vec<uuid::Uuid>) -> Result<Vec<KnowledgeEntryData>> {
        let entries = self.entries.lock().unwrap();
        Ok(entries.clone())
    }

    async fn update_entry(&self, _entry: KnowledgeEntryData) -> Result<()> {
        Ok(())
    }

    async fn update_entry_batch(&self, _entries: &[KnowledgeEntryData]) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl WikiIndexStorage for FakeStorage {
    async fn get_in_links_batch(&self, ids: &[String]) -> Result<Vec<Vec<String>>> {
        Ok(ids.iter().map(|_| vec!["in".to_string()]).collect())
    }

    async fn get_out_links_batch(&self, ids: &[String]) -> Result<Vec<Vec<String>>> {
        Ok(ids.iter().map(|_| vec!["out".to_string()]).collect())
    }
}

#[async_trait]
impl WikiSearchStorage for FakeStorage {
    /// 朴素标题子串匹配:只验证 trait 组合与 worker 链路,真实 BM25
    /// 打分在 TursoWikiStorage(见 tests/search.rs)。
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.title.contains(query))
            .take(limit)
            .map(|entry| SearchHit {
                entry_id: entry.id.clone(),
                score: 1.0,
            })
            .collect())
    }
}

fn sample_entry() -> KnowledgeEntryData {
    KnowledgeEntryData {
        id: "e1".to_string(),
        title: "title".to_string(),
        alias: vec![],
        content: "body".to_string(),
        ctime: 0,
        mtime: 0,
    }
}

#[tokio::test]
async fn dyn_wiki_storage_supports_both_traits() {
    // blanket impl 让 FakeStorage 自动满足 WikiStorage
    let store: Arc<dyn WikiStorage> = Arc::new(FakeStorage::new());

    store.create_entry(sample_entry()).await.unwrap();
    let got = store.get_entry(uuid::Uuid::nil()).await.unwrap();
    assert_eq!(got.title, "title");

    let one = ["e1".to_string()];
    let in_links = store.get_in_links_batch(&one).await.unwrap();
    assert_eq!(in_links, vec![vec!["in".to_string()]]);
    let out_links = store.get_out_links_batch(&one).await.unwrap();
    assert_eq!(out_links, vec![vec!["out".to_string()]]);
}

#[tokio::test]
async fn worker_accepts_dyn_and_concrete_backends() {
    // 运行时可替换：`Arc<dyn WikiStorage>` 进 worker
    let dyn_storage: Arc<dyn WikiStorage> = Arc::new(FakeStorage::new());
    let dyn_handle = start(dyn_storage);
    dyn_handle.create_entry(sample_entry()).await.unwrap();
    let got = dyn_handle.get_entry(uuid::Uuid::nil()).await.unwrap();
    assert_eq!(got.title, "title");

    // 静态分发：具体后端类型进 worker
    let concrete = start(Arc::new(FakeStorage::new()));
    concrete.create_entry(sample_entry()).await.unwrap();
}
