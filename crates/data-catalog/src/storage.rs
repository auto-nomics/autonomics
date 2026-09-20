use vfs::{BackendDefinition, VfsManifest};

use crate::error::Result;

/// Build a raw object-store operator from a `vfs.toml` backend definition.
///
/// The catalog borrows the `[[backend]]` table for credentials and endpoints;
/// no VFS mounts or path mappings are constructed.
pub fn operator_for_backend(manifest: &VfsManifest, backend_id: &str) -> Result<opendal::Operator> {
    let backend = backend_definition(manifest, backend_id)?;
    Ok(backend
        .config
        .build()
        .map_err(|error| format!("build catalog backend `{backend_id}`: {error}"))?)
}

pub async fn read_json_object<T: serde::de::DeserializeOwned>(
    operator: &opendal::Operator,
    key: &str,
) -> Result<T> {
    let bytes = operator
        .read(key)
        .await
        .map_err(|error| format!("read object `{key}`: {error}"))?;
    serde_json::from_slice(&bytes.to_vec())
        .map_err(|error| format!("parse object `{key}`: {error}").into())
}

pub async fn write_json_object<T: serde::Serialize>(
    operator: &opendal::Operator,
    key: &str,
    value: &T,
) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    write_object(operator, key, &bytes).await
}

pub async fn write_object(operator: &opendal::Operator, key: &str, bytes: &[u8]) -> Result<()> {
    let pending = format!("{key}.pending-{}", uuid::Uuid::new_v4());
    let mut writer = operator
        .writer(&pending)
        .await
        .map_err(|error| format!("create object `{key}`: {error}"))?;
    writer
        .write(bytes.to_vec())
        .await
        .map_err(|error| format!("write object `{key}`: {error}"))?;
    writer
        .close()
        .await
        .map_err(|error| format!("close object `{key}`: {error}"))?;
    publish_pending(operator, &pending, key).await?;
    Ok(())
}

pub async fn upload_file(
    operator: &opendal::Operator,
    key: &str,
    path: &std::path::Path,
) -> Result<()> {
    let mut input = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    let pending = format!("{key}.pending-{}", uuid::Uuid::new_v4());
    let mut writer = operator
        .writer(&pending)
        .await
        .map_err(|error| format!("create object `{key}`: {error}"))?;
    let mut chunk = vec![0_u8; 1024 * 1024];
    use tokio::io::AsyncReadExt;
    loop {
        let read = input
            .read(&mut chunk)
            .await
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        writer
            .write(chunk[..read].to_vec())
            .await
            .map_err(|error| format!("write object `{key}`: {error}"))?;
    }
    writer
        .close()
        .await
        .map_err(|error| format!("close object `{key}`: {error}"))?;
    publish_pending(operator, &pending, key).await?;
    Ok(())
}

async fn publish_pending(operator: &opendal::Operator, pending: &str, key: &str) -> Result<()> {
    if let Err(error) = operator.rename(pending, key).await {
        // Garage supports copy/delete but not S3 server-side rename. Copy only
        // starts after the complete pending object has been closed.
        if let Err(copy_error) = operator.copy(pending, key).await {
            let _ = operator.delete(pending).await;
            return Err(format!(
                "publish object `{key}`: rename failed ({error}); copy fallback failed ({copy_error})"
            )
            .into());
        }
        let _ = operator.delete(pending).await;
    }
    Ok(())
}

pub fn backend_definition(manifest: &VfsManifest, backend_id: &str) -> Result<BackendDefinition> {
    manifest
        .backend
        .iter()
        .find(|backend| backend.id == backend_id)
        .cloned()
        .ok_or_else(|| format!("catalog backend `{backend_id}` is not defined").into())
}
