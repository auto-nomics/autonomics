//! Ordered collections of input or output ports.

use arrow_schema::SchemaRef;
use datafusion::common::HashMap;
use serde::Serialize;
use serde::ser::{SerializeStruct, Serializer};

use super::id::PortId;
use super::port::Port;
use crate::value::PortType;

/// An ordered collection of [`Port`]s belonging to one dataflow direction
/// (input or output) on a node.
#[derive(Clone, Debug)]
pub struct Ports {
    ports: HashMap<PortId, Port>,
    /// Whether declared input ports are exhaustive. If `false`, edges may
    /// target additional, undeclared input ports. Declared ports remain
    /// required, and strict 1:1 connectivity still applies to every wired port.
    is_fixed: bool,
}

impl Default for Ports {
    fn default() -> Self {
        Self {
            ports: HashMap::default(),
            is_fixed: true,
        }
    }
}

impl Ports {
    /// Append a DataFrame port with the next sequential index.
    pub fn add_port(&mut self, schema: Option<SchemaRef>) {
        self.add_port_with(Port::new(self.next_index(), schema));
    }

    /// Append a typed-value port with the next sequential index.
    pub fn add_port_of_type(&mut self, schema: Option<SchemaRef>, data_type: PortType) {
        let port = Port::new(self.next_index(), schema).with_data_type(data_type);
        self.add_port_with(port);
    }

    /// Append a typed-value port with a semantic label.
    pub fn add_port_of_type_with_label(
        &mut self,
        schema: Option<SchemaRef>,
        data_type: PortType,
        label: impl Into<String>,
    ) {
        let port = Port::new(self.next_index(), schema)
            .with_data_type(data_type)
            .with_label(label);
        self.add_port_with(port);
    }

    /// Append a typed-value port with a semantic label and format contract.
    pub fn add_port_of_type_with_label_and_format(
        &mut self,
        schema: Option<SchemaRef>,
        data_type: PortType,
        label: impl Into<String>,
        format: impl Into<String>,
    ) {
        let port = Port::new(self.next_index(), schema)
            .with_data_type(data_type)
            .with_label(label)
            .with_format(format);
        self.add_port_with(port);
    }

    /// Append an input port with a primary and alternate format contracts.
    pub fn add_port_of_type_with_accepted_formats(
        &mut self,
        schema: Option<SchemaRef>,
        data_type: PortType,
        label: impl Into<String>,
        format: impl Into<String>,
        accepted_formats: impl IntoIterator<Item = impl Into<String>>,
    ) {
        let port = Port::new(self.next_index(), schema)
            .with_data_type(data_type)
            .with_label(label)
            .with_format(format)
            .with_accepted_formats(accepted_formats);
        self.add_port_with(port);
    }

    /// Append an optional, schema-less typed-value port.
    pub fn add_optional_port_of_type(&mut self, data_type: PortType) {
        let port = Port::new(self.next_index(), None)
            .with_data_type(data_type)
            .optional();
        self.add_port_with(port);
    }

    fn add_port_with(&mut self, port: Port) {
        self.ports.insert(port.index, port);
    }

    fn next_index(&self) -> PortId {
        self.ports.len() as PortId
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
        keys.sort_unstable();
        keys.into_iter().filter_map(move |key| self.ports.get(&key))
    }

    pub(super) fn set_fixed(mut self, is_fixed: bool) -> Self {
        self.is_fixed = is_fixed;
        self
    }

    pub(super) fn is_fixed(&self) -> bool {
        self.is_fixed
    }
}

impl Serialize for Ports {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Ports", 2)?;
        // Emit ports in index order regardless of HashMap iteration order, so
        // the serialized layout is stable and matches port numbering.
        let ordered: Vec<&Port> = self.iter().collect();
        state.serialize_field("ports", &ordered)?;
        state.serialize_field("is_fixed", &self.is_fixed)?;
        state.end()
    }
}

impl From<Vec<Port>> for Ports {
    fn from(ports: Vec<Port>) -> Self {
        let map = ports.into_iter().map(|port| (port.index, port)).collect();
        Self {
            ports: map,
            is_fixed: true,
        }
    }
}
