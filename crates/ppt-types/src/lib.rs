//! Core data types for the slide-deck engine.
//!
//! This crate is intentionally free of I/O and external API dependencies
//! (mirroring [`writing-types`] for the LaTeX system). It defines:
//!
//! - [`model`] — [`DeckMeta`] / [`SlideSpec`] / [`SlideFrontmatter`], the
//!   wire structs the GUI and agent tools consume directly (serde camelCase).
//! - [`clicks`] — the progressive-reveal model ported from Slidev's
//!   `useClicks.ts`: a pure integer algebra computing per-element
//!   `start/end/max` states and the slide's total click count.
//! - [`layout`] — the layout vocabulary (Slidev's built-ins minus
//!   app-specific ones, plus biomedical additions).
//! - [`aspect`] / [`range`] — small parsers (`"16/9"`, `"1,4-5"`).
//!
//! Keeping the types in a leaf crate prevents circular dependencies:
//! `ppt-base` can depend on `ppt-types` without pulling in storage or
//! browser code, and the GUI consumes the serde JSON shape directly.
//!
//! [`writing-types`]: <https://docs.rs/writing-types>

pub mod aspect;
pub mod clicks;
pub mod layout;
pub mod model;
pub mod range;

pub use aspect::{AspectRatio, ParseAspectError};
pub use clicks::{
    AtValue, ClickDirective, ClickPlan, ElementClickState, NormalizedAt, SingleAt, compute_plan,
    normalize_at,
};
pub use layout::Layout;
pub use model::{ColorSchema, DeckFeatures, DeckMeta, SlideFrontmatter, SlideSpec};
pub use range::{ParseRangeError, parse_range_pages};

/// Default canvas width in px (Slidev's `canvasWidth` headmatter default).
pub const DEFAULT_CANVAS_WIDTH: u32 = 980;

/// Generate a new deck ID (`deck-` + short random suffix).
pub fn new_deck_id() -> String {
    let id = uuid::Uuid::new_v4();
    format!("deck-{}", id.simple())
}

// uuid is only needed for ID generation; keep it out of the dependency list
// of the published types by declaring it here as a hidden dep is not possible,
// so it is declared in Cargo.toml but not re-exported.
