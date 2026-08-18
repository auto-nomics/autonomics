//! Per-node metadata, ports, the typed input envelope, and the [`DagNode`] trait.
//!
//! Each node declares a set of **input ports** and **output ports** ([`Port`]). An edge
//! in the DAG connects exactly one upstream output port to one downstream input port and
//! carries exactly one [`DataFrame`]. At execution time the scheduler injects one
//! [`NodeInput`] per connected input port, tagged with the port name so the node knows
//! which slot each DataFrame belongs to.

use arrow_array::Array as _;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::{common::HashMap, prelude::DataFrame};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Serialize, ser::SerializeMap};

use crate::dag::{DagError, graph::PortOutputs};
use crate::value::{FileRef, NodeValue, PortType};

/// Unique identifier for a node in the DAG.
pub type NodeId = String;

/// Numeric identifier for an individual port on a node.
///
/// Ports are indexed sequentially starting from 0. An edge in the DAG
/// connects one `(node, PortId)` pair on the output side to another
/// `(node, PortId)` pair on the input side.
pub type PortId = u8;

/// The default port name used when a single-input/single-output node does not name its
/// ports explicitly.
pub const DEFAULT_PORT: &str = "default";

/// A named, optionally-typed socket on a node.
///
/// `schema` is `None` when the shape of the data is not known at graph-build time
/// (e.g. a source reading a file whose schema is discovered at runtime). When both ends
/// of an edge declare a schema, the engine validates compatibility in [`DAG::validate`].
#[derive(Debug, Clone)]
pub struct Port {
    pub index: PortId,
    pub schema: Option<SchemaRef>,
    pub data_type: PortType,
    pub required: bool,
}

impl Port {
    /// An untyped port (schema discovered at runtime).
    pub fn new(index: PortId, schema: Option<SchemaRef>) -> Self {
        Self {
            index,
            schema,
            data_type: PortType::DataFrame,
            required: true,
        }
    }

    /// A port with a known schema.
    pub fn typed(index: PortId, schema: SchemaRef) -> Self {
        Self {
            index,
            schema: Some(schema),
            data_type: PortType::DataFrame,
            required: true,
        }
    }

    pub fn with_data_type(mut self, data_type: PortType) -> Self {
        self.data_type = data_type;
        self
    }

    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    // /// Convenience: the unnamed default port.
    // #[deprecated]
    // pub fn default_port() -> Self {
    //     Self::new(0, None)
    // }

    pub fn get_code(&self) -> String {
        format!("port_{0}", self.index.clone())
    }
}

/// Serializable description of one column in a port's [`arrow_schema::Schema`].
///
/// `arrow_schema::DataType` is not serde-serializable without an extra feature
/// flag, so we surface the type as its `Debug` string — enough for a caller
/// (e.g. the agent wiring edges) to reason about column shape.
#[derive(Serialize)]
struct PortFieldDto<'a> {
    name: &'a str,
    data_type: String,
    nullable: bool,
}

impl Serialize for Port {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("index", &self.index)?;
        map.serialize_entry("data_type", &self.data_type)?;
        map.serialize_entry("required", &self.required)?;
        match &self.schema {
            // A typed port: emit its columns so consumers can validate edge
            // compatibility without instantiating the node.
            Some(schema) => {
                let fields: Vec<PortFieldDto<'_>> = schema
                    .fields()
                    .iter()
                    .map(|f| PortFieldDto {
                        name: f.name().as_str(),
                        data_type: format!("{:?}", f.data_type()),
                        nullable: f.is_nullable(),
                    })
                    .collect();
                map.serialize_entry("schema", &fields)?;
            }
            // An untyped port (schema discovered at runtime).
            None => map.serialize_entry("schema", &None::<()>)?,
        }
        map.end()
    }
}

/// An ordered collection of [`Port`]s belonging to a single dataflow direction
/// (input or output) on a node.
#[derive(Clone, Debug)]
pub struct Ports {
    ports: HashMap<PortId, Port>,
    /// Whether the declared input ports are exhaustive. If `false`, edges may
    /// target additional, undeclared input ports (variadic inputs); declared
    /// ports remain required, and strict 1:1 still applies to every wired port.
    is_fixed: bool,
}
impl Default for Ports {
    fn default() -> Self {
        Self {
            ports: Default::default(),
            is_fixed: true,
        }
    }
}

impl Ports {
    /// Append a new port with the next sequential index and the given schema.
    pub fn add_port(&mut self, schema: Option<SchemaRef>) {
        let length = self.ports.len();
        self.ports
            .insert(length as PortId, Port::new(length as PortId, schema));
    }

    pub fn add_port_of_type(&mut self, schema: Option<SchemaRef>, data_type: PortType) {
        let length = self.ports.len();
        self.ports.insert(
            length as PortId,
            Port::new(length as PortId, schema).with_data_type(data_type),
        );
    }

    pub fn add_optional_port_of_type(&mut self, data_type: PortType) {
        let length = self.ports.len();
        self.ports.insert(
            length as PortId,
            Port::new(length as PortId, None)
                .with_data_type(data_type)
                .optional(),
        );
    }

    /// Look up a port by its numeric index.
    pub fn get(&self, index: PortId) -> Option<&Port> {
        self.ports.get(&index)
    }

    /// Number of declared ports.
    pub fn len(&self) -> usize {
        self.ports.len()
    }

    /// Returns `true` if no ports are declared.
    pub fn is_empty(&self) -> bool {
        self.ports.is_empty()
    }

    /// Iterate over ports in index order.
    pub fn iter(&self) -> impl Iterator<Item = &Port> {
        let mut keys: Vec<_> = self.ports.keys().copied().collect();
        keys.sort();
        keys.into_iter().filter_map(move |k| self.ports.get(&k))
    }
}

impl Serialize for Ports {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut st = serializer.serialize_struct("Ports", 2)?;
        // Emit ports in index order regardless of HashMap iteration order, so
        // the serialized layout is stable and matches port numbering.
        let ordered: Vec<&Port> = self.iter().collect();
        st.serialize_field("ports", &ordered)?;
        st.serialize_field("is_fixed", &self.is_fixed)?;
        st.end()
    }
}

impl From<Vec<Port>> for Ports {
    fn from(ports: Vec<Port>) -> Self {
        let map = ports.into_iter().map(|p| (p.index, p)).collect();
        Self {
            ports: map,
            is_fixed: true,
        }
    }
}

/// One upstream output injected into a node at execution time, one per connected input
/// port.
#[derive(Debug, Clone)]
pub struct NodeInput {
    /// The consuming node's input **port index** this data arrived on. The node
    /// looks up its inputs by this index to identify which slot each DataFrame
    /// belongs to.
    pub port: PortId,
    pub data: NodeValue,
}

impl NodeInput {
    pub fn new_dataframe(port: PortId, df: DataFrame) -> Self {
        Self {
            port,
            data: NodeValue::DataFrame(df),
        }
    }

    pub fn file(port: PortId, file: FileRef) -> Self {
        Self {
            port,
            data: NodeValue::File(file),
        }
    }

    pub fn dataframe(&self) -> Result<&DataFrame, DagError> {
        self.data.as_dataframe()
    }

    pub fn dataframe_value(&self) -> Result<&DataFrame, DagError> {
        self.dataframe()
    }

    pub fn file_value(&self) -> Result<&FileRef, DagError> {
        self.data.as_file()
    }
}

/// Static per-node port layout: declared input/output ports.
#[derive(Clone, Default, Debug)]
pub struct NodePorts {
    input_ports: Ports,
    output_ports: Ports,
}

impl NodePorts {
    /// A transform node with a single default input port and a single default output
    /// port.
    pub fn new() -> Self {
        Self {
            input_ports: Ports::default(),
            output_ports: Ports::default(),
        }
    }

    /// Set whether the input port count is fixed (default `true`).
    ///
    /// When `false`, the scheduler allows edges to additional, undeclared
    /// input ports. This is useful for variadic nodes that accept a dynamic
    /// number of inputs (e.g. `SqlNode`), and for nodes whose declared ports
    /// are required while extra ports are optional. Declared ports remain
    /// required, and every wired input port is still strict 1:1.
    pub fn set_fixed_input(mut self, is_fixed: bool) -> Self {
        self.input_ports.is_fixed = is_fixed;
        self
    }

    /// Append an output port with the given schema (builder-style).
    pub fn add_output_port(mut self, schema: Option<SchemaRef>) -> Self {
        self.output_ports.add_port(schema);
        self
    }

    pub fn add_output_port_of_type(
        mut self,
        schema: Option<SchemaRef>,
        data_type: PortType,
    ) -> Self {
        self.output_ports.add_port_of_type(schema, data_type);
        self
    }

    /// Append an input port with the given schema (builder-style).
    pub fn add_input_port(mut self, schema: Option<SchemaRef>) -> Self {
        self.input_ports.add_port(schema);
        self
    }

    pub fn add_input_port_of_type(
        mut self,
        schema: Option<SchemaRef>,
        data_type: PortType,
    ) -> Self {
        self.input_ports.add_port_of_type(schema, data_type);
        self
    }

    pub fn add_optional_input_port_of_type(mut self, data_type: PortType) -> Self {
        self.input_ports.add_optional_port_of_type(data_type);
        self
    }

    /// Access the declared input ports.
    pub fn input_ports(&self) -> &Ports {
        &self.input_ports
    }

    /// Access the declared output ports.
    pub fn output_ports(&self) -> &Ports {
        &self.output_ports
    }

    /// Look up a declared input port by index.
    pub fn input_port(&self, index: PortId) -> Option<&Port> {
        self.input_ports.get(index)
    }

    /// Look up a declared output port by index.
    pub fn output_port(&self, index: PortId) -> Option<&Port> {
        self.output_ports.get(index)
    }

    /// Returns `true` if the input port count is fixed (i.e. the scheduler
    /// will validate edge connectivity).
    pub fn is_fixed_input(&self) -> bool {
        self.input_ports.is_fixed
    }
}

impl Serialize for NodePorts {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut st = serializer.serialize_struct("NodePorts", 2)?;
        st.serialize_field("input_ports", &self.input_ports)?;
        st.serialize_field("output_ports", &self.output_ports)?;
        st.end()
    }
}

/// A single unit of work in the DAG.
///
/// `execute` receives one [`NodeInput`] per connected input port (the node identifies each
/// by its [`NodeInput::port`] index) and returns its outputs keyed by **output port name**, so the
/// engine can route each [`DataFrame`] through the correct outgoing edge.
///
/// Implementors must be [`Send`] + [`Sync`] because nodes are executed by an async
/// scheduler that may run them on different tasks.
///
/// - NodeSpec: editable parameters exposured to public during runtime, control customized action of node.
/// - NodePorts: contain input / output port wiring definitions of the node. It is definite during
/// compile time
#[async_trait]
pub trait DagNode: Send + Sync {
    /// Return this node's static metadata (declared ports).
    fn ports(&self) -> &NodePorts;

    /// Run the node's computation.
    ///
    /// Receives one [`NodeInput`] per connected input port and must return a
    /// [`PortOutputs`] map keyed by output port index. The [`NodeReporter`] is
    /// the node's channel for emitting mid-run observations — lifecycle status,
    /// progress, and log lines — back to the scheduler, which forwards them to
    /// external observers (e.g. the `run_dag` tool's live output). A node that
    /// has nothing to report simply ignores it (bind it to `_reporter`); the
    /// scheduler emits the authoritative terminal `Finished` event regardless.
    ///
    /// `ctx` is the framework-injected, immutable engine ingredients. A node
    /// that needs a `SessionContext` (e.g. to register upstream DataFrames as
    /// views and run SQL) obtains a **fresh**, isolated one via
    /// [`NodeCtx::session`](crate::registry::NodeCtx::session)
    /// and never stores one as a field — that is what keeps mutable catalog
    /// state from leaking across runs or between `clone_box` copies of a node.
    async fn execute(
        &mut self,
        ctx: &crate::registry::NodeCtx,
        inputs: &[NodeInput],
        reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError>;

    /// Clone this node into a boxed trait object.
    ///
    /// Required because `Clone` is not object-safe; the DAG stores nodes as
    /// `Box<dyn DagNode>` and needs to duplicate them (e.g. for validation
    /// dry-runs).
    fn clone_box(&self) -> Box<dyn DagNode>;

    /// The kind string identifying this node type (e.g. `"source_file"`,
    /// `"sql"`, `"sink_file"`). This MUST match the [`NodeFactory::kind`] that
    /// builds this node type.
    fn kind(&self) -> &'static str;

    /// Downcast helper for concrete-type introspection (e.g. extracting
    /// sink-specific details at report time).
    fn as_any(&self) -> &dyn std::any::Any;

    /// Optional: the file path this node writes to, if it is a file sink.
    /// Used by the scheduler's run report to surface output locations.
    /// Default `None`; overridden by file sink nodes.
    fn sink_path(&self) -> Option<&str> {
        None
    }

    /// Optional: the artifact path this node produces (e.g. a rendered plot).
    /// Used by the scheduler's run report. Default `None`; overridden by
    /// artifact-producing nodes (e.g. VizNode).
    fn artifact_path(&self) -> Option<&str> {
        None
    }

    // fn write_output(&self, port_id: &str, df: DataFrame) -> Result<(), DagError>;
    // fn get_input(&self, port_id: &str) -> Result<DataFrame, DagError>;
}

impl Clone for Box<dyn DagNode> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

// ============================ string-column helper ============================

/// Extract string values from an Arrow column, accepting **both** physical
/// layouts: `Utf8` (`StringArray`) and `Utf8View` (`StringViewArray`).
///
/// DataFusion ≥42 emits `Utf8View` for SQL output and Parquet reads, while
/// many older code paths produce `Utf8`. A bare
/// `downcast_ref::<StringArray>()` silently returns `None` on a `Utf8View`
/// column, which manifests upstream as a misleading "missing column" error.
/// This helper accepts either layout so node code is immune to the upstream
/// physical representation. Returns `None` only when the column is neither
/// string type.
pub fn string_opt_values(arr: &dyn arrow_array::Array) -> Option<Vec<Option<String>>> {
    use arrow_array::{StringArray, StringViewArray};
    if let Some(a) = arr.as_any().downcast_ref::<StringArray>() {
        Some(
            (0..a.len())
                .map(|i| {
                    if a.is_null(i) {
                        None
                    } else {
                        Some(a.value(i).to_string())
                    }
                })
                .collect(),
        )
    } else {
        arr.as_any().downcast_ref::<StringViewArray>().map(|a| {
            (0..a.len())
                .map(|i| {
                    if a.is_null(i) {
                        None
                    } else {
                        Some(a.value(i).to_string())
                    }
                })
                .collect()
        })
    }
}
