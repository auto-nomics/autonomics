use serde::{Deserialize, Serialize};

use crate::policy::{DEFAULT_DESTINATION_PREFIX, DEFAULT_MAX_BYTES, DownloadPolicy};

pub const DEFAULT_TIMEOUT_SECS: u64 = 3600;
pub const DEFAULT_CONCURRENCY: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloaderConfig {
    #[serde(default)]
    pub policy: DownloadPolicy,
    #[serde(default)]
    pub object_store_profiles: Vec<crate::policy::ObjectStoreProfile>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
}

pub const fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

pub const fn default_concurrency() -> usize {
    DEFAULT_CONCURRENCY
}

impl Default for DownloaderConfig {
    fn default() -> Self {
        Self {
            policy: DownloadPolicy {
                max_bytes: DEFAULT_MAX_BYTES,
                max_redirects: 5,
                destination_prefix: DEFAULT_DESTINATION_PREFIX.to_string(),
                allow: Vec::new(),
            },
            object_store_profiles: Vec::new(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            concurrency: DEFAULT_CONCURRENCY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let config = DownloaderConfig::default();
        assert_eq!(config.timeout_secs, 3600);
        assert_eq!(config.concurrency, 4);
        assert_eq!(config.policy.max_bytes, 1024 * 1024 * 1024);
    }
}
