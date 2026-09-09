//! DAG source nodes for RCSB PDB.
//!
//! - [`search::RcsbSearchNode`] (`source_rcsb_search`) emits result IDs and scores.
//! - [`entry::RcsbEntryNode`] (`source_rcsb_entry`) emits structured entry metadata.
//! - [`polymer_entity::RcsbPolymerEntityNode`] (`source_rcsb_polymer_entity`)
//!   emits sequences, UniProt mappings, organisms, and entity counts.
//! - [`structure::RcsbStructureNode`] (`source_rcsb_structure`) writes a
//!   structure file and emits a `FileRef`.

pub mod entry;
pub mod polymer_entity;
pub mod search;
pub mod structure;
pub mod util;
