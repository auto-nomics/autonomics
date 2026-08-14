//! Storage backend abstraction: declarative configuration that builds
//! `opendal::Operator` instances.
//!
//! A [`StorageConfig`] describes *how* to connect to a storage backend (Local
//! filesystem, S3, ...). [`StorageConfig::build`] turns it into an
//! [`opendal::Operator`], the unified handle all data I/O flows through.
//!
//! The catalog holds named backends registered at bootstrap. Each
//! `ResourceAddress::Storage` references a backend by name + a path within
//! it, so credentials live on the backend (runtime-only, never persisted)
//! while resource entries stay declarative.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use opendal::Operator;
use serde::{Deserialize, Serialize};

use crate::error::{ResourceError, Result};

// ── StorageConfig ────────────────────────────────────────────────────

/// Declarative description of a storage backend.
///
/// Credential fields are `Option<String>`: `None` means "defer to the opendal
/// default credential chain" (env vars, IAM role, ...). Backends are
/// registered at runtime and never persisted to the manifest — only the
/// backend *name* is stored on each resource entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum StorageConfig {
    /// Local filesystem, rooted at `root`.
    Local { root: String },
    /// AWS S3 or any S3-compatible store (set `endpoint` for MinIO/Garage).
    S3 {
        bucket: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_key_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_access_key: Option<String>,
    },
    /// Aliyun OSS.
    Oss {
        bucket: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_key_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_access_key: Option<String>,
    },
}

impl StorageConfig {
    /// A Local backend rooted at `root`.
    pub fn local(root: impl Into<String>) -> Self {
        StorageConfig::Local { root: root.into() }
    }

    /// An S3 backend with explicit credentials.
    pub fn s3(
        bucket: impl Into<String>,
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Self {
        StorageConfig::S3 {
            bucket: bucket.into(),
            endpoint: None,
            region: Some(region.into()),
            access_key_id: Some(access_key_id.into()),
            secret_access_key: Some(secret_access_key.into()),
        }
    }

    /// An S3-compatible backend (MinIO, Garage, ...) with a custom endpoint.
    pub fn s3_compatible(
        bucket: impl Into<String>,
        endpoint: impl Into<String>,
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Self {
        StorageConfig::S3 {
            bucket: bucket.into(),
            endpoint: Some(endpoint.into()),
            region: Some(region.into()),
            access_key_id: Some(access_key_id.into()),
            secret_access_key: Some(secret_access_key.into()),
        }
    }

    /// An Aliyun OSS backend.
    pub fn oss(
        bucket: impl Into<String>,
        endpoint: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Self {
        StorageConfig::Oss {
            bucket: bucket.into(),
            endpoint: Some(endpoint.into()),
            access_key_id: Some(access_key_id.into()),
            secret_access_key: Some(secret_access_key.into()),
        }
    }

    /// Build an [`opendal::Operator`] from this configuration.
    pub fn build(&self) -> Result<Operator> {
        match self {
            StorageConfig::Local { root } => {
                let builder = opendal::services::Fs::default().root(root);
                let op = Operator::new(builder)
                    .map_err(|e| ResourceError::Storage(format!("build Local operator: {e}")))?
                    .finish();
                Ok(op)
            }
            StorageConfig::S3 {
                bucket,
                endpoint,
                region,
                access_key_id,
                secret_access_key,
            } => {
                let mut builder = opendal::services::S3::default().bucket(bucket);
                if let Some(ep) = endpoint {
                    builder = builder.endpoint(ep);
                }
                if let Some(r) = region {
                    builder = builder.region(r);
                }
                if let (Some(ak), Some(sk)) = (access_key_id, secret_access_key) {
                    builder = builder.access_key_id(ak).secret_access_key(sk);
                }
                let op = Operator::new(builder)
                    .map_err(|e| ResourceError::Storage(format!("build S3 operator: {e}")))?
                    .finish();
                Ok(op)
            }
            StorageConfig::Oss {
                bucket,
                endpoint,
                access_key_id,
                secret_access_key,
            } => {
                let mut builder = opendal::services::Oss::default().bucket(bucket);
                if let Some(ep) = endpoint {
                    builder = builder.endpoint(ep);
                }
                if let (Some(ak), Some(sk)) = (access_key_id, secret_access_key) {
                    builder = builder.access_key_id(ak).access_key_secret(sk);
                }
                let op = Operator::new(builder)
                    .map_err(|e| ResourceError::Storage(format!("build OSS operator: {e}")))?
                    .finish();
                Ok(op)
            }
        }
    }

    /// Returns the local filesystem root when this is a `Local` backend.
    /// Used by archive logic to derive concrete filesystem paths.
    pub fn local_root(&self) -> Option<&str> {
        match self {
            StorageConfig::Local { root } => Some(root),
            _ => None,
        }
    }

    /// DataFusion object-store URL prefix for this backend.
    ///
    /// Local backends map to `file://`; S3/S3-compatible backends map to
    /// `s3://<bucket>`. The engine registers the built `Operator` against
    /// this prefix, and `ListingTable` URLs use [`Self::listing_url`].
    pub fn object_store_url(&self) -> String {
        match self {
            StorageConfig::Local { .. } => "file://".to_string(),
            StorageConfig::S3 { bucket, .. } => format!("s3://{bucket}"),
            StorageConfig::Oss { bucket, .. } => format!("oss://{bucket}"),
        }
    }

    /// Compose a DataFusion `ListingTable` URL for `path` within this backend.
    pub fn listing_url(&self, path: &str) -> String {
        let prefix = self.object_store_url();
        if path.starts_with('/') {
            format!("{prefix}{path}")
        } else {
            format!("{prefix}/{path}")
        }
    }
}

// ── StorageBackend ───────────────────────────────────────────────────

/// A registered named backend: its declarative config plus the built operator.
///
/// The config is retained so callers (archive, drift) can inspect whether a
/// backend is local without re-parsing.
#[derive(Debug, Clone)]
pub struct StorageBackend {
    pub config: StorageConfig,
    pub operator: Operator,
}

impl StorageBackend {
    pub fn new(config: StorageConfig) -> Result<Self> {
        let operator = config.build()?;
        Ok(Self { config, operator })
    }
}

// ── StorageRef ───────────────────────────────────────────────────────

/// A resolved storage reference: the operator to perform I/O through, plus
/// the path within it.
///
/// Returned by [`crate::catalog::ResourceCatalog::resolve_storage`].
/// Callers use `ref.operator.read(&ref.path)`, `.list(&ref.path)`, etc.
#[derive(Debug, Clone)]
pub struct StorageRef {
    pub operator: Operator,
    pub path: String,
}

// ── BackendRegistry ──────────────────────────────────────────────────

/// Named backend store, held inside the catalog behind an `Arc<RwLock<>>`.
#[derive(Debug, Default)]
pub struct BackendRegistry {
    backends: HashMap<String, StorageBackend>,
}

impl BackendRegistry {
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
        }
    }

    /// Register (or replace) a named backend. Builds the operator eagerly.
    pub fn register(&mut self, name: &str, config: StorageConfig) -> Result<()> {
        let backend = StorageBackend::new(config)?;
        self.backends.insert(name.to_string(), backend);
        Ok(())
    }

    /// Look up a backend by name.
    pub fn get(&self, name: &str) -> Option<&StorageBackend> {
        self.backends.get(name)
    }

    /// All registered backend names.
    pub fn names(&self) -> Vec<&str> {
        self.backends.keys().map(|s| s.as_str()).collect()
    }
}

/// Type alias for the shared backend registry used by `ResourceCatalog`.
pub type SharedBackendRegistry = Arc<RwLock<BackendRegistry>>;
