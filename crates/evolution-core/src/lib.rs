//! Storage and routing primitives shared by evolution subsystems.
//!
//! This crate intentionally does not know about skills or plugins. It owns
//! canonical observation records and deterministic audience routing; each
//! evolution subsystem maps a route into its own lifecycle.

pub mod embedding;
pub mod error;
pub mod hub;
pub mod observation;
pub mod routing;

pub use embedding::{
    HashingObservationEmbedder, ObservationEmbedder, SharedObservationEmbedder, VectorMapEmbedder,
    cosine, mean_vector,
};
pub use error::Error;
pub use hub::ObservationHub;
pub use observation::{
    Observation, ObservationInput, ObservationKind, ObservationSource, ObservationStore,
};
pub use routing::{
    ObservationAudience, ObservationRoute, ObservationRouteStatus, ObservationRouteStore,
    RouteDecision, classify_observation,
};
