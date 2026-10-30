//! 附件 blob 的抽象与内存实现。
//!
//! wiki 的附件 = sqlite 元数据行(见 [`crate::storage::types`])+ 本
//! trait 指向的二进制 blob。生产后端是 vfs 挂载存储(feature `vfs`,
//! 见 [`super::vfs_blob::VfsBlobStore`]);测试用 [`MemoryBlobStore`]。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::error::{Error, Result};

/// 附件 blob 后端:按 key 存取不可变二进制。key 由存储层生成
/// (`attachments/<entry_id>/<attachment_id>.<ext>`),实现方不做解释。
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// 写入(或覆盖)`key` 的内容。
    async fn put(&self, key: &str, data: Vec<u8>) -> Result<()>;
    /// 读取 `key`;不存在时返回 [`Error::NotFound`]。
    async fn get(&self, key: &str) -> Result<Vec<u8>>;
    /// 删除 `key`;key 不存在视为成功(幂等,配合元数据行的清理)。
    async fn delete(&self, key: &str) -> Result<()>;
}

/// 进程内 HashMap 后端(测试与无后端演示用)。
#[derive(Default)]
pub struct MemoryBlobStore {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryBlobStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 现存 key 快照(测试/诊断用,断言 blob 清理)。
    pub async fn keys(&self) -> Vec<String> {
        let blobs = self.blobs.lock().await;
        blobs.keys().cloned().collect()
    }
}

#[async_trait]
impl BlobStore for MemoryBlobStore {
    async fn put(&self, key: &str, data: Vec<u8>) -> Result<()> {
        self.blobs.lock().await.insert(key.to_string(), data);
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>> {
        self.blobs
            .lock()
            .await
            .get(key)
            .cloned()
            .ok_or_else(|| Error::NotFound(key.to_string()))
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.blobs.lock().await.remove(key);
        Ok(())
    }
}
