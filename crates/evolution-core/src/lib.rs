//! Storage and routing primitives shared by evolution subsystems.
//!
//! This crate intentionally does not know about skills or plugins. It owns
//! canonical observation records and deterministic audience routing; each
//! evolution subsystem maps a route into its own lifecycle.

pub mod error;
pub mod observation;
pub mod routing;

pub use error::Error;
pub use observation::{
    Observation, ObservationInput, ObservationKind, ObservationSource, ObservationStore,
};
pub use routing::{
    ObservationAudience, ObservationRoute, ObservationRouteStatus, ObservationRouteStore,
    RouteDecision, classify_observation,
};

pub type Result<T> = std::result::Result<T, Error>;
