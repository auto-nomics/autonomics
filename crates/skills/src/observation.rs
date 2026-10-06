//! Compatibility facade for shared evolution-core observations.
//!
//! Skill code and external callers should continue importing these types
//! from `skills`; evolution-core owns the canonical implementation.

pub use evolution_core::observation::*;
