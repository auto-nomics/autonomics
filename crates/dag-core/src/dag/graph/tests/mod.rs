//! Tests for the graph module, split by concern. Shared fixtures (node
//! payloads, the recording executor, DAG builders) live in [`common`].

mod common;
mod fanout_tests;
mod incremental_tests;
mod mutation_tests;
mod report_tests;
mod scheduler_tests;
mod streaming_tests;
mod topology_tests;
mod validation_tests;
