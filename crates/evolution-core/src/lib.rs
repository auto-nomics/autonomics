//! Observation-domain primitives shared by evolution subsystems.
//!
//! This crate intentionally does not know about skills or plugins. It owns
//! canonical observation records, their embeddings and clustering, and
//! deterministic audience routing; each evolution subsystem maps a route
//! into its own lifecycle.

pub mod cluster;
pub mod dimension;
pub mod error;
pub mod hub;
pub mod observation;
pub mod routing;
pub mod storage;

pub use error::Error;
pub use hub::ObservationHub;
pub use observation::{Observation, ObservationInput, ObservationKind, ObservationSource};
pub use routing::{
    ObservationAudience, ObservationRoute, ObservationRouteStatus, ObservationRouteStore,
    RouteDecision, classify_observation,
};
pub use storage::{EVOLUTION_DB, ObservationStorage, TursoObservationStorage};
