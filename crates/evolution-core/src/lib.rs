//! Observation-domain primitives shared by evolution subsystems.
//!
//! This crate intentionally does not know about skills or plugins. It owns
//! canonical observation records, their embeddings and clustering; each
//! evolution subsystem consumes the shared ledger directly.

pub mod cluster;
pub mod dimension;
pub mod error;
pub mod handle;
pub mod hub;
pub mod observation;
pub mod storage;
pub mod worker;

pub use error::Error;
pub use hub::ObservationHub;
pub use observation::{Observation, ObservationInput, ObservationKind, ObservationSource};
pub use storage::{EVOLUTION_DB, ObservationStorage, TursoObservationStorage};
