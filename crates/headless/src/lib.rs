//! Headless (non-interactive) execution of agentik agents.
//!
//! This crate is the UI-free sibling of the TUI: both entry points sit on
//! the same library layer (`runtime::RuntimeHost`, `agentik-core`) and
//! never share UI objects. It provides
//!
//! - [`event`]: the stable external run-event contract (`RunEvent`),
//!   emitted as JSONL on stdout in `--json` mode;
//! - [`processor`]: pluggable output rendering (human-readable vs JSONL)
//!   over the same translated event stream.
//!
//! # stdout discipline
//!
//! Borrowed from codex's `exec`: in human mode the only bytes written to
//! stdout are the final agent message (if any); in `--json` mode stdout is
//! strictly JSONL, one event per line. Everything else — progress, tool
//! output, warnings — goes to the progress stream (stderr). Processors
//! therefore write through injected [`std::io::Write`] targets instead of
//! printing, which also keeps them unit-testable against buffers.

#![deny(clippy::print_stdout)]

pub mod event;
pub mod processor;
