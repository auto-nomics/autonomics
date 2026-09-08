//! DAG source nodes that pull protein data from the UniProt REST API.
//!
//! - [`UniprotSearchNode`](search::UniprotSearchNode)
//!   (`source_uniprot_search`) — query UniProtKB and emit a table of
//!   entries.
//! - [`UniprotStreamNode`](stream::UniprotStreamNode)
//!   (`source_uniprot_stream`) — bulk-download raw TSV/FASTA/… output to a
//!   file.
//! - [`UniprotIdmapNode`](idmap::UniprotIdmapNode) (`source_uniprot_idmap`)
//!   — map identifiers between databases and emit a table.
//!
//! These are zero-input (idmap: optional input) source nodes. They reuse
//! the [`crate::UniProtClient`]; `DagNode::execute` is async on the
//! engine's tokio runtime.

pub mod idmap;
pub mod search;
pub mod stream;
