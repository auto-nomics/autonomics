//! Markdown parsing helpers.
//!
//! The wiki stores plain standard Markdown. Only the link syntax is
//! interpreted, and only to populate the link index — rendering never
//! rewrites the document body.

mod parse;

pub use parse::{ExtractedLink, LinkKind, extract_links, normalize_path};
