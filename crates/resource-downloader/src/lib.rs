//! Central resource downloader for Autonomics.
//!
//! The crate separates three concerns:
//!
//! - [`policy`] — deny-by-default source/destination/size rules;
//! - [`manager`] — transfer execution, redirects, progress, integrity, and
//!   atomic installation through the VFS;
//! - [`tools`] — the `resource_download` Agent tool.
//!
//! Current production support is HTTP/HTTPS with deny-by-default host policy. The manager and policy interfaces are
//! intentionally protocol-shaped so FTP/SFTP/object-store adapters can be
//! added without changing the Agent-facing contract.

pub mod config;
pub mod error;
pub mod handlers;
pub mod integrity;
pub mod manager;
pub mod policy;
pub mod progress;
pub mod runtime;
pub mod tools;
pub mod uri;

pub use config::DownloaderConfig;
pub use manager::{DownloadManager, DownloadReceipt, DownloadRequest};
pub use policy::{AllowRule, DownloadPolicy};
