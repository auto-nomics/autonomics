//! Process-level views over the immutable object-storage catalog.

mod common;
mod hf;
mod model;
mod provider;
mod s3;

pub use hf::HfCatalogService;
pub use model::{CatalogDataset, CatalogRecord, CatalogSearchQuery, CatalogSnapshot};
pub use provider::CatalogServiceTrait;
pub use s3::S3CatalogService;
