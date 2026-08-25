//! Versioned data-package catalog shared by the CLI, VFS, and DAG runtime.
//!
//! Packages are built outside the application, published as immutable object
//! collections, and indexed by a small root catalog. Existing nodes continue
//! to consume `/bundles/<id>` through [`dag_core::DataBundle`].

pub mod config;
pub mod model;
pub mod package;
pub mod publish;
pub mod runtime;
pub mod storage;

pub use config::CatalogConfig;
pub use model::{CatalogEntry, CatalogIndex, DatasetFile, DatasetManifest};
pub use package::{build_package, validate_package};
pub use publish::publish_package;
pub use runtime::{CatalogRuntime, catalog_mount_definitions};
