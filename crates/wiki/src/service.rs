//! Wiki 服务:tokio worker 模式(与 evolution-core 的 worker/handle
//! 同款)。
//!
//! [`start`] 起一个 worker 任务串行消费 [`WikiCommand`];调用方持
//! [`WikiHandle`](可 Clone)发送命令并等待 oneshot 回执。最后一个句柄
//! drop 后控制通道关闭,worker 处理完当前命令即退出。存储访问由此全部
//! 经单一循环串行化 —— `dyn WikiStorage` 与具体后端走同一条路径。

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result};
use crate::storage::types::{KnowledgeEntryData, SearchHit, WikiStorage};

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
    GetEntry {
        id: uuid::Uuid,
        reply: oneshot::Sender<Result<KnowledgeEntryData>>,
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
    GetInLinksBatch {
        ids: Vec<String>,
        reply: oneshot::Sender<Result<Vec<Vec<String>>>>,
    },
    GetOutLinksBatch {
        ids: Vec<String>,
        reply: oneshot::Sender<Result<Vec<Vec<String>>>>,
    },
    /// BM25 全文检索(中英文混合分词),按相关性降序。
    Search {
        query: String,
        limit: usize,
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
                WikiCommand::GetEntry { id, reply } => {
                    let _ = reply.send(self.store.get_entry(id).await.map_err(Error::from));
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
                WikiCommand::Search {
                    query,
                    limit,
                    reply,
                } => {
                    let _ = reply.send(self.store.search(&query, limit).await.map_err(Error::from));
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

    pub async fn delete_entry(&self, id: uuid::Uuid) -> Result<()> {
        self.call(|reply| WikiCommand::DeleteEntry { id, reply })
            .await
    }

    pub async fn get_entry(&self, id: uuid::Uuid) -> Result<KnowledgeEntryData> {
        self.call(|reply| WikiCommand::GetEntry { id, reply }).await
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
}
