use serde::{Deserialize, Serialize};

use crate::error::{DownloadError, Result};
use crate::uri::ResourceUri;

/// Deny-by-default source policy. A rule matches when all supplied matchers
/// match. An empty `hosts` list is invalid so a rule can never accidentally
/// allow every host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AllowRule {
    pub hosts: Vec<String>,
    #[serde(default)]
    pub path_prefixes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadPolicy {
    #[serde(default)]
    pub allow: Vec<AllowRule>,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: u64,
    #[serde(default = "default_max_redirects")]
    pub max_redirects: usize,
    #[serde(default = "default_destination_prefix")]
    pub destination_prefix: String,
}

pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024 * 1024;
pub const DEFAULT_DESTINATION_PREFIX: &str = "/downloads";

pub const fn default_max_bytes() -> u64 {
    DEFAULT_MAX_BYTES
}

pub fn default_destination_prefix() -> String {
    DEFAULT_DESTINATION_PREFIX.to_string()
}

pub const fn default_max_redirects() -> usize {
    5
}

impl Default for DownloadPolicy {
    fn default() -> Self {
        Self {
            allow: Vec::new(),
            max_bytes: default_max_bytes(),
            max_redirects: default_max_redirects(),
            destination_prefix: default_destination_prefix(),
        }
    }
}

impl DownloadPolicy {
    pub fn public_https() -> Self {
        Self::default()
    }

    pub fn allow_https(host: impl Into<String>, path_prefix: impl Into<String>) -> Self {
        let mut policy = Self::default();
        policy.allow = vec![AllowRule {
            hosts: vec![host.into()],
            path_prefixes: vec![path_prefix.into()],
        }];
        policy
    }

    pub fn validate_rule(&self, rule: usize) -> Result<()> {
        let Some(entry) = self.allow.get(rule) else {
            return Ok(());
        };
        if entry.hosts.is_empty() {
            return Err(DownloadError::Denied {
                resource: format!("policy rule {rule}"),
                message: "contains no hosts".to_string(),
            });
        }
        if entry.hosts.iter().any(|host| {
            host.trim().is_empty()
                || host.starts_with('.')
                || host.ends_with('.')
                || host.contains("://")
                || host.contains("/")
                || (host.parse::<std::net::IpAddr>().is_ok() && !host.contains(':'))
        }) {
            return Err(DownloadError::Denied {
                resource: format!("policy rule {rule}"),
                message: "contains an invalid host".to_string(),
            });
        }
        for prefix in &entry.path_prefixes {
            if !prefix.starts_with('/') {
                return Err(DownloadError::Denied {
                    resource: format!("policy rule {rule}"),
                    message: format!("path prefix `{prefix}` must start with /"),
                });
            }
        }
        Ok(())
    }

    pub fn check_source(&self, uri: &ResourceUri) -> Result<()> {
        for (index, rule) in self.allow.iter().enumerate() {
            self.validate_rule(index)?;
            let host_matches = rule.hosts.iter().any(|allowed| {
                let allowed = allowed.trim();
                uri.host == *allowed
                    || uri.host.ends_with(&format!(".{allowed}"))
                    || uri.authority_key() == *allowed
            });
            if !host_matches {
                continue;
            }
            if rule.path_prefixes.is_empty() || path_prefix_matches(&rule.path_prefixes, &uri.path)
            {
                return Ok(());
            }
        }
        Err(DownloadError::denied(
            uri.display(),
            "host/path is not on the download allowlist",
        ))
    }

    pub fn check_destination(&self, destination: &str) -> Result<()> {
        let prefix = self.destination_prefix.trim_end_matches('/');
        if destination == prefix || destination.starts_with(&format!("{prefix}/")) {
            return Ok(());
        }
        Err(DownloadError::InvalidDestination {
            path: destination.to_string(),
            message: format!("must be under `{}`", self.destination_prefix),
        })
    }

    pub fn check_size(&self, source: &str, actual: Option<u64>) -> Result<()> {
        let Some(actual) = actual else {
            return Ok(());
        };
        if actual > self.max_bytes {
            return Err(DownloadError::TooLarge {
                resource: source.to_string(),
                actual,
                limit: self.max_bytes,
            });
        }
        Ok(())
    }
}

fn path_prefix_matches(prefixes: &[String], path: &str) -> bool {
    prefixes
        .iter()
        .any(|prefix| path == prefix || path.starts_with(prefix.trim_end_matches('/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(input: &str) -> ResourceUri {
        ResourceUri::parse(input).unwrap()
    }

    #[test]
    fn policy_is_deny_by_default_for_sources() {
        let policy = DownloadPolicy::public_https();
        assert!(
            policy
                .check_source(&uri("https://example.com/file"))
                .is_err()
        );
    }

    #[test]
    fn policy_matches_host_subdomains_and_path_prefixes() {
        let policy = DownloadPolicy::allow_https("example.com", "/data/");
        assert!(
            policy
                .check_source(&uri("https://example.com/data/a"))
                .is_ok()
        );
        assert!(
            policy
                .check_source(&uri("https://ftp.example.com/data/a"))
                .is_ok()
        );
        assert!(
            policy
                .check_source(&uri("https://example.com/file"))
                .is_err()
        );
        assert!(
            policy
                .check_source(&uri("https://badexample.com/data/a"))
                .is_err()
        );
    }

    #[test]
    fn destinations_must_stay_under_configured_prefix() {
        let policy = DownloadPolicy::default();
        assert!(policy.check_destination("/downloads/a").is_ok());
        assert!(policy.check_destination("/etc/passwd").is_err());
    }

    #[test]
    fn object_store_profiles_restrict_keys_and_credentials_stay_host_side() {
        let profile = ObjectStoreProfile {
            id: "s3-public".to_string(),
            kind: ObjectStoreKind::S3,
            bucket: "open-data".to_string(),
            endpoint: Some("https://s3.example.test".to_string()),
            region: None,
            access_key_id: Some("host-managed-key".to_string()),
            secret_access_key: Some("host-managed-secret".to_string()),
            allow_prefixes: vec!["/reference/".to_string()],
        };
        assert!(profile.check_key("/reference/hg38.fa.gz").is_ok());
        assert!(profile.check_key("/secret/key").is_err());
        assert!(profile.check_key("reference/no-leading-slash").is_err());
        assert!(profile.check_key("/reference/../secret").is_err());
    }

    #[test]
    fn malformed_rules_are_rejected_instead_of_allow_all() {
        let mut policy = DownloadPolicy::default();
        policy.allow = vec![AllowRule {
            hosts: Vec::new(),
            path_prefixes: Vec::new(),
        }];
        assert!(policy.check_source(&uri("https://example.com/x")).is_err());
    }
}

/// A host-managed object-store source profile. Agents refer to profiles by ID;
/// credentials never appear in model-visible input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectStoreProfile {
    pub id: String,
    pub kind: ObjectStoreKind,
    pub bucket: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,
    #[serde(default)]
    pub allow_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectStoreKind {
    S3,
    Oss,
}

impl ObjectStoreProfile {
    pub fn check_key(&self, key: &str) -> Result<()> {
        if key.is_empty() || !key.starts_with('/') {
            return Err(DownloadError::InvalidSource {
                resource: key.to_string(),
                message: "object key must start with /".to_string(),
            });
        }
        if key.contains("..") || key.contains('\\') {
            return Err(DownloadError::InvalidSource {
                resource: key.to_string(),
                message: "object key contains forbidden traversal characters".to_string(),
            });
        }
        if self.allow_prefixes.iter().any(|prefix| {
            !prefix.starts_with('/') || prefix.contains("..") || prefix.contains('\\')
        }) {
            return Err(DownloadError::Denied {
                resource: format!("object-store profile `{}`", self.id),
                message: "contains an invalid key prefix".to_string(),
            });
        }
        let allowed = self
            .allow_prefixes
            .iter()
            .any(|prefix| key == prefix || key.starts_with(prefix.trim_end_matches('/')));
        if allowed {
            Ok(())
        } else {
            Err(DownloadError::denied(
                format!("object-store profile `{}`", self.id),
                "object key is outside allowed prefixes",
            ))
        }
    }

    #[cfg(all(feature = "object-store", feature = "http"))]
    pub fn build_operator(&self) -> opendal::Result<opendal::Operator> {
        match self.kind {
            ObjectStoreKind::S3 => {
                let mut builder = opendal::services::S3::default().bucket(&self.bucket);
                if let Some(endpoint) = &self.endpoint {
                    builder = builder.endpoint(endpoint);
                }
                if let Some(region) = &self.region {
                    builder = builder.region(region);
                }
                if let (Some(ak), Some(sk)) = (&self.access_key_id, &self.secret_access_key) {
                    builder = builder.access_key_id(ak).secret_access_key(sk);
                }
                Ok(opendal::Operator::new(builder)?.finish())
            }
            ObjectStoreKind::Oss => {
                let mut builder = opendal::services::Oss::default().bucket(&self.bucket);
                if let Some(endpoint) = &self.endpoint {
                    builder = builder.endpoint(endpoint);
                }
                if let (Some(ak), Some(sk)) = (&self.access_key_id, &self.secret_access_key) {
                    builder = builder.access_key_id(ak).access_key_secret(sk);
                }
                Ok(opendal::Operator::new(builder)?.finish())
            }
        }
    }
}
