use arrow_array::RecordBatch;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::registry::NodeCtx;
use dag_core::value::{FileRef, PortType};
use datafusion::dataframe::DataFrame;
use sha2::{Digest, Sha256};

pub(crate) fn dataframe_output(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let dataframe: DataFrame = ctx
        .session()
        .read_batch(batch)
        .map_err(|error| DagError::Schedule(error.to_string()))?;
    let mut output = PortOutputs::new();
    output.insert(0, dataframe);
    Ok(output)
}

pub(crate) fn file_port() -> dag_core::node::NodePorts {
    dag_core::node::NodePorts::new().add_output_port_of_type(None, PortType::File)
}

pub(crate) async fn write_pdf(
    ctx: &NodeCtx,
    path: &str,
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
            .write_bytes(vpath, bytes)
            .await
            .map_err(|error| DagError::Schedule(format!("failed to write `{path}`: {error}")))?;
        Ok(FileRef::remote(
            path,
            Some("pdf".to_owned()),
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
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                DagError::Schedule(format!(
                    "failed to create directory `{}`: {error}",
                    parent.display()
                ))
            })?;
        }
        tokio::fs::write(dest, &bytes)
            .await
            .map_err(|error| DagError::Schedule(format!("failed to write `{path}`: {error}")))?;
        FileRef::local(dest, Some("pdf".to_owned()))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn request_error(error: crate::ProtocolioError) -> DagError {
    DagError::Schedule(format!("protocols.io request failed: {error}"))
}

pub(crate) fn batch_error(error: arrow_schema::ArrowError) -> DagError {
    DagError::Schedule(format!("protocols.io batch conversion failed: {error}"))
}

pub(crate) fn dataframe_port() -> dag_core::node::NodePorts {
    dag_core::node::NodePorts::new().add_output_port(None)
}
