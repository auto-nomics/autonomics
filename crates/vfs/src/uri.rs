//! Shared parsing for filesystem-domain addresses and legacy VFS URIs.
//!
//! Canonical addresses use `<domain>://path`, such as `local://work/x`
//! or `ht://share/x`. The mount-aware storage resolver validates the domain,
//! confines relative components, and chooses the backend. `vfs://` remains
//! a compatibility spelling for the composite virtual namespace.
//!
//! Use `storage_path` to route byte I/O and `domain` to inspect an explicit
//! placement. `virtual_path` and `is_uri` recognize only the legacy spelling.
//! Construct canonical public addresses with `OpendalFileStorage::canonical_uri`.

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

/// Encode a virtual path for a domain URI, preserving only path separators
/// and RFC 3986 unreserved bytes. In particular literal '%' is encoded.
pub fn encode_path(path: &str) -> String {
    let mut output = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

/// Append an unescaped relative object path to a URI or virtual prefix.
/// Existing prefix escapes are preserved; only the appended key is encoded.
pub fn join_path(prefix: &str, relative: &str) -> String {
    let relative = relative.trim_start_matches('/');
    if let Some((scheme, rest)) = split_scheme(prefix) {
        if domain(prefix).is_some() {
            let base = rest.trim_matches('/');
            let suffix = encode_path(relative);
            return if base.is_empty() {
                format!("{scheme}://{suffix}")
            } else {
                format!("{scheme}://{base}/{suffix}")
            };
        }
        if rest.trim_matches('/').is_empty() {
            return format!("{scheme}:///{relative}");
        }
        return format!("{}/{relative}", prefix.trim_end_matches('/'));
    }
    format!("{}/{}", prefix.trim_end_matches('/'), relative)
}

/// Adapt a VFS domain address to a URL library's path-only representation.
/// Public domain addresses have no authority: every byte after `://` belongs
/// to the path. DataFusion's standard URL parser requires a leading path slash
/// to avoid interpreting the first path segment as a hostname. Escapes are kept
/// intact, and legacy VFS / native filesystem URLs retain their spelling.
pub fn datafusion_uri(address: &str) -> String {
    match split_scheme(address) {
        Some((domain, rest)) if valid_domain_name(domain) => {
            format!("{domain}:///{}", rest.trim_start_matches('/'))
        }
        _ => address.to_owned(),
    }
}

/// Decode a domain path once before traversal and mount checks. Encoded
/// separators are refused, so a segment cannot acquire a hidden boundary.
pub fn decode_path(path: &str) -> Result<String, &'static str> {
    let bytes = path.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            let pair = bytes
                .get(index + 1..index + 3)
                .ok_or("truncated URI escape")?;
            let digit = |b: u8| (b as char).to_digit(16).map(|n| n as u8);
            let value = (digit(pair[0]).ok_or("invalid URI escape")? << 4)
                | digit(pair[1]).ok_or("invalid URI escape")?;
            if matches!(value, b'/' | b'\\' | 0) {
                return Err("encoded URI separator or NUL");
            }
            output.push(value);
            index += 3;
        } else {
            if matches!(byte, b'?' | b'#' | b'\\' | 0) {
                return Err("URI path requires percent encoding");
            }
            output.push(byte);
            index += 1;
        }
    }
    String::from_utf8(output).map_err(|_| "URI path is not UTF-8")
}

/// Domain names are canonical lowercase URI schemes. Legacy transport and
/// virtual-namespace schemes cannot be claimed by a domain mount.
pub fn valid_domain_name(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"+.-".contains(&b))
        && !matches!(name, "vfs" | "file" | "http" | "https")
}

/// Split any `<scheme>://rest` address into its scheme and the rest.
/// `None` when the string carries no scheme separator. This is the
/// admission point for domain addresses (`hpc-b://…`): the scheme names
/// a mount-declared domain and the mount table resolves it.
pub fn split_scheme(uri: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = uri.split_once("://")?;
    if scheme.is_empty() || scheme.starts_with('-') {
        return None;
    }
    Some((scheme, rest))
}

/// Address admitted through the mounted storage layer. Legacy `vfs://`
/// addresses return their virtual path; domain addresses retain their scheme
/// until the mount-aware resolver validates them. `file://` remains a host path.
/// Unknown schemes deliberately reach the resolver and fail closed there.
pub fn storage_path(address: &str) -> Option<&str> {
    let (scheme, _) = split_scheme(address)?;
    if scheme == "file" {
        None
    } else {
        Some(virtual_path(address).unwrap_or(address))
    }
}

/// Domain encoded in an address. Legacy virtual paths need mount context.
pub fn domain(address: &str) -> Option<&str> {
    let (scheme, _) = split_scheme(address)?;
    (scheme != SCHEME && scheme != "file").then_some(scheme)
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
        assert_eq!(
            virtual_path("vfs:/x"),
            None,
            "partial prefix must not match"
        );
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

#[cfg(test)]
mod scheme_tests {
    use super::*;

    #[test]
    fn joining_preserves_prefix_escapes_and_encodes_only_member_keys() {
        assert_eq!(
            join_path("ht://a%20b/", "x%part.txt"),
            "ht://a%20b/x%25part.txt"
        );
        assert_eq!(join_path("ht://", "a b"), "ht://a%20b");
        assert_eq!(join_path("vfs:///", "a%20b"), "vfs:///a%20b");
        assert_eq!(join_path("/", "a b"), "/a b");
    }

    #[test]
    fn double_slash_addresses_adapt_without_changing_keys() {
        assert_eq!(
            datafusion_uri("ht://Work%20Files/a%25.csv"),
            "ht:///Work%20Files/a%25.csv"
        );
        assert_eq!(
            datafusion_uri("ht:///Work%20Files/a%25.csv"),
            "ht:///Work%20Files/a%25.csv"
        );
        assert_eq!(datafusion_uri("local://"), "local:///");
        assert_eq!(datafusion_uri("vfs:///old.csv"), "vfs:///old.csv");
        assert_eq!(datafusion_uri("file:///native.csv"), "file:///native.csv");
        assert_eq!(
            datafusion_uri("https://example.org/file"),
            "https://example.org/file"
        );
        assert_eq!(join_path("ht:///", "a b"), "ht://a%20b");
        assert_eq!(
            join_path("ht:///work%20dir", "a b"),
            "ht://work%20dir/a%20b"
        );
    }

    #[test]
    fn domain_paths_round_trip_literal_escapes_and_unicode() {
        let path = "/space name/文献/doi%3Ax?#.txt";
        assert_eq!(decode_path(&encode_path(path)).unwrap(), path);
        assert_eq!(decode_path("/%2e%2e/x").unwrap(), "/../x");
        for path in [
            "/%",
            "/%GG",
            "/%2Fsecret",
            "/%00",
            "/x?query",
            "/x#fragment",
        ] {
            assert!(decode_path(path).is_err(), "{path}");
        }
    }

    #[test]
    fn domain_names_are_unambiguous_uri_schemes() {
        for name in ["local", "ht", "hpc-b", "hpc.2", "hpc+archive"] {
            assert!(valid_domain_name(name), "{name}");
        }
        for name in [
            "", "vfs", "file", "http", "https", "HT", "1ht", "hpc_b", "../ht", "ht://",
        ] {
            assert!(!valid_domain_name(name), "{name}");
        }
    }

    #[test]
    fn split_scheme_extracts_any_domain_address() {
        assert_eq!(
            split_scheme("hpc-b://share/x.vcf"),
            Some(("hpc-b", "share/x.vcf"))
        );
        assert_eq!(
            split_scheme("vfs:///input.tsv"),
            Some(("vfs", "/input.tsv"))
        );
        assert_eq!(
            split_scheme("local://work/a.tsv"),
            Some(("local", "work/a.tsv"))
        );
        assert_eq!(split_scheme("/plain/path"), None);
        assert_eq!(split_scheme("://x"), None, "empty scheme is not an address");
    }
}
