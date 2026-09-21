//! Registry-agnostic image references for official tool containers.
//!
//! Tool nodes pin the repository and manifest digest, while deployment is free
//! to select a full registry namespace through `AUTONOMICS_IMAGE_PREFIX`.
//! Production always uses GHCR when that variable is not set.

pub const IMAGE_PREFIX_ENV: &str = "AUTONOMICS_IMAGE_PREFIX";
pub const DEFAULT_IMAGE_PREFIX: &str = "ghcr.io/auto-nomics/autonomics";

pub fn registry_image(repository: &str, digest: &str) -> Result<String, String> {
    let prefix = image_prefix_from_env()?;
    image_for_prefix(&prefix, repository, digest)
}

fn image_prefix_from_env() -> Result<String, String> {
    image_prefix_from_value(std::env::var_os(IMAGE_PREFIX_ENV).as_deref())
}

fn image_prefix_from_value(value: Option<&std::ffi::OsStr>) -> Result<String, String> {
    match value {
        Some(value) => normalize_prefix(&value.to_string_lossy()),
        None => Ok(DEFAULT_IMAGE_PREFIX.into()),
    }
}

fn normalize_prefix(value: &str) -> Result<String, String> {
    let prefix = value.trim().trim_end_matches('/');
    if prefix.is_empty() {
        return Err(format!("`{IMAGE_PREFIX_ENV}` cannot be empty"));
    }
    if prefix.contains("://")
        || prefix.contains('\\')
        || prefix.contains('@')
        || prefix.chars().any(char::is_whitespace)
        || prefix.split('/').any(str::is_empty)
    {
        return Err(format!(
            "`{IMAGE_PREFIX_ENV}` must be a registry prefix without a scheme, empty segments, or whitespace, got `{prefix}`"
        ));
    }

    Ok(prefix.to_string())
}

fn image_for_prefix(prefix: &str, repository: &str, digest: &str) -> Result<String, String> {
    let prefix = normalize_prefix(prefix)?;
    if repository.is_empty() || repository.contains(['/', '@', '\\']) {
        return Err("repository must be a single lowercase path component".into());
    }
    if digest.is_empty() {
        return Err("digest cannot be empty".into());
    }
    if digest == format!("sha256:{}", "0".repeat(64)) {
        return Err("placeholder digest must be replaced".into());
    }

    Ok(format!("{prefix}/{repository}@{digest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_an_immutable_ghcr_reference() {
        let image = image_for_prefix(
            "ghcr.io/example/autonomics",
            "lava",
            "sha256:b7dd1d3a3493cc32af2dc286f2aace77036b1fd806bee4f14a4ff67509268be7",
        )
        .unwrap();
        assert_eq!(
            image,
            "ghcr.io/example/autonomics/lava@sha256:b7dd1d3a3493cc32af2dc286f2aace77036b1fd806bee4f14a4ff67509268be7"
        );
    }

    #[test]
    fn falls_back_to_the_default_ghcr_prefix() {
        let prefix = image_prefix_from_value(None).unwrap();
        assert_eq!(prefix, DEFAULT_IMAGE_PREFIX);
        let digest = "sha256:bccbe15b2ec8d079e1bf869c4f06bfe4143642015394453c584dc981e5403fbe";
        assert_eq!(
            image_for_prefix(&prefix, "pyradiomics", digest).unwrap(),
            format!("{DEFAULT_IMAGE_PREFIX}/pyradiomics@{digest}")
        );
    }

    #[test]
    fn normalizes_a_trailing_separator() {
        let prefix =
            image_prefix_from_value(Some(std::ffi::OsStr::new("ghcr.io/example/autonomics/")))
                .unwrap();
        assert_eq!(prefix, "ghcr.io/example/autonomics");
    }

    #[test]
    fn default_prefix_is_ghcr() {
        assert_eq!(DEFAULT_IMAGE_PREFIX, "ghcr.io/auto-nomics/autonomics");
    }

    #[test]
    fn rejects_invalid_registry_prefixes() {
        let error = image_prefix_from_value(Some(std::ffi::OsStr::new("https://ghcr.io/example")))
            .unwrap_err();
        assert!(error.contains("without a scheme"));

        let error =
            image_prefix_from_value(Some(std::ffi::OsStr::new("ghcr.io/example//autonomics")))
                .unwrap_err();
        assert!(error.contains("empty segments"));

        let error = image_prefix_from_value(Some(std::ffi::OsStr::new(""))).unwrap_err();
        assert!(error.contains("cannot be empty"));
    }

    #[test]
    fn rejects_invalid_image_parts() {
        let prefix = "ghcr.io/example/autonomics";
        assert!(
            image_for_prefix(prefix, "nested/lava", "sha256:test")
                .unwrap_err()
                .contains("single lowercase path component")
        );
        assert_eq!(
            image_for_prefix(prefix, "lava", "").unwrap_err(),
            "digest cannot be empty"
        );
        assert_eq!(
            image_for_prefix(
                prefix,
                "timesfm",
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
            )
            .unwrap_err(),
            "placeholder digest must be replaced"
        );
    }
}
