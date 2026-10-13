//! Source shims for dashboard tests: compile the real widget files
//! from `src/widgets/` into the test crate. The app crate is
//! binary-only (no lib target), so `#[path]` is the only way
//! integration tests can reach them. Paths resolve relative to this
//! directory (`tests/widgets/`).

// `evolution_widget/mod.rs` declares `pub mod skill_browser;` /
// `pub mod observation_browser;`, which resolve relative to the mod.rs
// directory both in the src tree and under this `#[path]` inclusion,
// so only popup needs an explicit shim here.

#[path = "../../src/widgets/popup.rs"]
pub mod popup;
#[path = "../../src/widgets/evolution_widget/mod.rs"]
pub mod evolution_widget;
