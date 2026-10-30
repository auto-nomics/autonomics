//! 附件 blob 的 vfs 后端(feature `vfs`)。
//!
//! 把 [`crate::storage::blob::BlobStore`] 落到 [`vfs::OpendalFileStorage`]:
//! 写走 `write_bytes`(staging + 原子替换 + 对象锁),读/删走
//! `resolve`/`resolve_path` 取挂载路由后的 Operator 直连(读不需要
//! staging)。opendal 的 `NotFound` 映射为 [`Error::NotFound`]。

use std::sync::Arc;

use async_trait::async_trait;

use super::blob::BlobStore;
use super::error::{Error, Result};

pub struct VfsBlobStore {
    vfs: vfs::OpendalFileStorage,
    /// 所有 key 的公共前缀(如 `wiki-attachments`),与 vfs 挂载命名
    /// 空间隔离。
    prefix: String,
}

impl VfsBlobStore {
    /// 挂在 `storage` 的 `prefix` 虚拟目录下(默认建议 `wiki-attachments`)。
    pub fn new(storage: vfs::OpendalFileStorage, prefix: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            vfs: storage,
            prefix: prefix.into(),
        })
    }

    fn vpath(&self, key: &str) -> String {
        // key 由存储层生成(`attachments/<uuid>.<ext>`),无用户输入,
        // 但仍走 normalize_path 防御。
        vfs::OpendalFileStorage::normalize_path(&format!("{}/{}", self.prefix, key))
    }

    fn map_err(key: &str, e: vfs::opendal::Error) -> Error {
        if e.kind() == vfs::opendal::ErrorKind::NotFound {
            Error::NotFound(format!("{key} (blob)"))
        } else {
            Error::Unknown(format!("blob backend: {e}"))
        }
    }
}

#[async_trait]
impl BlobStore for VfsBlobStore {
    async fn put(&self, key: &str, data: Vec<u8>) -> Result<()> {
        self.vfs
            .write_bytes(&self.vpath(key), data)
            .await
            .map_err(|e| Self::map_err(key, e))
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>> {
        let vpath = self.vpath(key);
        let operator = self.vfs.resolve(&vpath);
        let path = self.vfs.resolve_path(&vpath);
        let buffer = operator
            .read(&path)
            .await
            .map_err(|e| Self::map_err(key, e))?;
        Ok(buffer.to_vec())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        // 幂等:目标不存在也算删净。
        match self.vfs.delete_object(&self.vpath(key)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == vfs::opendal::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Self::map_err(key, e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wiki-vfs-blob-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn vfs_blob_put_get_delete_roundtrip() {
        let root = temp_root();
        let store = VfsBlobStore::new(vfs::OpendalFileStorage::new(&root), "wiki-attachments");

        let payload = b"\x89PNG\r\n\x1a\nvfs-blob-bytes".to_vec();
        store
            .put("attachments/e1/a.png", payload.clone())
            .await
            .unwrap();

        let got = store.get("attachments/e1/a.png").await.unwrap();
        assert_eq!(got, payload);

        // 未知 key → NotFound(映射自 opendal)。
        assert!(matches!(
            store.get("attachments/e1/missing.png").await,
            Err(Error::NotFound(_))
        ));

        store.delete("attachments/e1/a.png").await.unwrap();
        assert!(store.get("attachments/e1/a.png").await.is_err());
        // 删除不存在的 key 是幂等成功。
        store.delete("attachments/e1/a.png").await.unwrap();

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn vfs_blob_put_overwrites() {
        let root = temp_root();
        let store = VfsBlobStore::new(vfs::OpendalFileStorage::new(&root), "wiki-attachments");

        store.put("k.bin", b"v1".to_vec()).await.unwrap();
        store.put("k.bin", b"v2-longer".to_vec()).await.unwrap();
        assert_eq!(store.get("k.bin").await.unwrap(), b"v2-longer");

        std::fs::remove_dir_all(&root).ok();
    }
}
