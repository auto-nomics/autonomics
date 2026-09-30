//! Source shims for dashboard tests: compile the real widget files
//! from `src/widgets/` into the test crate. The app crate is
//! binary-only (no lib target), so `#[path]` is the only way
//! integration tests can reach them. Paths resolve relative to this
//! directory (`tests/widgets/`).

#[path = "../../src/widgets/popup.rs"]
pub mod popup;
#[path = "../../src/widgets/skill_evolution_widget.rs"]
pub mod skill_evolution_widget;
