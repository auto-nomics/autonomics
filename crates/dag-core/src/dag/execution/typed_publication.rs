//! Publication of declared directory outputs in the coordinator's namespace.
//! Worker-local paths and coordinator VFS paths are never interchangeable.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use sha2::{Digest, Sha256};

use super::typed_worker::TypedWorkerEnvelope;
use crate::dag::DagError;
use crate::dag::PortOutputs;
use crate::{FileRef, NodeCtx, NodeValue};

const MANIFEST_NAME: &str = "zz-publication.json";
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
const CHUNK_BYTES: u64 = 1024 * 1024;

fn failure(message: impl Into<String>) -> DagError {
    DagError::Schedule(message.into())
}

pub(super) fn worker_root(workspace: &Path, port: u8) -> std::path::PathBuf {
    workspace
        .join("typed-publications")
        .join(format!("port-{port}"))
}

pub(super) fn write_manifest(
    root: &Path,
    workspace: &Path,
    port: u8,
    files: &[FileRef],
) -> Result<(), DagError> {
    let base = worker_root(workspace, port);
    let paths = files
        .iter()
        .map(|file| {
            let path = file.path.strip_prefix("file://").unwrap_or(&file.path);
            let relative = Path::new(path)
                .strip_prefix(&base)
                .map_err(|_| failure("worker publication escaped its declared directory"))?;
            let relative = relative
                .to_str()
                .ok_or_else(|| failure("publication path must be UTF-8"))?;
            validate_member(relative)?;
            Ok(relative.to_owned())
        })
        .collect::<Result<Vec<_>, DagError>>()?;
    let bytes = serde_json::to_vec(&paths).map_err(|e| failure(e.to_string()))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(failure("worker publication manifest exceeds 8 MiB"));
    }
    std::fs::write(root.join(MANIFEST_NAME), bytes).map_err(|e| failure(e.to_string()))
}

fn validate_member(path: &str) -> Result<(), DagError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(failure("invalid relative worker publication path"));
    }
    Ok(())
}

async fn read_chunk(
    file: &FileRef,
    ctx: &NodeCtx,
    offset: u64,
    end: u64,
) -> Result<Vec<u8>, DagError> {
    if let Some(path) = vfs::uri::storage_path(&file.path) {
        let storage = ctx
            .opendal
            .as_ref()
            .ok_or_else(|| failure("publication requires VFS"))?;
        storage
            .read_range(path, offset..end)
            .await
            .map(|b| b.to_vec())
            .map_err(|e| failure(e.to_string()))
    } else {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let mut input =
            tokio::fs::File::open(file.path.strip_prefix("file://").unwrap_or(&file.path))
                .await
                .map_err(|e| failure(e.to_string()))?;
        input
            .seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| failure(e.to_string()))?;
        let mut bytes = vec![0; (end - offset) as usize];
        input
            .read_exact(&mut bytes)
            .await
            .map_err(|e| failure(e.to_string()))?;
        Ok(bytes)
    }
}

async fn ensure_fresh(ctx: &NodeCtx, target: &str) -> Result<(), DagError> {
    let storage = ctx
        .opendal
        .as_ref()
        .ok_or_else(|| failure("publication requires VFS"))?;
    storage
        .check_writable(target)
        .map_err(|e| failure(e.to_string()))?;
    storage
        .canonical_uri(target)
        .map_err(|e| failure(e.to_string()))?;
    // Reject static symlinks and non-directory ancestors on local mounts.
    if let Some(local) = storage.local_host_path(target) {
        for ancestor in local.ancestors() {
            match std::fs::symlink_metadata(ancestor) {
                Ok(meta)
                    if meta.file_type().is_symlink() || ancestor == local || !meta.is_dir() =>
                {
                    return Err(failure(format!(
                        "unsafe or existing publication target `{target}`"
                    )));
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(failure(e.to_string())),
            }
        }
    }
    let operator = storage.resolve(target);
    match operator.stat(&storage.resolve_path(target)).await {
        Err(e) if e.kind() == vfs::opendal::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(failure(format!(
            "existing output not overwritten: `{target}`"
        ))),
        Err(e) => Err(failure(e.to_string())),
    }
}

async fn publish_file(ctx: &NodeCtx, file: &FileRef, target: &str) -> Result<FileRef, DagError> {
    let fingerprint = file
        .fingerprint
        .as_ref()
        .ok_or_else(|| failure("unmeasured publication file"))?;
    let expected = fingerprint
        .content_hash
        .as_deref()
        .ok_or_else(|| failure("publication file lacks SHA256"))?;
    ensure_fresh(ctx, target).await?;
    let storage = ctx
        .opendal
        .as_ref()
        .ok_or_else(|| failure("publication requires VFS"))?;
    let canonical = storage
        .canonical_uri(target)
        .map_err(|e| failure(e.to_string()))?;
    let mut writer = storage
        .atomic_writer(target)
        .await
        .map_err(|e| failure(e.to_string()))?;
    let transfer = async {
        let mut hash = Sha256::new();
        let mut offset = 0;
        while offset < fingerprint.size {
            let end = (offset + CHUNK_BYTES).min(fingerprint.size);
            let bytes = read_chunk(file, ctx, offset, end).await?;
            if bytes.len() as u64 != end - offset {
                return Err(failure("short publication read"));
            }
            hash.update(&bytes);
            writer
                .write(bytes)
                .await
                .map_err(|e| failure(e.to_string()))?;
            offset = end;
        }
        if format!("{:x}", hash.finalize()) != expected.strip_prefix("sha256:").unwrap_or(expected)
        {
            return Err(failure("worker publication SHA256 mismatch"));
        }
        Ok(())
    }
    .await;
    if let Err(error) = transfer {
        let _ = writer.abort().await;
        return Err(error);
    }
    writer
        .commit_new()
        .await
        .map_err(|e| failure(e.to_string()))?;
    let mut published = file.clone();
    published.path = canonical;
    published.domain = crate::DomainId::new(vfs::uri::domain(&published.path).unwrap_or("local"));
    published.fingerprint.as_mut().unwrap().immutable_remote = false;
    Ok(published)
}

pub(super) async fn publish(
    envelope: &TypedWorkerEnvelope,
    outputs: &mut PortOutputs,
    ctx: &NodeCtx,
) -> Result<BTreeMap<u8, Vec<FileRef>>, DagError> {
    let mut planned = Vec::new();
    let mut targets = BTreeSet::new();
    for (pointer, port) in &envelope.output_directories {
        let destination = envelope
            .task
            .spec
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .and_then(|s| vfs::uri::storage_path(s))
            .ok_or_else(|| failure("coordinator publication destination must be VFS"))?;
        let destination = ctx
            .opendal
            .as_ref()
            .ok_or_else(|| failure("publication requires VFS"))?
            .try_resolve_virtual(destination)
            .map_err(|e| failure(e.to_string()))?;
        let destination = destination.as_str();
        let files = outputs
            .get(port)
            .ok_or_else(|| failure("missing publication output"))?
            .as_file_set()?;
        let (manifest, payloads) = files
            .split_last()
            .ok_or_else(|| failure("missing publication manifest"))?;
        let size = manifest
            .fingerprint
            .as_ref()
            .map(|f| f.size)
            .filter(|size| *size <= MAX_MANIFEST_BYTES)
            .ok_or_else(|| failure("unmeasured or oversized publication manifest"))?;
        let bytes = read_chunk(manifest, ctx, 0, size).await?;
        let expected = manifest
            .fingerprint
            .as_ref()
            .and_then(|f| f.content_hash.as_deref())
            .ok_or_else(|| failure("publication manifest lacks SHA256"))?;
        if format!("{:x}", Sha256::digest(&bytes))
            != expected.strip_prefix("sha256:").unwrap_or(expected)
        {
            return Err(failure("publication manifest SHA256 mismatch"));
        }
        let members: Vec<String> =
            serde_json::from_slice(&bytes).map_err(|e| failure(e.to_string()))?;
        if members.len() != payloads.len() || members.is_empty() {
            return Err(failure(
                "publication manifest count differs from transported files",
            ));
        }
        for (member, file) in members.into_iter().zip(payloads) {
            validate_member(&member)?;
            let target = format!("{}/{member}", destination.trim_end_matches('/'));
            if !targets.insert(target.clone()) {
                return Err(failure("duplicate publication target"));
            }
            ensure_fresh(ctx, &target).await?;
            planned.push((*port, target, file.clone()));
        }
    }
    let mut published: BTreeMap<u8, Vec<FileRef>> = BTreeMap::new();
    for (port, target, file) in planned {
        let file = publish_file(ctx, &file, &target).await.map_err(|error| {
            let paths = published.values().flatten().map(|file| file.path.as_str()).take(16).collect::<Vec<_>>();
            failure(format!("coordinator publication failed for `vfs://{target}`: {error}; already published {} files: {paths:?}", published.values().map(Vec::len).sum::<usize>()))
        })?;
        published.entry(port).or_default().push(file);
    }
    for (port, files) in &published {
        outputs.insert(*port, NodeValue::FileSet(files.clone()));
    }
    Ok(published)
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;
    use serde_json::json;
    use std::sync::Arc;

    fn envelope() -> TypedWorkerEnvelope {
        serde_json::from_value(json!({
            "protocol":1,
            "worker":{"executable":"/worker", "executable_sha256":"a".repeat(64),
                "expected_source_revision":"test", "plugins_root":null,"plugins_sha256":null,
                "apptainer_catalog":null,"apptainer_executable":null,"worker_vfs_manifest":null,"worker_vfs_sha256":null},
            "task":{"id":"extract","kind":"core/archive_extract", "spec":{"destination_prefix":"vfs:///work/out"},
                "inputs":[],"outputs":[{"name":"files","port":0,"payload":"file_set","artifact_paths":[]}]},
            "input_files":{},"output_directories":{"/destination_prefix":0},"bundles":[],"plugin_identity":null
        })).unwrap()
    }

    fn measured(path: &Path, bytes: &[u8]) -> FileRef {
        std::fs::write(path, bytes).unwrap();
        let mut file = FileRef::local(path, None).unwrap();
        file.fingerprint.as_mut().unwrap().content_hash =
            Some(format!("{:x}", Sha256::digest(bytes)));
        file
    }

    #[tokio::test]
    async fn publication_rejects_traversal_duplicates_and_mutated_transport_bytes() {
        for (names, mutate) in [
            (vec!["../escape.md", "other.md"], false),
            (vec!["nested/主方案.md", "nested/主方案.md"], false),
            (vec!["a.md", "b.md"], true),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let storage = Arc::new(vfs::OpendalFileStorage::new(temp.path().join("vfs")));
            let ctx = NodeCtx::new(SessionContext::new().runtime_env(), Some(storage));
            let a = measured(&temp.path().join("a"), b"original");
            let b = measured(&temp.path().join("b"), b"second");
            if mutate {
                std::fs::write(&a.path, b"modified").unwrap();
            }
            let manifest = measured(
                &temp.path().join("manifest"),
                &serde_json::to_vec(&names).unwrap(),
            );
            let mut outputs = PortOutputs::new();
            outputs.insert(0, NodeValue::FileSet(vec![a, b, manifest]));
            assert!(publish(&envelope(), &mut outputs, &ctx).await.is_err());
            assert!(!temp.path().join("vfs/work/out/a.md").exists());
            assert!(!temp.path().join("vfs/work/escape.md").exists());
        }
    }

    #[tokio::test]
    async fn all_targets_are_checked_before_publishing_and_symlinks_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("vfs");
        std::fs::create_dir_all(root.join("work/out")).unwrap();
        std::fs::write(root.join("work/out/blocked.md"), b"history").unwrap();
        let storage = Arc::new(vfs::OpendalFileStorage::new(&root));
        let ctx = NodeCtx::new(SessionContext::new().runtime_env(), Some(storage));
        let a = measured(&temp.path().join("a"), b"first");
        let b = measured(&temp.path().join("b"), b"second");
        let manifest = measured(&temp.path().join("manifest"), br#"["new.md","blocked.md"]"#);
        let mut outputs = PortOutputs::new();
        outputs.insert(0, NodeValue::FileSet(vec![a, b, manifest]));
        assert!(publish(&envelope(), &mut outputs, &ctx).await.is_err());
        assert!(!root.join("work/out/new.md").exists());
        assert_eq!(
            std::fs::read(root.join("work/out/blocked.md")).unwrap(),
            b"history"
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(temp.path(), root.join("work/out/link")).unwrap();
            assert!(
                ensure_fresh(&ctx, "/work/out/link/escape.md")
                    .await
                    .is_err()
            );
        }
    }
}
