//! DAG reverse-compilation — emits equivalent R / Python scripts from the
//! same typed DAG the Rust engine executes.
//!
//! See `docs/dag-codegen-design.md` for the full design document.

pub mod compiler;
pub mod context;
pub mod helpers;

pub use compiler::{CompiledScript, DagCompiler};
pub use context::{CodegenCtx, CodegenError, CodegenTarget, NodeCodegen};
