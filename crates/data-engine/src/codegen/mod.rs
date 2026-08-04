//! DAG reverse-compilation — emits equivalent R / Python scripts from the
//! same typed DAG the Rust engine executes.
//!
//! See `docs/dag-codegen-design.md` for the full design document.
//!
//! ## Quick start
//!
//! ```no_run
//! use data_engine::codegen::{DagCompiler, CodegenTarget};
//!
//! # let registry: data_engine::node_registry::registry::NodeRegistry = unimplemented!();
//! let manifest = data_engine::dag::history::DagManifest::from_parts(vec![], vec![]);
//! let compiler = DagCompiler { registry: &registry };
//! let script = compiler.compile(&manifest, CodegenTarget::R).unwrap();
//! println!("{}", script.source);
//! ```

pub mod compiler;
pub mod context;
pub mod helpers;

pub use compiler::{CompiledScript, DagCompiler};
pub use context::{CodegenCtx, CodegenError, CodegenTarget, NodeCodegen};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod xval_tests;
