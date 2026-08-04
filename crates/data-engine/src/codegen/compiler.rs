//! DAG-level compiler that walks the topological order of a [`DagManifest`]
//! and delegates to each node factory's codegen method.

use std::collections::{HashMap, HashSet};

use crate::dag::graph::DAG;
use crate::dag::history::{DagManifest, EdgeEntry, NodeEntry};
use crate::node_registry::registry::NodeRegistry;

use super::context::{CodegenError, CodegenTarget, make_ctx, sanitize_var_name, topo_sort};

// ── output ──────────────────────────────────────────────────────────────────

/// A fully assembled, ready-to-run script.
#[derive(Debug, Clone)]
pub struct CompiledScript {
    /// Target language.
    pub target: CodegenTarget,
    /// Complete source code (header imports + body).
    pub source: String,
    /// All required packages (deduped, first-seen order).
    pub packages: Vec<String>,
    /// Node kinds that returned `NotSupported` and were emitted as comments.
    pub skipped_nodes: Vec<String>,
    /// Per-node `# NOTE:` lines collected for user awareness.
    pub warnings: Vec<String>,
}

// ── compiler ────────────────────────────────────────────────────────────────

/// Compiles a [`DagManifest`] into R or Python source code by delegating to
/// each registered node factory's codegen method.
pub struct DagCompiler<'a> {
    pub registry: &'a NodeRegistry,
}

impl DagCompiler<'_> {
    /// Compile a manifest into source code.
    pub fn compile(
        &self,
        manifest: &DagManifest,
        target: CodegenTarget,
    ) -> Result<CompiledScript, CodegenError> {
        let node_refs: Vec<&NodeEntry> = manifest.nodes.iter().collect();
        let edge_refs: Vec<&EdgeEntry> = manifest.edges.iter().collect();

        // 1. Topological sort
        let order = topo_sort(&node_refs, &edge_refs)?;

        // 2. Build incoming-edge lookup: (to_node, to_port) → (from_node, from_port)
        let mut incoming: HashMap<(&str, u8), (&str, u8)> = HashMap::new();
        for edge in &manifest.edges {
            incoming.insert((&edge.to, edge.to_port), (&edge.from, edge.from_port));
        }

        // Index nodes by id for quick lookup
        let nodes_by_id: HashMap<&str, &NodeEntry> =
            manifest.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        // 3. Walk in topo order
        let mut port_vars: HashMap<(String, u8), String> = HashMap::new();
        let mut body: Vec<String> = Vec::new();
        let mut all_packages: Vec<String> = Vec::new();
        let mut packages_seen: HashSet<String> = HashSet::new();
        let mut skipped: Vec<String> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        let mut fresh_counter = 0usize;

        for node_id in &order {
            let entry = nodes_by_id.get(node_id.as_str()).unwrap();

            // Resolve the factory
            let factory = self
                .registry
                .get_factory(&entry.kind)
                .map_err(|e| CodegenError::Topology(e.to_string()))?;

            // Determine input variable names
            let ports = factory.ports();
            let input_vars: Vec<String> = if ports.is_fixed_input() {
                // Fixed input: iterate declared input ports
                let n_inputs = ports.input_ports().len();
                (0..n_inputs as u8)
                    .map(
                        |port_idx| match incoming.get(&(entry.id.as_str(), port_idx)) {
                            Some((from_node, from_port)) => port_vars
                                .get(&((*from_node).to_string(), *from_port))
                                .cloned()
                                .unwrap_or_else(|| format!("__missing_input_{port_idx}")),
                            None => format!("__missing_input_{port_idx}"),
                        },
                    )
                    .collect()
            } else {
                // Variadic: collect all incoming edges in declaration order.
                // Port index is implied by edge order (0, 1, 2, ...).
                manifest
                    .edges
                    .iter()
                    .filter(|e| e.to == entry.id)
                    .map(|e| {
                        port_vars
                            .get(&(e.from.clone(), e.from_port))
                            .cloned()
                            .unwrap_or_else(|| "__missing_input".to_string())
                    })
                    .collect()
            };

            // Pre-allocate output variable name
            let output_var = sanitize_var_name(&entry.id);

            // Build context and dispatch
            let mut ctx = make_ctx(&input_vars, &output_var, target, &mut fresh_counter);

            let result = match target {
                CodegenTarget::R => factory.codegen_r(&entry.spec, &mut ctx),
                CodegenTarget::Python => factory.codegen_python(&entry.spec, &mut ctx),
            };

            match result {
                Ok(node_cg) => {
                    // Validate output_vars count matches declared ports
                    let n_outputs = ports.output_ports().len();
                    if node_cg.output_vars.len() != n_outputs {
                        return Err(CodegenError::Topology(format!(
                            "node '{}' (kind '{}') codegen produced {} output_vars but \
                             declares {} output ports",
                            entry.id,
                            entry.kind,
                            node_cg.output_vars.len(),
                            n_outputs
                        )));
                    }

                    // Register output variable names for downstream nodes
                    for (port_idx, var) in node_cg.output_vars.iter().enumerate() {
                        port_vars.insert((entry.id.clone(), port_idx as u8), var.clone());
                    }

                    // Section header
                    body.push(String::new());
                    body.push(format!(
                        "# ── {}: {} ──────────────────────────────────",
                        entry.id, entry.kind
                    ));

                    // Collect # NOTE: warnings
                    for line in &node_cg.code {
                        if line.trim_start().starts_with("# NOTE:") {
                            warnings.push(line.clone());
                        }
                    }
                    body.extend(node_cg.code);

                    // Collect packages
                    for pkg in factory.r_packages() {
                        if packages_seen.insert(pkg.clone()) {
                            all_packages.push(pkg);
                        }
                    }
                    for pkg in node_cg.extra_packages {
                        if packages_seen.insert(pkg.clone()) {
                            all_packages.push(pkg);
                        }
                    }
                }
                Err(CodegenError::NotSupported { kind, .. }) => {
                    skipped.push(kind.clone());
                    body.push(String::new());
                    body.push(format!(
                        "# NOTE: node '{}' (kind '{}') has no {} codegen — skipped",
                        entry.id, entry.kind, target
                    ));
                    // Register placeholder so downstream references resolve
                    port_vars.insert((entry.id.clone(), 0), output_var);
                }
                Err(e) => return Err(e),
            }
        }

        // 4. Assemble header + body
        let header = build_header(&all_packages, target);
        let source = format!("{header}\n{}", body.join("\n"));

        Ok(CompiledScript {
            target,
            source,
            packages: all_packages,
            skipped_nodes: skipped,
            warnings,
        })
    }

    /// Convenience: compile the live DAG's current manifest.
    pub fn compile_dag(
        &self,
        dag: &DAG,
        target: CodegenTarget,
    ) -> Result<CompiledScript, CodegenError> {
        self.compile(&dag.to_manifest(), target)
    }
}

fn build_header(packages: &[String], target: CodegenTarget) -> String {
    match target {
        CodegenTarget::R => {
            let mut h = String::from("# Generated by autonomics DAG compiler\n\n");
            for pkg in packages {
                h.push_str(&format!("library({pkg})\n"));
            }
            if !packages.contains(&"data.table".to_string()) {
                h.push_str("library(data.table)\n");
            }
            h
        }
        CodegenTarget::Python => {
            let mut h = String::from("# Generated by autonomics DAG compiler\n\n");
            for pkg in packages {
                h.push_str(&format!("import {pkg}\n"));
            }
            h.push_str("import pandas as pd\n");
            h
        }
    }
}
