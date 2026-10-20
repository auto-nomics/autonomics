//! Runtime file loading for the central downloader.
use std::path::Path;

use crate::config::DownloaderConfig;
use crate::error::{DownloadError, Result};

pub const MANIFEST_FILE_NAME: &str = "resource-downloader.toml";

/// Load `<state_dir>/resource-downloader.toml`.
///
/// A missing file intentionally yields deny-by-default defaults rather than
/// disabling the shared tool: operators can add allow rules without changing
/// runtime code. Parsing errors are hard startup failures so a malformed
/// policy cannot silently broaden access.
pub fn load(state_dir: &Path) -> Result<Option<DownloaderConfig>> {
    let path = state_dir.join(MANIFEST_FILE_NAME);
    match std::fs::read_to_string(&path) {
        Ok(source) => {
            toml::from_str(&source)
                .map(Some)
                .map_err(|error| DownloadError::InvalidSource {
                    resource: path.display().to_string(),
                    message: error.to_string(),
                })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DownloadError::InvalidSource {
            resource: path.display().to_string(),
            message: error.to_string(),
        }),
    }
}

pub fn load_or_default(state_dir: &Path) -> Result<DownloaderConfig> {
    Ok(load(state_dir)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::AllowRule;

    #[test]
    fn missing_manifest_is_deny_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_or_default(dir.path()).unwrap();
        assert!(config.policy.allow.is_empty());
    }

    #[test]
    fn malformed_manifest_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(MANIFEST_FILE_NAME), "policy =").unwrap();
        assert!(load_or_default(dir.path()).is_err());
    }

    #[test]
    fn loads_allow_rules_and_profiles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(MANIFEST_FILE_NAME),
            r#"
[policy]
max_bytes = 123

[[policy.allow]]
hosts = ["example.com"]
path_prefixes = ["/data/"]

[[object_store_profiles]]
id = "public"
kind = "s3"
bucket = "open-data"
allow_prefixes = ["/reference/"]
"#,
        )
        .unwrap();
        let config = load_or_default(dir.path()).unwrap();
        assert_eq!(config.policy.max_bytes, 123);
        assert_eq!(
            config.policy.allow,
            vec![AllowRule {
                hosts: vec!["example.com".to_string()],
                path_prefixes: vec!["/data/".to_string()],
            }]
        );
        assert_eq!(config.object_store_profiles.len(), 1);
    }
}
