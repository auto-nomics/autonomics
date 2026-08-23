//! Shared naming helpers for full-text objects in the bibliography VFS.

use sha2::{Digest, Sha256};

pub const VFS_PREFIX: &str = "vfs://";

/// A stable, content-addressed VFS location for an uploaded full text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFulltext {
    pub path: String,
    pub file_hash: String,
}

/// Build a collision-resistant literature object path.
///
/// Including the SHA-256 prefix means replacing a file with different content
/// never overwrites the object still referenced by the old database row. The
/// old object can therefore be deleted only after the database commit.
pub fn stored_fulltext(article_id: &str, filename: &str, content: &[u8]) -> StoredFulltext {
    let file_hash = hex(&Sha256::digest(content));
    let filename = encode_component(filename);
    let path = format!(
        "{VFS_PREFIX}/literature/{}/{file_hash}-{filename}",
        encode_component(article_id)
    );
    StoredFulltext { path, file_hash }
}

/// Convert a bibliography VFS URI back to the virtual path expected by
/// [`vfs::OpendalFileStorage`].
pub fn vfs_virtual_path(uri: &str) -> Option<String> {
    let path = uri.strip_prefix(VFS_PREFIX)?.trim_start_matches('/');
    (!path.is_empty()).then(|| format!("/{path}"))
}

fn encode_component(value: &str) -> String {
    if value.is_empty() {
        return "unnamed".to_owned();
    }
    let special_component = value == "." || value == "..";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        let safe = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.');
        let safe = safe && !(special_component && byte == b'.');
        if safe {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_content_addressed_and_escapes_components() {
        let a = stored_fulltext("doi:10.1000/x", "paper one.pdf", b"content");
        let b = stored_fulltext("doi:10.1000/x", "paper one.pdf", b"changed");
        assert_ne!(a.path, b.path);
        assert!(a.path.starts_with("vfs:///literature/doi%3A10.1000%2Fx/"));
        assert!(a.path.ends_with("-paper%20one.pdf"));
        assert_eq!(a.file_hash.len(), 64);
    }

    #[test]
    fn parses_virtual_path() {
        assert_eq!(
            vfs_virtual_path("vfs:///literature/a/file.txt"),
            Some("/literature/a/file.txt".to_owned())
        );
        assert_eq!(vfs_virtual_path("europepmc:PMC1"), None);
    }
}
