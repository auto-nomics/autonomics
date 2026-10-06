//! Graph validation: cycles, port wiring, payload types, schemas, and
//! path-aliasing hazards.

use std::collections::HashSet;

use petgraph::algo::is_cyclic_directed;
use petgraph::visit::EdgeRef;

use super::{DAG, Result};
use crate::dag::NodeId;
use crate::dag::error::DagError;
use crate::value::PortType;

impl DAG {
    /// Validate the graph: cycles, port wiring, payload types, and schemas.
    ///
    /// Checks (in order):
    /// 1. No cycles.
    /// 2. Every edge references an existing output port, and — for fixed-input
    ///    nodes — an existing input port (variadic nodes accept undeclared
    ///    input ports).
    /// 3. The default-port `add_edge` form was only used on single-port nodes.
    /// 4. Each input port has at most one incoming edge (strict 1:1).
    /// 5. Every declared input port has exactly one incoming edge.
    /// 6. Connected payload types are compatible.
    /// 7. Where both endpoints are DataFrame ports with schemas, the output
    ///    schema covers the input schema's required fields with compatible types.
    pub fn validate(&self) -> Result<()> {
        if is_cyclic_directed(&self.graph) {
            return Err(DagError::Cycle(self.cycle_node_names()));
        }
        self.validate_port_wiring()?;
        self.validate_schemas()?;
        self.validate_path_dependencies()?;
        Ok(())
    }

    /// Reject `file_reference`-style path reads that alias a file another node
    /// in this DAG declares as its output. The port graph cannot order such a
    /// pair, so the read races the write: it fails with a missing-file error,
    /// or silently reads a stale file left by an earlier run.
    fn validate_path_dependencies(&self) -> Result<()> {
        // Paths this DAG's nodes write, from the static sink/artifact hooks.
        let mut writers: Vec<(String, &str)> = Vec::new();
        for (id, node) in &self.nodes {
            if let Some(path) = node.sink_path() {
                writers.push((canonical_file_path(path).to_string(), id));
            }
            if let Some(path) = node.artifact_path() {
                writers.push((canonical_file_path(path).to_string(), id));
            }
        }
        if writers.is_empty() {
            return Ok(());
        }
        for (id, node) in &self.nodes {
            for path in node.referenced_file_paths() {
                if let Some((_, writer)) = writers
                    .iter()
                    .find(|(written, _)| *written == canonical_file_path(&path))
                {
                    return Err(DagError::Schedule(format!(
                        "node `{id}` references file `{path}` by path, but node `{writer}` \
                         writes that same file in this DAG; connect the writer's output \
                         port to the reader instead — path references carry no ordering \
                         and would race the write"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Port existence, default-edge disambiguation, strict-1:1, and completeness.
    pub(super) fn validate_port_wiring(&self) -> Result<()> {
        // (node, port) pairs that have at least one incoming edge.
        let mut connected: HashSet<(NodeId, u8)> = HashSet::new();

        for edge in self.graph.edge_references() {
            let from = self.graph[edge.source()].clone();
            let to = self.graph[edge.target()].clone();
            let label = edge.weight();
            let from_ports = self.nodes[&from].ports();
            let to_ports = self.nodes[&to].ports();

            // Port existence.
            if from_ports.output_port(label.from_port).is_none() {
                return Err(DagError::PortNotFound {
                    node: from,
                    port: label.from_port,
                    direction: "output",
                });
            }

            if to_ports.is_fixed_input() && to_ports.input_port(label.to_port).is_none() {
                return Err(DagError::PortNotFound {
                    node: to.clone(),
                    port: label.to_port,
                    direction: "input",
                });
            }

            // Strict 1:1 on the input port (shared with add_edge).
            if !connected.insert((to.clone(), label.to_port)) {
                return Err(DagError::PortOverconnected {
                    node: to,
                    port: label.to_port,
                });
            }
        }

        // Completeness: every required declared input port must have an edge.
        for id in self.nodes.keys() {
            let meta = self.nodes[id].ports();
            for port in meta.input_ports().iter() {
                if port.required && !connected.contains(&((*id).clone(), port.index)) {
                    return Err(DagError::PortDisconnected {
                        node: id.clone(),
                        port: port.index,
                    });
                }
            }
        }
        Ok(())
    }

    /// Schema compatibility between connected ports (skipped when either side's
    /// schema is `None`). Iterates every edge and delegates the per-edge check
    /// to [`Self::validate_edge_schema`], which is also used at `add_edge` time.
    fn validate_schemas(&self) -> Result<()> {
        for edge in self.graph.edge_references() {
            let from = &self.graph[edge.source()];
            let to = &self.graph[edge.target()];
            let label = edge.weight();
            self.validate_edge_schema(from, label.from_port, to, label.to_port)?;
        }
        Ok(())
    }

    /// Validate payload-type and schema compatibility for a single edge.
    pub(super) fn validate_edge_schema(
        &self,
        from: &str,
        from_port: u8,
        to: &str,
        to_port: u8,
    ) -> Result<()> {
        let (Some(from_node), Some(to_node)) = (self.nodes.get(from), self.nodes.get(to)) else {
            return Ok(());
        };
        let from_port_field = from_node.ports().output_port(from_port);
        let to_port_field = to_node.ports().input_port(to_port);
        let (Some(fp), Some(tp)) = (from_port_field, to_port_field) else {
            return Ok(());
        };
        if !fp.data_type.accepts(tp.data_type) {
            return Err(DagError::PortTypeMismatch {
                from_node: from.to_string(),
                from_port,
                to_node: to.to_string(),
                to_port,
                expected: tp.data_type.to_string(),
                actual: fp.data_type.to_string(),
            });
        }
        if !tp.accepts_format(fp.format.as_deref()) {
            return Err(DagError::PortFormatMismatch {
                from_node: from.to_string(),
                from_port,
                to_node: to.to_string(),
                to_port,
                expected: tp.format.clone().unwrap_or_else(|| "unspecified".into()),
                actual: fp.format.clone().unwrap_or_else(|| "unspecified".into()),
            });
        }
        let (PortType::DataFrame, PortType::DataFrame, Some(out_schema), Some(in_schema)) = (
            fp.data_type,
            tp.data_type,
            fp.schema.as_ref(),
            tp.schema.as_ref(),
        ) else {
            return Ok(());
        };
        if let Err(reason) = schema_compatible(out_schema, in_schema) {
            return Err(DagError::SchemaMismatch {
                from_node: from.to_string(),
                from_port,
                to_node: to.to_string(),
                to_port,
                reason,
            });
        }
        Ok(())
    }
}

/// Check that every field required by `input` is present in `output` with a
/// compatible type.
///
/// Compatibility rule: the output schema must contain, by name, every field the
/// input schema declares, and the types must match exactly. (Stricter than
/// "subtype"; deliberately conservative — if a transform needs looser rules it
/// can leave the port schema `None`.)
pub(super) fn schema_compatible(
    output: &arrow_schema::SchemaRef,
    input: &arrow_schema::SchemaRef,
) -> std::result::Result<(), String> {
    use std::collections::HashMap as StdHashMap;
    let out_fields: StdHashMap<&str, &arrow_schema::Field> = output
        .fields()
        .iter()
        .map(|f| (f.name().as_str(), f.as_ref()))
        .collect();
    for in_field in input.fields() {
        match out_fields.get(in_field.name().as_str()) {
            None => {
                return Err(format!(
                    "input requires column `{}` which is absent from output",
                    in_field.name()
                ));
            }
            Some(out_field) if out_field.data_type() != in_field.data_type() => {
                return Err(format!(
                    "column `{}` type mismatch: output {:?} vs input {:?}",
                    in_field.name(),
                    out_field.data_type(),
                    in_field.data_type()
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Canonical spelling for path-dependency comparison: `file://` prefixes are
/// dropped and `//`-prefixed paths collapse, mirroring `file_reference`'s own
/// local-path resolution. Everything else compares as written.
fn canonical_file_path(path: &str) -> &str {
    path.strip_prefix("file://")
        .unwrap_or(path)
        .trim_start_matches("//")
}
