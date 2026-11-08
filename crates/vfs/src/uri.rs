//! Central vfs URI parsing and construction.
//!
//! Every `vfs://` string in the workspace — node specs, published
//! artifacts, exports, bib attachments, panel manifests — is parsed and
//! built through this module instead of ad-hoc
//! `strip_prefix("vfs://")` at the call site. The scheme is currently a
//! single virtual namespace, so `virtual_path`/`is_uri` carry no extra
//! information over the literal prefix; the point of the indirection is
//! that scheme semantics (per-backend kinds, origin validation) gain
//! exactly one place to change.
//!
//! Resolution itself stays prefix-based over the mount table: the string
//! after [`virtual_path`] is matched against mount `virtual_prefix`es by
//! [`crate::mount`]. Scheme and mount prefix must not disagree once the
//! scheme becomes authoritative.

/// The scheme name of the virtual namespace.
pub const SCHEME: &str = "vfs";

/// Wire prefix of [`SCHEME`].
pub const SCHEME_PREFIX: &str = "vfs://";

/// Strip the `vfs://` scheme prefix, returning the virtual path.
///
/// Returns `None` for absolute local paths and any other scheme — the
/// caller decides what a non-virtual path means in its context.
pub fn virtual_path(uri: &str) -> Option<&str> {
    uri.strip_prefix(SCHEME_PREFIX)
}

/// Whether `path` addresses the virtual namespace (`vfs://…`).
pub fn is_uri(path: &str) -> bool {
    path.starts_with(SCHEME_PREFIX)
}

/// Build a `vfs://` URI from a virtual path.
pub fn uri(virtual_path: impl std::fmt::Display) -> String {
    format!("{SCHEME_PREFIX}{virtual_path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_path_strips_only_the_scheme() {
        assert_eq!(virtual_path("vfs:///input.tsv"), Some("/input.tsv"));
        assert_eq!(virtual_path("vfs://data/x.bin"), Some("data/x.bin"));
        assert_eq!(virtual_path("/abs/local.tsv"), None);
        assert_eq!(virtual_path("vfs:/x"), None, "partial prefix must not match");
        assert_eq!(virtual_path("ssh://host/x"), None);
    }

    #[test]
    fn is_uri_matches_the_same_set() {
        assert!(is_uri("vfs://x"));
        assert!(!is_uri("vfs:/x"));
        assert!(!is_uri("vfsx://y"));
    }

    #[test]
    fn uri_round_trips_through_virtual_path() {
        let built = uri(format!("data/{}", "x.bin"));
        assert_eq!(built, "vfs://data/x.bin");
        assert_eq!(virtual_path(&built), Some("data/x.bin"));
    }
}
