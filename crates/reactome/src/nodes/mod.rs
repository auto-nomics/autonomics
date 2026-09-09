//! DAG source nodes that pull pathway data from the Reactome API.
//!
//! - [`ReactomePathwaysNode`](pathways::ReactomePathwaysNode)
//!   (`source_reactome_pathways`) — top-level pathways for a species.
//! - [`ReactomeMappingNode`](mapping::ReactomeMappingNode)
//!   (`source_reactome_mapping`) — map an external identifier to pathways.
//! - [`ReactomeAnalysisNode`](analysis::ReactomeAnalysisNode)
//!   (`source_reactome_analysis`) — pathway over-representation analysis.
//! - [`ReactomeParticipantsNode`](participants::ReactomeParticipantsNode)
//!   (`source_reactome_participants`) — participants of a pathway.
//!
//! These are zero-input source nodes. They reuse the
//! [`crate::ReactomeClient`]; `DagNode::execute` is async on the engine's
//! tokio runtime.

pub mod analysis;
pub mod mapping;
pub mod participants;
pub mod pathways;
