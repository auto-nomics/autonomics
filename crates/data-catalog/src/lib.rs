//! Versioned data-package catalog shared by the CLI, VFS, and DAG runtime.
//!
//! Packages are built outside the application, published as immutable object
//! collections, and indexed by a small remote catalog. Consumers install
//! selected packages into a [`LocalCatalog`] cache; the runtime VFS and DAG
//! bundle registry are generated from that local index.

pub mod config;
pub mod error;
pub mod local;
pub mod model;
pub mod package;
pub mod publish;
pub mod remote;
pub mod storage;

pub use config::CatalogConfig;
pub use local::LocalCatalog;
pub use model::{CatalogEntry, CatalogIndex, DatasetFile, DatasetManifest};
pub use package::{build_package, validate_package};
pub use publish::publish_package;
pub use remote::RemoteCatalog;
