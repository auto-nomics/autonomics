//! DAG source nodes for CDC NHANES.
//!
//! - [`files::NhanesFilesNode`] (`source_nhanes_files`) emits the component's
//!   file listing as a table (cycle, topic, hrefs, file stem, …).
//! - [`download::NhanesDownloadNode`] (`source_nhanes_download`) downloads one
//!   validated XPT file and emits a `FileRef`.

pub mod download;
pub mod files;
pub mod util;
