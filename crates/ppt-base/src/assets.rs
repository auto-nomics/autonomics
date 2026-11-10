//! Deck asset & export artifact blob storage.
//!
//! Same shape as wiki's `VfsBlobStore` but self-contained (no shared
//! trait): a thin put/get/delete layer over [`vfs::OpendalFileStorage`]
//! with a per-area prefix (`deck-assets`, `deck-exports`), plus an
//! in-memory backend for tests and vfs-less deployments.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{Error, Result};

/// One blob area: a backend + key prefix.
#[derive(Clone)]
pub struct BlobArea {
    backend: Backend,
    prefix: String,
}

#[derive(Clone)]
enum Backend {
    #[cfg(feature = "vfs")]
    Vfs(vfs::OpendalFileStorage),
    Memory(Arc<std::sync::Mutex<HashMap<String, Vec<u8>>>>),
}

impl BlobArea {
    /// vfs-backed area under `prefix` (recommended: `deck-assets` /
    /// `deck-exports`).
    #[cfg(feature = "vfs")]
    pub fn on_vfs(storage: vfs::OpendalFileStorage, prefix: impl Into<String>) -> Self {
        Self {
            backend: Backend::Vfs(storage),
            prefix: prefix.into(),
        }
    }

    /// In-memory area (tests).
    pub fn in_memory(prefix: impl Into<String>) -> Self {
        Self {
            backend: Backend::Memory(Arc::new(std::sync::Mutex::new(HashMap::new()))),
            prefix: prefix.into(),
        }
    }

    fn vpath(&self, key: &str) -> String {
        format!("{}/{}", self.prefix.trim_matches('/'), key)
    }

    #[cfg(feature = "vfs")]
    fn vfs_path(&self, storage: &vfs::OpendalFileStorage, key: &str) -> Result<String> {
        storage
            .try_resolve_virtual(&vfs::uri::join_path(&self.prefix, key))
            .map_err(|e| Error::Asset(format!("blob address {key}: {e}")))
    }

    /// Write (or overwrite) a blob.
    pub async fn put(&self, key: &str, data: Vec<u8>) -> Result<()> {
        match &self.backend {
            #[cfg(feature = "vfs")]
            Backend::Vfs(storage) => {
                let vpath = self.vfs_path(storage, key)?;
                storage
                    .write_bytes(&vpath, data)
                    .await
                    .map_err(|e| Error::Asset(format!("blob write {key}: {e}")))
            }
            Backend::Memory(map) => {
                map.lock().expect("blob map poisoned").insert(self.vpath(key), data);
                Ok(())
            }
        }
    }

    /// Read a blob if it exists, distinguishing absence from storage failures.
    pub async fn get_optional(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match &self.backend {
            #[cfg(feature = "vfs")]
            Backend::Vfs(storage) => {
                let vpath = self.vfs_path(storage, key)?;
                let operator = storage.resolve(&vpath);
                let path = storage.resolve_path(&vpath);
                match operator.read(&path).await {
                    Ok(buf) => Ok(Some(buf.to_vec())),
                    Err(e) if e.kind() == vfs::opendal::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(Error::Asset(format!("blob read {key}: {e}"))),
                }
            }
            Backend::Memory(map) => Ok(map.lock().expect("blob map poisoned").get(&self.vpath(key)).cloned()),
        }
    }

    /// Read a blob; missing keys are [`Error::Asset`] with "not found".
    pub async fn get(&self, key: &str) -> Result<Vec<u8>> {
        match &self.backend {
            #[cfg(feature = "vfs")]
            Backend::Vfs(storage) => {
                let vpath = self.vfs_path(storage, key)?;
                let operator = storage.resolve(&vpath);
                let path = storage.resolve_path(&vpath);
                match operator.read(&path).await {
                    Ok(buf) => Ok(buf.to_vec()),
                    Err(e) if e.kind() == vfs::opendal::ErrorKind::NotFound => {
                        Err(Error::Asset(format!("{key} (blob) not found")))
                    }
                    Err(e) => Err(Error::Asset(format!("blob read {key}: {e}"))),
                }
            }
            Backend::Memory(map) => map
                .lock()
                .expect("blob map poisoned")
                .get(&self.vpath(key))
                .cloned()
                .ok_or_else(|| Error::Asset(format!("{key} (blob) not found"))),
        }
    }

    /// Delete a blob (idempotent).
    pub async fn delete(&self, key: &str) -> Result<()> {
        match &self.backend {
            #[cfg(feature = "vfs")]
            Backend::Vfs(storage) => {
                let vpath = self.vfs_path(storage, key)?;
                match storage.delete_object(&vpath).await {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == vfs::opendal::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(Error::Asset(format!("blob delete {key}: {e}"))),
                }
            }
            Backend::Memory(map) => {
                map.lock().expect("blob map poisoned").remove(&self.vpath(key));
                Ok(())
            }
        }
    }
}

/// Build a content-addressed asset blob key:
/// `assets/{deck_id}/{asset_id}.{ext}` (extension derived from the
/// filename; empty when unknown).
pub fn asset_blob_key(deck_id: &str, asset_id: &str, filename: &str) -> String {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    format!("assets/{deck_id}/{asset_id}{ext}")
}

/// Export artifact blob key: `exports/{deck_id}/{export_id}.{ext}`.
pub fn export_blob_key(deck_id: &str, export_id: &str, ext: &str) -> String {
    format!("exports/{deck_id}/{export_id}.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_backend_roundtrip() {
        let area = BlobArea::in_memory("deck-assets");
        area.put("assets/d1/a.png", b"bytes".to_vec()).await.unwrap();
        assert_eq!(area.get("assets/d1/a.png").await.unwrap(), b"bytes");
        assert!(area.get("assets/d1/missing").await.is_err());
        area.delete("assets/d1/a.png").await.unwrap();
        assert!(area.get("assets/d1/a.png").await.is_err());
        // idempotent delete
        area.delete("assets/d1/a.png").await.unwrap();
    }

    #[cfg(feature = "vfs")]
    #[tokio::test]
    async fn domain_blob_prefix_preserves_escaped_keys_and_refuses_unknown_domains() {
        let storage = vfs::OpendalFileStorage::new_temp();
        let area = BlobArea::on_vfs(storage.clone(), "local://deck%20assets");
        area.put("a %.txt", b"bytes".to_vec()).await.unwrap();
        assert_eq!(area.get("a %.txt").await.unwrap(), b"bytes");
        assert_eq!(
            storage
                .content_length("/deck assets/a %.txt")
                .await
                .unwrap(),
            5
        );
        area.delete("a %.txt").await.unwrap();
        assert!(area.get_optional("a %.txt").await.unwrap().is_none());
        let unknown = BlobArea::on_vfs(storage, "missing://assets");
        assert!(unknown.put("a", vec![1]).await.is_err());
        assert!(unknown.get_optional("a").await.is_err());
        assert!(unknown.delete("a").await.is_err());
    }

    #[test]
    fn asset_keys() {
        assert_eq!(
            asset_blob_key("d1", "a1", "figure.TIFF"),
            "assets/d1/a1.tiff"
        );
        assert_eq!(asset_blob_key("d1", "a2", "noext"), "assets/d1/a2");
        assert_eq!(
            export_blob_key("d1", "e1", "pdf"),
            "exports/d1/e1.pdf"
        );
    }
}
