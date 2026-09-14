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

/// VFS location of a figure image extracted for `article_id`.
///
/// Unlike [`stored_fulltext`] there is no content-hash prefix: MinerU
/// derives figure file names from the image content itself, so names are
/// already stable and re-extraction overwrites in place.
pub fn figure_object_path(article_id: &str, name: &str) -> String {
    format!(
        "{VFS_PREFIX}/literature/{}/images/{}",
        encode_component(article_id),
        encode_component(name)
    )
}

/// Normalize a figure reference from markdown or a URL segment.
///
/// Accepts a bare name or an `images/`-prefixed path and returns the
/// basename. The name must be within `[A-Za-z0-9._-]` and must not start
/// with a dot — anything else (traversal attempts, empty input) is
/// rejected with `None`.
pub fn sanitize_figure_name(raw: &str) -> Option<String> {
    let stripped = raw.strip_prefix("images/").unwrap_or(raw);
    let base = stripped.rsplit('/').next().unwrap_or(stripped);
    let valid = !base.is_empty()
        && !base.starts_with('.')
        && base
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    valid.then(|| base.to_owned())
}

const IMAGES_MARKER: &str = "images/";

/// Collect the figure names a markdown text references
/// (`![](images/<name>.jpg)`), deduplicated, in order of first appearance.
///
/// The markdown is the source of truth for which figures exist — the
/// cleanup paths derive "old minus new" sets from these scans.
pub fn scan_image_refs(markdown: &str) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    let mut search_from = 0;
    while let Some(offset) = markdown[search_from..].find(IMAGES_MARKER) {
        let name_start = search_from + offset + IMAGES_MARKER.len();
        let name_end = markdown[name_start..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
            .map(|relative| name_start + relative)
            .unwrap_or(markdown.len());
        if name_end > name_start
            && let Some(name) = sanitize_figure_name(&markdown[name_start..name_end])
            && !refs.contains(&name)
        {
            refs.push(name);
        }
        search_from = name_end.max(name_start);
    }
    refs
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

    #[test]
    fn builds_figure_paths() {
        assert_eq!(
            figure_object_path("doi:10.1000/x", "abc.jpg"),
            "vfs:///literature/doi%3A10.1000%2Fx/images/abc.jpg"
        );
    }

    #[test]
    fn sanitizes_figure_names() {
        assert_eq!(sanitize_figure_name("abc.jpg"), Some("abc.jpg".into()));
        assert_eq!(
            sanitize_figure_name("images/abc.jpg"),
            Some("abc.jpg".into())
        );
        assert_eq!(sanitize_figure_name("a/b/c.png"), Some("c.png".into()));
        assert_eq!(sanitize_figure_name(".."), None);
        assert_eq!(sanitize_figure_name(".hidden"), None);
        assert_eq!(sanitize_figure_name(""), None);
        assert_eq!(sanitize_figure_name("bad name.jpg"), None);
        assert_eq!(sanitize_figure_name("%2e%2e"), None);
    }

    #[test]
    fn scans_image_refs_from_markdown() {
        let markdown = "text ![](images/aaa.jpg) more\n\
                        ![alt](images/bbb.png \"title\")\n\
                        `images/ccc.jpg`\n\
                        repeat ![](images/aaa.jpg)\n\
                        outside ![](https://example.com/images/ddd.jpg)\n\
                        not-a-ref images/ bad\n";
        let refs = scan_image_refs(markdown);
        // The remote URL also carries an images/ segment — its name is
        // captured by design (dedup keeps it harmless); the bare marker
        // followed by a space yields nothing.
        assert_eq!(refs, ["aaa.jpg", "bbb.png", "ccc.jpg", "ddd.jpg"]);
    }
}
