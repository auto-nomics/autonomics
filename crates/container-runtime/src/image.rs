//! Full image references for tool containers.
//!
//! A reference is fully described by its source data — registry host,
//! repository path, and immutable manifest digest — with no deployment-time
//! prefix or environment override. The manifest that pins an image is the
//! single source of truth for its address; moving a deployment to another
//! registry rewrites the address fields while digests survive unchanged,
//! because a manifest digest is a content address and registry-independent.
//!
//! [`ImageRepo`] and [`ManifestDigest`] are the manifest-level building
//! blocks (single repository component, verified digest). The string-based
//! [`registry_image`] is the legacy bridge for the hardcoded wrapper
//! constants and resolves them against the fixed GHCR namespace; it retires
//! with the wrapper migration.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Registry host and namespace the legacy wrapper bridge resolves against.
pub const GHCR_REGISTRY: &str = "ghcr.io";
pub const GHCR_NAMESPACE: &str = "auto-nomics/autonomics";

/// A registry host: lowercase hostname with an optional numeric port
/// (`ghcr.io`, `192.168.10.24:30500`). No scheme, no path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct RegistryHost(String);

impl RegistryHost {
    pub fn new(value: &str) -> Result<Self, String> {
        if value.is_empty() {
            return Err("registry host cannot be empty".into());
        }
        if value.contains("://") {
            return Err(format!(
                "registry host cannot contain a scheme, got `{value}`"
            ));
        }
        let host_port = value.split(':').collect::<Vec<_>>();
        let port_valid = match host_port.as_slice() {
            [host] => host.len() == value.len(),
            [host, port] => {
                !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
                    && !host.is_empty()
            }
            _ => false,
        };
        let valid = port_valid
            && !value.contains(['/', '\\', '@'])
            && value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | ':'));
        if !valid {
            return Err(format!(
                "registry host must be a lowercase host with an optional port, got `{value}`"
            ));
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RegistryHost {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl fmt::Display for RegistryHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One or more lowercase repository path segments
/// (`mtag`, `auto-nomics/autonomics/mtag`). Every segment must be a valid
/// single component; `.` and `..` segments are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct RepositoryPath(String);

impl RepositoryPath {
    pub fn new(value: &str) -> Result<Self, String> {
        let valid = !value.is_empty()
            && !value.starts_with('/')
            && !value.ends_with('/')
            && value.split('/').all(|segment| {
                !segment.is_empty()
                    && segment != "."
                    && segment != ".."
                    && valid_repo_segment(segment)
            });
        if !valid {
            return Err(format!(
                "repository path must be non-empty lowercase segments, got `{value}`"
            ));
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RepositoryPath {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl fmt::Display for RepositoryPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A single lowercase repository component (`"mtag"`, `"ldsc"`).
///
/// The manifest-level `[image]` repo field: one segment, validated at
/// construction so an invalid name cannot travel toward the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct ImageRepo(String);

impl ImageRepo {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.is_empty() {
            return Err("image repo cannot be empty".into());
        }
        if value.contains(['/', '@', '\\', ':']) || !valid_repo_segment(&value) {
            return Err(format!(
                "image repo `{value}` must be a single lowercase path component"
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ImageRepo {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl fmt::Display for ImageRepo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn valid_repo_segment(segment: &str) -> bool {
    segment
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_'))
}

/// An OCI manifest digest in canonical form: `sha256:` + 64 lowercase hex
/// characters.
///
/// Uppercase hex is accepted and normalized (same bytes, same digest), so
/// equality comparisons and inventory generation stay canonical. The
/// all-zeros placeholder is rejected: it marks an unfilled scaffold.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct ManifestDigest(String);

impl ManifestDigest {
    const HEX_LEN: usize = 64;

    pub fn new(value: &str) -> Result<Self, String> {
        let Some(hex) = value.strip_prefix("sha256:") else {
            return Err(format!("digest must start with `sha256:`, got `{value}`"));
        };
        if hex.len() != Self::HEX_LEN || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!(
                "digest must be `sha256:` + {} hex chars, got `{value}`",
                Self::HEX_LEN
            ));
        }
        if hex.bytes().all(|b| b == b'0') {
            return Err("placeholder digest must be replaced".into());
        }
        Ok(Self(format!("sha256:{}", hex.to_ascii_lowercase())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ManifestDigest {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl fmt::Display for ManifestDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A fully resolved image reference: `{registry}/{path}@{digest}`.
///
/// All three parts are validated at construction; [`ImageReference::as_str`]
/// cannot fail. Serializes to that string form, symmetric with
/// [`ImageReference::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct ImageReference {
    registry: RegistryHost,
    path: RepositoryPath,
    digest: ManifestDigest,
}

impl Serialize for ImageReference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.as_str())
    }
}

impl ImageReference {
    pub fn new(registry: &str, path: &str, digest: &str) -> Result<Self, String> {
        Ok(Self {
            registry: RegistryHost::new(registry)?,
            path: RepositoryPath::new(path)?,
            digest: ManifestDigest::new(digest)?,
        })
    }

    pub fn from_parts(
        registry: RegistryHost,
        path: RepositoryPath,
        digest: ManifestDigest,
    ) -> Self {
        Self {
            registry,
            path,
            digest,
        }
    }

    /// Parse `host/path/segments@sha256:...`. The registry host is the part
    /// before the first `/`; there is no docker.io defaulting.
    pub fn parse(value: &str) -> Result<Self, String> {
        let (address, digest) = value
            .rsplit_once('@')
            .ok_or_else(|| format!("image reference must contain `@digest`, got `{value}`"))?;
        let (registry, path) = address
            .split_once('/')
            .ok_or_else(|| format!("image reference must contain a registry host, got `{value}`"))?;
        Self::new(registry, path, digest)
    }

    pub fn registry(&self) -> &RegistryHost {
        &self.registry
    }

    pub fn path(&self) -> &RepositoryPath {
        &self.path
    }

    pub fn digest(&self) -> &ManifestDigest {
        &self.digest
    }

    pub fn as_str(&self) -> String {
        format!(
            "{}/{}@{}",
            self.registry.as_str(),
            self.path.as_str(),
            self.digest.as_str()
        )
    }
}

impl TryFrom<String> for ImageReference {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl fmt::Display for ImageReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}@{}", self.registry, self.path, self.digest)
    }
}

/// Legacy bridge for the hardcoded wrapper constants: `repo` + `digest`
/// resolved against the fixed GHCR namespace. The repository must stay a
/// single segment — wrapper constants are names, not paths. New code builds
/// a full [`ImageReference`] from manifest fields instead.
pub fn registry_image(repository: &str, digest: &str) -> Result<String, String> {
    let repository = ImageRepo::new(repository)?;
    Ok(ImageReference::new(
        GHCR_REGISTRY,
        &format!("{GHCR_NAMESPACE}/{}", repository.as_str()),
        digest,
    )?
    .as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAVA_DIGEST: &str =
        "sha256:b7dd1d3a3493cc32af2dc286f2aace77036b1fd806bee4f14a4ff67509268be7";
    const PLACEHOLDER: &str =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn registry_hosts_accept_hosts_and_ports() {
        assert!(RegistryHost::new("ghcr.io").is_ok());
        assert!(RegistryHost::new("192.168.10.24:30500").is_ok());
        assert!(RegistryHost::new("localhost:5000").is_ok());
        for bad in [
            "",
            "https://ghcr.io",
            "GHCR.io",
            "ghcr.io/auto-nomics",
            "a:b:c",
            "host:",
            "host:port",
            "ghcr.io x",
        ] {
            assert!(RegistryHost::new(bad).is_err(), "`{bad}` must be rejected");
        }
    }

    #[test]
    fn repository_paths_accept_lowercase_segments_only() {
        assert!(RepositoryPath::new("mtag").is_ok());
        assert!(RepositoryPath::new("auto-nomics/autonomics/mtag").is_ok());
        for bad in [
            "",
            "/mtag",
            "mtag/",
            "auto-nomics//mtag",
            "auto-nomics/./mtag",
            "auto-nomics/../mtag",
            "Auto-Nomics/mtag",
        ] {
            assert!(RepositoryPath::new(bad).is_err(), "`{bad}` must be rejected");
        }
    }

    #[test]
    fn image_repo_rejects_casing_whitespace_and_separators() {
        assert!(ImageRepo::new("mtag").is_ok());
        assert!(ImageRepo::new("single-cell-preprocessor").is_ok());
        assert!(ImageRepo::new("pathway.gsea").is_ok());
        for bad in ["MTAG", "mtag x", "mtag:x", "mtag/lava", "mtag@x", ""] {
            assert!(ImageRepo::new(bad).is_err(), "`{bad}` must be rejected");
        }
    }

    #[test]
    fn digest_requires_exact_hex_and_normalizes_case() {
        assert!(ManifestDigest::new(LAVA_DIGEST).is_ok());
        // Same bytes in uppercase normalize to the canonical lowercase form.
        let upper = format!(
            "sha256:{}",
            LAVA_DIGEST.trim_start_matches("sha256:").to_uppercase()
        );
        assert_eq!(ManifestDigest::new(&upper).unwrap().as_str(), LAVA_DIGEST);

        let bad_hex = format!("sha256:{}", "z".repeat(64));
        assert!(ManifestDigest::new(&bad_hex).is_err());
        assert!(ManifestDigest::new("sha256:short").is_err());
        assert!(
            ManifestDigest::new(LAVA_DIGEST.strip_prefix("sha256:").unwrap()).is_err()
        );
        assert!(ManifestDigest::new("").is_err());
        assert_eq!(
            ManifestDigest::new(PLACEHOLDER).unwrap_err(),
            "placeholder digest must be replaced"
        );
    }

    #[test]
    fn references_roundtrip_through_their_string_form() {
        let reference = ImageReference::new(
            "ghcr.io",
            "auto-nomics/autonomics/lava",
            LAVA_DIGEST,
        )
        .unwrap();
        let text = reference.as_str();
        assert_eq!(text, format!("ghcr.io/auto-nomics/autonomics/lava@{LAVA_DIGEST}"));
        assert_eq!(ImageReference::parse(&text).unwrap(), reference);
        assert_eq!(reference.to_string(), text);

        let local = ImageReference::new("192.168.10.24:30500", "atc/smr", LAVA_DIGEST).unwrap();
        assert_eq!(
            local.as_str(),
            format!("192.168.10.24:30500/atc/smr@{LAVA_DIGEST}")
        );

        assert!(ImageReference::parse("ghcr.io/lava").is_err());
        assert!(ImageReference::parse("lava@sha256:x").is_err());
    }

    #[test]
    fn legacy_bridge_resolves_the_fixed_ghcr_namespace() {
        assert_eq!(
            registry_image("lava", LAVA_DIGEST).unwrap(),
            format!("ghcr.io/auto-nomics/autonomics/lava@{LAVA_DIGEST}")
        );
        // The bridge validates through the same newtypes, not a relaxed copy.
        assert!(registry_image("nested/lava", LAVA_DIGEST).is_err());
        assert_eq!(
            registry_image("lava", "").unwrap_err(),
            "digest must start with `sha256:`, got ``"
        );
        assert_eq!(
            registry_image("timesfm", PLACEHOLDER).unwrap_err(),
            "placeholder digest must be replaced"
        );
    }

    #[test]
    fn newtypes_parse_and_render_through_serde() {
        let repo: ImageRepo = serde_json::from_str("\"mtag\"").unwrap();
        assert_eq!(repo.as_str(), "mtag");
        assert!(serde_json::from_str::<ImageRepo>("\"MTAG\"").is_err());

        let digest: ManifestDigest = serde_json::from_str(&format!("\"{LAVA_DIGEST}\"")).unwrap();
        assert_eq!(digest.as_str(), LAVA_DIGEST);
        assert!(
            serde_json::from_str::<ManifestDigest>(&format!("\"{PLACEHOLDER}\"")).is_err(),
            "the scaffold placeholder must fail deserialization"
        );

        let reference_text = format!("ghcr.io/auto-nomics/autonomics/lava@{LAVA_DIGEST}");
        let reference: ImageReference = serde_json::from_str(&format!("\"{reference_text}\""))
            .unwrap();
        assert_eq!(reference.as_str(), reference_text);
        assert_eq!(serde_json::to_string(&reference).unwrap(), format!("\"{reference_text}\""));
    }
}
