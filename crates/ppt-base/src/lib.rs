//! Slide-deck engine: Slidev-semantics parser, Turso storage, markdown
//! source operations, agent tools, and the export pipeline.
//!
//! Layer map:
//!
//! - [`parser`] — faithful Rust port of Slidev's markdown parser
//!   (`packages/parser/src/core.ts`): line scanner, frontmatter, speaker
//!   notes, titles, image extraction, feature detection, round-trip
//!   stringify/prettify.
//! - [`deck`] — assembles [`ppt_types`] wire structs ([`ppt_types::DeckMeta`],
//!   [`ppt_types::SlideSpec`]) from parsed source, merging headmatter
//!   defaults and computing click plans.
//! - [`operations`] — slide patch/insert/remove/move over canonical
//!   markdown (the Slidev MCP operation model).
//! - [`store`] — Turso persistence for decks/versions/assets/exports.
//! - [`tools`] — agent tool registrations.

pub mod assets;
pub mod export;
pub mod shared;
pub mod tools;
pub mod deck;
pub mod operations;
pub mod parser;
pub mod store;

pub use assets::BlobArea;
pub use error::{Error, Result};
pub use export::{DeckExporter, NullExporter};
pub use shared::{CachedDeck, DeckShared};

pub mod error;

/// Hash a string the same way Slidev does (djb2 over UTF-16 code units,
/// i32 wrapping, base36, 12 chars). Used for slide/deck revisions; only
/// stability per content matters, not cryptographic strength.
pub fn content_revision(s: &str) -> String {
    parser::revision_hash(s.trim())
}
