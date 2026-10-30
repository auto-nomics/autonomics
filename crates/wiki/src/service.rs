//! Wiki 服务:tokio worker 模式(与 evolution-core 的 worker/handle
//! 同款)。
//!
//! [`start`] 起一个 worker 任务串行消费 [`WikiCommand`];调用方持
//! [`WikiHandle`](可 Clone)发送命令并等待 oneshot 回执。最后一个句柄
//! drop 后控制通道关闭,worker 处理完当前命令即退出。存储访问由此全部
//! 经单一循环串行化 —— `dyn WikiStorage` 与具体后端走同一条路径。

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result};
use crate::storage::types::{
    EntryVersionSummary, KnowledgeEntryData, RecycleItem, SearchFilter, SearchHit, WikiStorage,
};

/// worker 从控制通道消费的命令。每条命令携带 oneshot 回执;调用方
/// drop 回执即变为 fire-and-forget。
pub enum WikiCommand {
    CreateEntry {
        entry: KnowledgeEntryData,
        reply: oneshot::Sender<Result<()>>,
    },
    DeleteEntry {
        id: uuid::Uuid,
        reply: oneshot::Sender<Result<()>>,
    },
    RestoreEntry {
        id: uuid::Uuid,
        reply: oneshot::Sender<Result<()>>,
    },
    PurgeEntry {
        id: uuid::Uuid,
        reply: oneshot::Sender<Result<()>>,
    },
    ListDeleted {
        reply: oneshot::Sender<Result<Vec<RecycleItem>>>,
    },
    GetEntry {
        id: uuid::Uuid,
        reply: oneshot::Sender<Result<KnowledgeEntryData>>,
    },
    FindByTitle {
        title: String,
        reply: oneshot::Sender<Result<Option<KnowledgeEntryData>>>,
    },
    GetEntryBatch {
        ids: Vec<uuid::Uuid>,
        reply: oneshot::Sender<Result<Vec<KnowledgeEntryData>>>,
    },
    UpdateEntry {
        entry: KnowledgeEntryData,
        reply: oneshot::Sender<Result<()>>,
    },
    UpdateEntryBatch {
        entries: Vec<KnowledgeEntryData>,
        reply: oneshot::Sender<Result<()>>,
    },
    ListVersions {
        id: uuid::Uuid,
        reply: oneshot::Sender<Result<Vec<EntryVersionSummary>>>,
    },
    GetVersion {
        id: uuid::Uuid,
        version: i64,
        reply: oneshot::Sender<Result<KnowledgeEntryData>>,
    },
    GetInLinksBatch {
        ids: Vec<String>,
        reply: oneshot::Sender<Result<Vec<Vec<String>>>>,
    },
    GetOutLinksBatch {
        ids: Vec<String>,
        reply: oneshot::Sender<Result<Vec<Vec<String>>>>,
    },
    GetMentions {
        id: String,
        reply: oneshot::Sender<Result<Vec<String>>>,
    },
    /// BM25 全文检索(中英文混合分词),按相关性降序。
    Search {
        query: String,
        limit: usize,
        reply: oneshot::Sender<Result<Vec<SearchHit>>>,
    },
    /// BM25 检索 + 过滤器(标签 AND / mtime 时间窗)。
    SearchFiltered {
        query: String,
        limit: usize,
        filter: SearchFilter,
        reply: oneshot::Sender<Result<Vec<SearchHit>>>,
    },
}

/// 启动 wiki worker 并返回其控制句柄。须在 tokio runtime 上下文调用
/// (内部 `tokio::spawn`)。容量 128 与 evolution worker 一致:命令突发
/// 时进缓冲,不把调用方顶出局。
pub fn start<T: WikiStorage + ?Sized + 'static>(store: Arc<T>) -> WikiHandle {
    let (tx, rx) = mpsc::channel::<WikiCommand>(128);
    let worker = WikiWorker::new(store, rx);
    tokio::spawn(worker.run());
    WikiHandle { tx }
}

/// wiki worker:持有存储与命令接收端,逐条串行处理。
pub struct WikiWorker<T: WikiStorage + ?Sized> {
    store: Arc<T>,
    rx: mpsc::Receiver<WikiCommand>,
}

impl<T: WikiStorage + ?Sized> WikiWorker<T> {
    pub fn new(store: Arc<T>, rx: mpsc::Receiver<WikiCommand>) -> Self {
        Self { store, rx }
    }

    /// 控制循环:一次一条命令,存储访问严格串行。最后一个句柄 drop、
    /// 通道关闭后退出。回执 send 失败只说明接收端已 drop
    /// (fire-and-forget),无需上报。
    async fn run(mut self) {
        while let Some(command) = self.rx.recv().await {
            match command {
                WikiCommand::CreateEntry { entry, reply } => {
                    let _ = reply.send(self.store.create_entry(entry).await.map_err(Error::from));
                }
                WikiCommand::DeleteEntry { id, reply } => {
                    let _ = reply.send(self.store.delete_entry(id).await.map_err(Error::from));
                }
                WikiCommand::RestoreEntry { id, reply } => {
                    let _ = reply.send(self.store.restore_entry(id).await.map_err(Error::from));
                }
                WikiCommand::PurgeEntry { id, reply } => {
                    let _ = reply.send(self.store.purge_entry(id).await.map_err(Error::from));
                }
                WikiCommand::ListDeleted { reply } => {
                    let _ = reply.send(self.store.list_deleted().await.map_err(Error::from));
                }
                WikiCommand::GetEntry { id, reply } => {
                    let _ = reply.send(self.store.get_entry(id).await.map_err(Error::from));
                }
                WikiCommand::FindByTitle { title, reply } => {
                    let _ = reply.send(self.store.find_by_title(&title).await.map_err(Error::from));
                }
                WikiCommand::GetEntryBatch { ids, reply } => {
                    let _ = reply.send(self.store.get_entry_batch(ids).await.map_err(Error::from));
                }
                WikiCommand::UpdateEntry { entry, reply } => {
                    let _ = reply.send(self.store.update_entry(entry).await.map_err(Error::from));
                }
                WikiCommand::UpdateEntryBatch { entries, reply } => {
                    let _ = reply.send(
                        self.store
                            .update_entry_batch(&entries)
                            .await
                            .map_err(Error::from),
                    );
                }
                WikiCommand::ListVersions { id, reply } => {
                    let _ = reply.send(self.store.list_versions(id).await.map_err(Error::from));
                }
                WikiCommand::GetVersion { id, version, reply } => {
                    let _ = reply.send(
                        self.store
                            .get_version(id, version)
                            .await
                            .map_err(Error::from),
                    );
                }
                WikiCommand::GetInLinksBatch { ids, reply } => {
                    let _ = reply.send(
                        self.store
                            .get_in_links_batch(&ids)
                            .await
                            .map_err(Error::from),
                    );
                }
                WikiCommand::GetOutLinksBatch { ids, reply } => {
                    let _ = reply.send(
                        self.store
                            .get_out_links_batch(&ids)
                            .await
                            .map_err(Error::from),
                    );
                }
                WikiCommand::GetMentions { id, reply } => {
                    let _ = reply.send(self.store.get_mentions(&id).await.map_err(Error::from));
                }
                WikiCommand::Search {
                    query,
                    limit,
                    reply,
                } => {
                    let _ = reply.send(self.store.search(&query, limit).await.map_err(Error::from));
                }
                WikiCommand::SearchFiltered {
                    query,
                    limit,
                    filter,
                    reply,
                } => {
                    let _ = reply.send(
                        self.store
                            .search_with_filter(&query, limit, &filter)
                            .await
                            .map_err(Error::from),
                    );
                }
            }
        }
        tracing::debug!("wiki worker exiting: control channel closed");
    }
}

/// wiki worker 的控制句柄。Clone 共享同一 worker;最后一个句柄 drop
/// 后通道关闭,worker 在处理完当前命令后退出。
#[derive(Clone)]
pub struct WikiHandle {
    tx: mpsc::Sender<WikiCommand>,
}

impl WikiHandle {
    /// 发送命令并等待回执。发送失败(通道已关)与回执丢失(worker 未能
    /// 回复)都归为 [`Error::WorkerGone`]。
    async fn call<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T>>) -> WikiCommand,
    ) -> Result<T> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(make(reply))
            .await
            .map_err(|e| Error::WorkerGone(e.to_string()))?;
        rx.await.map_err(|e| Error::WorkerGone(e.to_string()))?
    }

    pub async fn create_entry(&self, entry: KnowledgeEntryData) -> Result<()> {
        self.call(|reply| WikiCommand::CreateEntry { entry, reply })
            .await
    }

    /// 软删除进回收站;`restore_entry` 恢复,`purge_entry` 净删。
    pub async fn delete_entry(&self, id: uuid::Uuid) -> Result<()> {
        self.call(|reply| WikiCommand::DeleteEntry { id, reply })
            .await
    }

    pub async fn restore_entry(&self, id: uuid::Uuid) -> Result<()> {
        self.call(|reply| WikiCommand::RestoreEntry { id, reply })
            .await
    }

    pub async fn purge_entry(&self, id: uuid::Uuid) -> Result<()> {
        self.call(|reply| WikiCommand::PurgeEntry { id, reply })
            .await
    }

    /// 回收站清单(按删除时间升序)。
    pub async fn list_deleted(&self) -> Result<Vec<RecycleItem>> {
        self.call(|reply| WikiCommand::ListDeleted { reply }).await
    }

    pub async fn get_entry(&self, id: uuid::Uuid) -> Result<KnowledgeEntryData> {
        self.call(|reply| WikiCommand::GetEntry { id, reply }).await
    }

    /// 按标题精确查找;不存在返回 `None`。
    pub async fn find_by_title(&self, title: &str) -> Result<Option<KnowledgeEntryData>> {
        let title = title.to_string();
        self.call(|reply| WikiCommand::FindByTitle { title, reply })
            .await
    }

    pub async fn get_entry_batch(&self, ids: Vec<uuid::Uuid>) -> Result<Vec<KnowledgeEntryData>> {
        self.call(|reply| WikiCommand::GetEntryBatch { ids, reply })
            .await
    }

    pub async fn update_entry(&self, entry: KnowledgeEntryData) -> Result<()> {
        self.call(|reply| WikiCommand::UpdateEntry { entry, reply })
            .await
    }

    pub async fn update_entry_batch(&self, entries: Vec<KnowledgeEntryData>) -> Result<()> {
        self.call(|reply| WikiCommand::UpdateEntryBatch { entries, reply })
            .await
    }

    /// 版本历史(新→旧,仅元信息)。
    pub async fn list_versions(&self, id: uuid::Uuid) -> Result<Vec<EntryVersionSummary>> {
        self.call(|reply| WikiCommand::ListVersions { id, reply })
            .await
    }

    pub async fn get_version(&self, id: uuid::Uuid, version: i64) -> Result<KnowledgeEntryData> {
        self.call(|reply| WikiCommand::GetVersion { id, version, reply })
            .await
    }

    /// 回滚到历史版本:取快照、mtime 提到现在、走普通更新 —— 追加新
    /// 版本,历史不丢。
    pub async fn restore_version(&self, id: uuid::Uuid, version: i64) -> Result<()> {
        let mut snapshot = self.get_version(id, version).await?;
        snapshot.mtime = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(snapshot.mtime);
        self.update_entry(snapshot).await
    }

    pub async fn get_in_links_batch(&self, ids: &[String]) -> Result<Vec<Vec<String>>> {
        let ids = ids.to_vec();
        self.call(|reply| WikiCommand::GetInLinksBatch { ids, reply })
            .await
    }

    pub async fn get_out_links_batch(&self, ids: &[String]) -> Result<Vec<Vec<String>>> {
        let ids = ids.to_vec();
        self.call(|reply| WikiCommand::GetOutLinksBatch { ids, reply })
            .await
    }

    /// 提及:正文纯文本含该词条标题/别名、且尚未链接的候选(补链来源)。
    pub async fn get_mentions(&self, id: &str) -> Result<Vec<String>> {
        let id = id.to_string();
        self.call(|reply| WikiCommand::GetMentions { id, reply })
            .await
    }

    /// BM25 全文检索;空查询或 `limit == 0` 返回空。
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let query = query.to_string();
        self.call(|reply| WikiCommand::Search {
            query,
            limit,
            reply,
        })
        .await
    }

    /// BM25 检索 + 过滤器(标签 AND / mtime 时间窗)。
    pub async fn search_filtered(
        &self,
        query: &str,
        limit: usize,
        filter: &SearchFilter,
    ) -> Result<Vec<SearchHit>> {
        let query = query.to_string();
        let filter = filter.clone();
        self.call(|reply| WikiCommand::SearchFiltered {
            query,
            limit,
            filter,
            reply,
        })
        .await
    }

    /// 检索并回填正文:返回 (词条, 得分) 按相关性降序。检索与取正文是
    /// 两条相邻命令,worker 串行保证本进程一致;两步之间词条若被其他
    /// 进程净删,该命中被跳过。
    pub async fn search_entries(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(KnowledgeEntryData, f64)>> {
        let hits = self.search(query, limit).await?;
        let ids: Vec<uuid::Uuid> = hits
            .iter()
            .filter_map(|hit| uuid::Uuid::parse_str(&hit.entry_id).ok())
            .collect();
        let entries = self.get_entry_batch(ids).await?;
        let by_id: HashMap<String, KnowledgeEntryData> = entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect();
        Ok(hits
            .into_iter()
            .filter_map(|hit| by_id.get(&hit.entry_id).map(|e| (e.clone(), hit.score)))
            .collect())
    }
}
