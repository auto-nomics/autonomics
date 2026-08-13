//! Data model — workflow manifests, skills, tools, ports, snapshots.
//!
//! Everything in this module is plain serde, no I/O, no async. Persistence
//! lives in [`crate::store`].

pub mod manifest;
pub mod port;
pub mod skill;
pub mod snapshot;
pub mod tool;

pub use manifest::{CURRENT_SCHEMA_VERSION, EdgeEntry, NodeEntry, Viewport, WorkflowManifest};
pub use port::PortSpec;
pub use skill::{Skill, SkillInfo};
pub use snapshot::SnapshotInfo;
pub use tool::{NodeKindInfo, Tool};
