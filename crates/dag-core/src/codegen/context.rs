//! Types shared between the DAG compiler and per-node codegen implementations.

use crate::dag::NodeId;

// ── target language ────────────────────────────────────────────────────────

/// Which programming language to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodegenTarget {
    R,
    Python,
}

impl std::fmt::Display for CodegenTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::R => f.write_str("R"),
            Self::Python => f.write_str("Python"),
        }
    }
}

// ── per-node compilation context ───────────────────────────────────────────

/// Context passed into each [`crate::registry::NodeFactory`]'s
/// `codegen_r` / `codegen_python` method.
///
/// Holds the variable names feeding each input port, the pre-allocated output
/// variable name, the target language, and a fresh-variable allocator for
/// intermediate temporaries.
pub struct CodegenCtx<'a> {
    /// Variable names feeding each input port (index-aligned to port index).
    pub input_vars: &'a [String],
    /// The variable name the compiler pre-allocated for this node's primary
    /// (port-0) output. Multi-output nodes allocate additional names via
    /// [`Self::fresh_var`].
    pub output_var: &'a str,
    /// Target language.
    pub target: CodegenTarget,
    /// Shared per-compile fresh-name counter.
    fresh: &'a mut usize,
}

impl CodegenCtx<'_> {
    /// Allocate a unique intermediate variable name.
    pub fn fresh_var(&mut self, hint: &str) -> String {
        let name = format!("{}_{}", hint, *self.fresh);
        *self.fresh += 1;
        name
    }
}

/// Construct a `CodegenCtx` — only called by the [`super::compiler::DagCompiler`].
pub(crate) fn make_ctx<'a>(
    input_vars: &'a [String],
    output_var: &'a str,
    target: CodegenTarget,
    fresh: &'a mut usize,
) -> CodegenCtx<'a> {
    CodegenCtx {
        input_vars,
        output_var,
        target,
        fresh,
    }
}

// ── per-node result ────────────────────────────────────────────────────────

/// What a node factory's codegen produces.
pub struct NodeCodegen {
    /// Generated source lines (statements in the target language).
    pub code: Vec<String>,
    /// Variable names assigned to each output port, index-aligned.
    /// For single-output nodes: `vec![output_var.to_string()]`.
    /// Length must equal `factory.ports().output_ports().len()`.
    pub output_vars: Vec<String>,
    /// Additional packages required, beyond the factory's static list.
    pub extra_packages: Vec<String>,
}

impl NodeCodegen {
    /// Convenience: a single-output node that assigns `output_var` and has no
    /// extra packages.
    pub fn simple(code: Vec<String>, output_var: impl Into<String>) -> Self {
        Self {
            code,
            output_vars: vec![output_var.into()],
            extra_packages: vec![],
        }
    }
}

// ── error type ─────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum CodegenError {
    #[error("node kind '{kind}' does not support {target} codegen")]
    NotSupported { kind: String, target: CodegenTarget },
    #[error("failed to deserialize spec for '{kind}': {source}")]
    BadSpec {
        kind: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("DAG topology error: {0}")]
    Topology(String),
    #[error("node kind '{kind}' is not registered")]
    UnknownKind { kind: String },
}

// ── helper: variable-name sanitization ─────────────────────────────────────

/// Sanitize a DAG node id into a valid R/Python identifier.
///
/// Replaces any character outside `[A-Za-z0-9_]` with `_`, and prefixes `n_`
/// if the result starts with a digit (identifiers cannot start with digits).
pub fn sanitize_var_name(id: &str) -> String {
    let mut out: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'n');
        out.insert(1, '_');
    }
    if out.is_empty() {
        out.push_str("n_empty");
    }
    out
}

// ── helper: topo sort on manifest ──────────────────────────────────────────

/// Topologically sort manifest nodes using Kahn's algorithm.
///
/// Returns node ids in dependency order (parents before children).
/// Ties are broken by declaration order in `manifest.nodes` for deterministic
/// output. Returns [`CodegenError::Topology`] on cycle.
pub(crate) fn topo_sort(
    nodes: &[&super::super::dag::history::NodeEntry],
    edges: &[&super::super::dag::history::EdgeEntry],
) -> Result<Vec<NodeId>, CodegenError> {
    use std::collections::{HashMap, HashSet, VecDeque};

    // Index nodes by declaration order for stable tie-breaking.
    let mut order_index: HashMap<&str, usize> = HashMap::new();
    for (i, entry) in nodes.iter().enumerate() {
        order_index.insert(entry.id.as_str(), i);
    }

    // Build adjacency + in-degree.
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for entry in nodes {
        adj.insert(entry.id.as_str(), Vec::new());
        in_degree.insert(entry.id.as_str(), 0);
    }
    for edge in edges {
        // edge: from → to (data flows from `from` to `to`)
        adj.get_mut(edge.from.as_str())
            .unwrap()
            .push(edge.to.as_str());
        *in_degree.get_mut(edge.to.as_str()).unwrap() += 1;
    }

    // Priority queue keyed by declaration order for stable output.
    let mut ready: VecDeque<&str> = nodes
        .iter()
        .filter(|e| *in_degree.get(e.id.as_str()).unwrap_or(&0) == 0)
        .map(|e| e.id.as_str())
        .collect();
    // Sort initial ready set by declaration order.
    let mut ready_vec: Vec<&str> = ready.drain(..).collect();
    ready_vec.sort_by_key(|id| *order_index.get(id).unwrap_or(&usize::MAX));
    ready = VecDeque::from(ready_vec);

    let mut result = Vec::with_capacity(nodes.len());
    let mut visited = HashSet::new();

    while let Some(id) = ready.pop_front() {
        if !visited.insert(id) {
            continue;
        }
        result.push(id.to_string());
        // Collect newly-ready successors, sort by declaration order.
        let mut new_ready: Vec<&str> = Vec::new();
        for &succ in &adj[id] {
            let deg = in_degree.get_mut(succ).unwrap();
            *deg -= 1;
            if *deg == 0 {
                new_ready.push(succ);
            }
        }
        new_ready.sort_by_key(|sid| *order_index.get(sid).unwrap_or(&usize::MAX));
        // Merge into ready queue maintaining sort order.
        // Since we process in declaration order and new_ready is sorted,
        // we can just push them — but to be fully correct we should re-sort.
        // For simplicity, rebuild the queue.
        for r in new_ready {
            ready.push_back(r);
        }
        // Re-sort the entire queue by declaration order for stable output.
        let mut q: Vec<&str> = ready.drain(..).collect();
        q.sort_by_key(|id| *order_index.get(id).unwrap_or(&usize::MAX));
        ready = VecDeque::from(q);
    }

    if result.len() != nodes.len() {
        return Err(CodegenError::Topology(
            "cycle detected in DAG manifest — cannot topologically sort".into(),
        ));
    }

    Ok(result)
}
