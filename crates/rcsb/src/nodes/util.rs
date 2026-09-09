use std::sync::Arc;

use dag_core::dag::DagError;
use dag_core::registry::NodeCtx;
use dag_core::value::{FileRef, PortType};
use sha2::{Digest, Sha256};

pub(super) async fn write_output(
    ctx: &NodeCtx,
    path: &str,
    format: &str,
    bytes: Vec<u8>,
) -> Result<FileRef, DagError> {
    let size = bytes.len() as u64;
    let sha256 = sha256_hex(&bytes);

    if let Some(vpath) = path.strip_prefix("vfs://") {
        let storage = ctx.opendal.as_ref().ok_or_else(|| {
            DagError::Schedule(format!(
                "path `{path}` requires engine file storage, but none is registered"
            ))
        })?;
        storage
            .check_writable(vpath)
            .map_err(|e| DagError::Schedule(format!("vfs path `{path}` is not writable: {e}")))?;
        storage
            .write_bytes(vpath, bytes)
            .await
            .map_err(|e| DagError::Schedule(format!("failed to write `{path}`: {e}")))?;
        Ok(FileRef::remote(
            path,
            Some(format.to_owned()),
            size,
            Some(sha256),
        ))
    } else {
        let dest = std::path::Path::new(path);
        if !dest.is_absolute() {
            return Err(DagError::Schedule(format!(
                "path `{path}` must be a `vfs://` address or an absolute local path"
            )));
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                DagError::Schedule(format!(
                    "failed to create directory `{}`: {e}",
                    parent.display()
                ))
            })?;
        }
        tokio::fs::write(dest, &bytes)
            .await
            .map_err(|e| DagError::Schedule(format!("failed to write `{path}`: {e}")))?;
        FileRef::local(dest, Some(format.to_owned()))
    }
}

pub(super) fn file_port() -> dag_core::node::NodePorts {
    dag_core::node::NodePorts::new().add_output_port_of_type(None, PortType::File)
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub(super) fn str_array(rows: Vec<Option<String>>) -> Arc<dyn arrow_array::Array> {
    Arc::new(arrow_array::StringArray::from(rows))
}
