//! Versioned data-package catalog shared by the CLI, VFS, and DAG runtime.
//!
//! Packages are built outside the application, published as immutable Hugging
//! Face repositories, and indexed by repository references. Consumers install
//! selected packages into a [`LocalCatalog`] cache; the runtime VFS and DAG
//! bundle registry are generated from that local index.

pub mod config;
pub mod error;
pub mod hf;
pub mod local;
pub mod model;
pub mod package;
pub mod remote;

pub use config::CatalogConfig;
pub use hf::{HfPublishTarget, MultiRepoHfSource, publish_package_to_hf};
pub use local::{LocalCatalog, default_panel_cache_root};
pub use model::{CatalogEntry, CatalogIndex, DatasetFile, DatasetManifest};
pub use package::{build_package, validate_package};
pub use remote::RemoteCatalog;
