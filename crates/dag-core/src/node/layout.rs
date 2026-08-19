//! Static input/output port layout for node implementations.

use arrow_schema::SchemaRef;
use serde::Serialize;
use serde::ser::{SerializeStruct, Serializer};

use super::id::PortId;
use super::port::Port;
use super::port_set::Ports;
use crate::value::PortType;

/// Static per-node port layout: declared input and output ports.
#[derive(Clone, Default, Debug)]
pub struct NodePorts {
    input_ports: Ports,
    output_ports: Ports,
}

impl NodePorts {
    /// An empty port layout for a node that declares its ports explicitly.
    pub fn new() -> Self {
        Self {
            input_ports: Ports::default(),
            output_ports: Ports::default(),
        }
    }

    /// Set whether the input port count is fixed (the default).
    ///
    /// When `false`, the scheduler allows edges to additional, undeclared input
    /// ports. This supports variadic nodes while keeping declared ports
    /// required and enforcing strict 1:1 connectivity on every wired port.
    pub fn set_fixed_input(mut self, is_fixed: bool) -> Self {
        self.input_ports = self.input_ports.set_fixed(is_fixed);
        self
    }

    /// Append an output DataFrame port.
    pub fn add_output_port(mut self, schema: Option<SchemaRef>) -> Self {
        self.output_ports.add_port(schema);
        self
    }

    /// Append an output port carrying a specific value type.
    pub fn add_output_port_of_type(
        mut self,
        schema: Option<SchemaRef>,
        data_type: PortType,
    ) -> Self {
        self.output_ports.add_port_of_type(schema, data_type);
        self
    }

    /// Append an input DataFrame port.
    pub fn add_input_port(mut self, schema: Option<SchemaRef>) -> Self {
        self.input_ports.add_port(schema);
        self
    }

    /// Append an input port carrying a specific value type.
    pub fn add_input_port_of_type(
        mut self,
        schema: Option<SchemaRef>,
        data_type: PortType,
    ) -> Self {
        self.input_ports.add_port_of_type(schema, data_type);
        self
    }

    /// Append an optional, schema-less input port carrying a specific value type.
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

    /// Returns `true` if the input port count is fixed.
    pub fn is_fixed_input(&self) -> bool {
        self.input_ports.is_fixed()
    }
}

impl Serialize for NodePorts {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("NodePorts", 2)?;
        state.serialize_field("input_ports", &self.input_ports)?;
        state.serialize_field("output_ports", &self.output_ports)?;
        state.end()
    }
}
