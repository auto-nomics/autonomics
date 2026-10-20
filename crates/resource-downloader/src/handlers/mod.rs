//! Protocol adapters.
//!
//! The first production adapter is HTTPS. Additional protocol adapters should
//! implement the same manager-level invariants: deny-by-default policy,
//! streaming, size limits, integrity checking, and atomic VFS installation.
#[cfg(feature = "http")]
pub mod http;
