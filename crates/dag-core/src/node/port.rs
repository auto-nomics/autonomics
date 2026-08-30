//! Individual node ports.

use arrow_schema::SchemaRef;
use serde::ser::{SerializeMap, Serializer};
use serde::{Serialize, Serializer as _};

use super::id::PortId;
use crate::value::PortType;

/// A named, optionally-typed socket on a node.
///
/// `schema` is `None` when the shape of the data is not known at graph-build
/// time (for example, a source whose schema is discovered at runtime). When
/// both ends of an edge declare a schema, the engine validates compatibility.
#[derive(Debug, Clone)]
pub struct Port {
    pub index: PortId,
    pub label: Option<String>,
    pub schema: Option<SchemaRef>,
    pub data_type: PortType,
    pub required: bool,
}

impl Port {
    /// An untyped DataFrame port whose schema is discovered at runtime.
    pub fn new(index: PortId, schema: Option<SchemaRef>) -> Self {
        Self {
            index,
            label: None,
            schema,
            data_type: PortType::DataFrame,
            required: true,
        }
    }

    /// Attach a semantic label for graph editors and generated node docs.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// A typed DataFrame port.
    pub fn typed(index: PortId, schema: SchemaRef) -> Self {
        Self {
            index,
            label: None,
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

    pub fn get_code(&self) -> String {
        format!("port_{}", self.index)
    }
}

/// Serializable description of one column in a port's Arrow schema.
///
/// `arrow_schema::DataType` is not serde-serializable without an extra feature
/// flag, so the type is surfaced as its `Debug` string. This is enough for a
/// caller wiring edges to reason about column shape.
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
        if let Some(label) = &self.label {
            map.serialize_entry("label", label)?;
        }
        map.serialize_entry("data_type", &self.data_type)?;
        map.serialize_entry("required", &self.required)?;

        match &self.schema {
            Some(schema) => {
                let fields: Vec<PortFieldDto<'_>> = schema
                    .fields()
                    .iter()
                    .map(|field| PortFieldDto {
                        name: field.name().as_str(),
                        data_type: format!("{:?}", field.data_type()),
                        nullable: field.is_nullable(),
                    })
                    .collect();
                map.serialize_entry("schema", &fields)?;
            }
            None => map.serialize_entry("schema", &None::<()>)?,
        }

        map.end()
    }
}
