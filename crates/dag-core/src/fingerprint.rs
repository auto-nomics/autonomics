//! Plugin implementation identity and file content hashing.
//!
//! [`PluginIdentity`] pins the *measured software environment* a node
//! executes in (manifest bytes, staged script, digest-pinned image, mounted
//! panel digests). It is carried on plugin-backed nodes via
//! [`crate::node::DagNode::plugin_identity`] and included in task manifests,
//! so provenance evidence reflects exactly what executed — never a
//! re-read that could drift from the loaded definition.
//!
//! [`file_sha256`] provides content-level hashing over resolved file-like
//! values, shared by the flow engine's resume fingerprints and input
//! validation.

use sha2::{Digest, Sha256};

/// The identity of the plugin implementation that executes a plugin-backed
/// node.
///
/// Built by the plugin loader from the *loaded* definition (not re-read from
/// disk at dispatch time), so the identity always reflects what the
/// engine would actually execute.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PluginIdentity {
    /// `sha256:{hex}` over the family `manifest.toml` bytes exactly as
    /// loaded — every declaration (node definitions, params, panels, image
    /// metadata) participates at the byte level.
    pub manifest_sha256: String,
    /// `sha256:{hex}` over the exact script source the node stages into the
    /// container. For `script_file` nodes this is the referenced file's
    /// content (inlined by the loader before validation); for inline
    /// `script` nodes the manifest hash already covers it and this is the
    /// same bytes hashed uniformly. `None` for argv-only commands.
    pub script_sha256: Option<String>,
    /// Full image reference string (`host/path@sha256:…`) from the
    /// manifest's `[image]` table — pinned by digest, so any digest change
    /// is a different execution environment.
    pub image_reference: String,
    /// Catalog panels mounted for this node. The declaration (binding,
    /// mount, bundle id) is covered by the manifest hash; the `digest` pins
    /// *which content* the catalog resolved, when it is known.
    pub panels: Vec<PanelIdentity>,
}

/// One mounted catalog panel, in the shape that matters for identity.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PanelIdentity {
    /// Local binding slot the node mounts through.
    pub binding: String,
    /// Stable bundle identifier in the runtime `BundleRegistry`.
    pub bundle_id: String,
    /// Immutable catalog content digest of the resolved bundle, when known.
    pub digest: Option<String>,
}

/// Best-effort sha256 over a resolved file-like value. Returns the bare
/// lowercase hex digest.
///
/// `vfs://` values read the object store; bare paths and `file://` values read
/// the host filesystem. These are upstream FileRef identities, not unresolved
/// node-spec paths: a virtual mount must not replace the host bytes consumed
/// downstream, or hide their modification or disappearance.
pub async fn file_sha256(
    path: &str,
    storage: Option<&vfs::OpendalFileStorage>,
) -> Option<(u64, String)> {
    if let Some(vpath) = vfs::uri::storage_path(path) {
        let storage = storage?;
        return storage
            .sha256_with_progress(&storage.try_resolve_virtual(vpath).ok()?, |_, _| {})
            .await
            .ok();
    }
    host_sha256(path.strip_prefix("file://").unwrap_or(path)).await
}

async fn host_sha256(path: &str) -> Option<(u64, String)> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await.ok()?;
    let metadata = file.metadata().await.ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut bytes_read = 0u64;
    loop {
        let read = file.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes_read += read as u64;
    }
    let after = file.metadata().await.ok()?;
    if bytes_read != metadata.len()
        || after.len() != metadata.len()
        || after.modified().ok() != metadata.modified().ok()
    {
        return None;
    }
    Some((bytes_read, hex_lower(&hasher.finalize())))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_sha256_hashes_host_files_and_misses_cleanly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("a.bin");
        std::fs::write(&path, b"autonomics").unwrap();
        let (size, digest) = file_sha256(path.to_str().unwrap(), None)
            .await
            .expect("hash a local file");
        assert_eq!(size, 10);
        assert_eq!(digest.len(), 64, "bare lowercase hex digest");
        assert!(digest.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert!(
            file_sha256(directory.path().join("missing.bin").to_str().unwrap(), None)
                .await
                .is_none(),
            "a missing file is a miss, not a guess"
        );
    }
}
