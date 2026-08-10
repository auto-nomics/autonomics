use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A struct that can describe its own tool definition.
///
/// This trait is typically derived via `#[derive(ToolInput)]` (from the
/// `agentik-proc` crate) paired with `#[tool(name, description)]`.
///
/// Implementations return a [`ToolDefinition`] whose JSON Schema mirrors
/// the struct's fields: each named field becomes a parameter, field types
/// map to JSON Schema types, and `Option<T>` fields are treated as optional.
///
/// # Example
///
/// ```ignore
/// #[derive(Deserialize, ToolInput)]
/// #[tool(name = "bash", description = "Run shell command")]
/// pub struct BashInput {
///     #[doc = "Brief explanation"]
///     pub reason: String,
/// }
/// ```
pub trait ToolInput: Serialize {
    /// Generate a [`ToolDefinition`] from the struct's metadata.
    fn definition() -> ToolDefinition;
}

/// Blanket impl for `serde_json::Value` — used by test-only tools
/// and tools that accept arbitrary JSON without typed deserialization.
impl ToolInput for Value {
    fn definition() -> ToolDefinition {
        ToolDefinition {
            name: String::new(),
            description: String::new(),
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: Map::new(),
                required: vec![],
                additional: Map::new(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: ToolInputSchema,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInputSchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub properties: Map<String, Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    #[serde(flatten)]
    pub additional: Map<String, Value>,
}

impl ToolInputSchema {
    /// Build a [`ToolInputSchema`] from a JSON Schema describing the tool's
    /// input struct (typically produced by `schemars`).
    ///
    /// The root schema is expected to be a JSON object. The well-known keys
    /// `type` / `properties` / `required` are lifted into the typed fields;
    /// everything else (e.g. `additionalProperties`) is preserved via the
    /// `additional` flatten. The `"$schema"` meta-URI and generated `"title"`
    /// are stripped — they are schemars bookkeeping that some provider APIs
    /// reject.
    #[must_use]
    pub fn from_root_schema(mut root: Value) -> Self {
        let Some(obj) = root.as_object_mut() else {
            return Self {
                schema_type: "object".to_string(),
                properties: Map::new(),
                required: Vec::new(),
                additional: Map::new(),
            };
        };
        obj.remove("$schema");
        obj.remove("title");
        let schema_type = obj
            .remove("type")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "object".to_string());
        let properties = obj
            .remove("properties")
            .and_then(|v| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default();
        let required = obj
            .remove("required")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        Self {
            schema_type,
            properties,
            required,
            additional: obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolChoice {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "any")]
    Any,
    #[serde(rename = "tool")]
    Tool { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: ToolResultContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Json(Value),
    Blocks(Vec<ToolResultBlock>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolResultBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ImageSource },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ImageSource {
    #[serde(rename = "base64")]
    Base64 { media_type: String, data: String },
}

impl ToolResult {
    #[must_use]
    pub fn from_pending_task(tool_use_id: &str, seq: u64) -> Self {
        Self {
            tool_use_id: tool_use_id.to_string(),
            content: ToolResultContent::Text(format!(
                "Task #{seq} is running in background. \
                 The result will be delivered automatically when it completes."
            )),
            is_error: None,
        }
    }

    #[must_use]
    pub fn task_finish_notification(tool_use_id: &str, seq: u64) -> Self {
        Self {
            tool_use_id: tool_use_id.to_string(),
            content: ToolResultContent::Text(format!(
                "Task #{seq} has finished. \
                 Use `view_task_results` with task={seq} to view output."
            )),
            is_error: None,
        }
    }

    #[must_use]
    pub fn with_id<T: Into<String>>(mut self, id: T) -> Self {
        self.tool_use_id = id.into();
        self
    }
}

#[derive(Debug, Clone)]
pub struct ToolDefinitionBuilder {
    name: String,
    description: String,
    properties: Map<String, Value>,
    required: Vec<String>,
    additional: Map<String, Value>,
}

impl ToolDefinitionBuilder {
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            properties: Map::new(),
            required: Vec::new(),
            additional: Map::new(),
        }
    }

    #[must_use]
    pub fn parameter(
        mut self,
        name: impl Into<String>,
        param_type: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        let param_name = name.into();
        let param_schema = serde_json::json!({
            "type": param_type.into(),
            "description": description.into()
        });
        self.properties.insert(param_name, param_schema);
        self
    }

    #[must_use]
    pub fn enum_parameter(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        values: Vec<String>,
    ) -> Self {
        let param_name = name.into();
        let param_schema = serde_json::json!({
            "type": "string",
            "description": description.into(),
            "enum": values
        });
        self.properties.insert(param_name, param_schema);
        self
    }

    #[must_use]
    pub fn array_parameter(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        item_type: impl Into<String>,
    ) -> Self {
        let param_name = name.into();
        let param_schema = serde_json::json!({
            "type": "array",
            "description": description.into(),
            "items": {
                "type": item_type.into()
            }
        });
        self.properties.insert(param_name, param_schema);
        self
    }

    #[must_use]
    pub fn object_parameter(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        properties: Map<String, Value>,
    ) -> Self {
        let param_name = name.into();
        let param_schema = serde_json::json!({
            "type": "object",
            "description": description.into(),
            "properties": properties
        });
        self.properties.insert(param_name, param_schema);
        self
    }

    #[must_use]
    pub fn required(mut self, name: impl Into<String>) -> Self {
        let param_name = name.into();
        if !self.required.contains(&param_name) {
            self.required.push(param_name);
        }
        self
    }

    #[must_use]
    pub fn additional_property(mut self, key: impl Into<String>, value: Value) -> Self {
        self.additional.insert(key.into(), value);
        self
    }

    #[must_use]
    pub fn default(mut self, name: impl Into<String>, value: Value) -> Self {
        let param_name = name.into();
        if let Some(obj) = self
            .properties
            .get_mut(&param_name)
            .and_then(|schema| schema.as_object_mut())
        {
            obj.insert("default".to_string(), value);
        }
        self
    }

    #[must_use]
    pub fn build(self) -> ToolDefinition {
        ToolDefinition {
            name: self.name,
            description: self.description,
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: self.properties,
                required: self.required,
                additional: self.additional,
            },
        }
    }
}

/// Per-field override applied to a `schemars`-generated schema.
///
/// `description` and `default` are injected into the field's property schema
/// after generation. This preserves the `#[desc = "..."]` / `#[default = ...]`
/// attribute semantics of the `#[tool]` macro on top of schemars.
#[derive(Debug, Clone)]
pub struct FieldOverride {
    pub name: &'static str,
    pub description: Option<String>,
    pub default: Option<Value>,
}

/// Build a [`ToolDefinition`] from a type's `schemars` JSON Schema.
///
/// This is the schemars-backed replacement for the hand-rolled type mapping
/// that used to live in the `#[tool]` proc macro. It generates a full,
/// inlined JSON Schema for `T` (so nested structs / enums / `serde_json::Value`
/// / `Option<T>` / `Vec<T>` are all handled correctly), then applies the
/// per-field [`FieldOverride`]s (descriptions and defaults declared via
/// `#[desc]` / `#[default]`).
///
/// Subschemas are inlined (`inline_subschemas = true`) so the result contains
/// no `$ref` / `$defs`, which several provider APIs do not resolve.
#[must_use]
/// # Panics
///
/// # Panics
///
/// Panics if the schemars-generated schema fails to serialize to JSON,
/// which should never happen for valid types.
pub fn tool_definition_from_schema<T: JsonSchema>(
    name: &str,
    description: &str,
    overrides: &[FieldOverride],
) -> ToolDefinition {
    let mut settings = schemars::generate::SchemaSettings::default();
    settings.inline_subschemas = true;
    let schema = settings.into_generator().into_root_schema_for::<T>();
    let mut root =
        serde_json::to_value(&schema).expect("schemars-generated schema must serialize to JSON");

    // Apply per-field overrides on top of the generated properties.
    if let Some(props) = root.get_mut("properties").and_then(|v| v.as_object_mut()) {
        for ov in overrides {
            let Some(prop) = props.get_mut(ov.name).and_then(|v| v.as_object_mut()) else {
                continue;
            };
            if let Some(desc) = &ov.description
                && !desc.is_empty()
            {
                prop.insert("description".to_string(), Value::String(desc.clone()));
            }
            if let Some(default) = &ov.default {
                prop.insert("default".to_string(), default.clone());
            }
        }

        // Tool-calling LLMs rely on `type` to decide how to serialise each
        // parameter.  schemars omits `type` for `serde_json::Value` (it can be
        // anything), which leaves the model without guidance and may produce
        // string-encoded objects that silently bypass tool-level validation.
        // For every property that schemars left without a `type`, default to
        // `"object"` — the correct choice for arbitrary JSON payloads like
        // node specs.
        for prop_schema in props.values_mut() {
            if !prop_schema.is_object() {
                continue;
            }
            let obj = prop_schema.as_object_mut().unwrap();
            if !obj.contains_key("type") {
                obj.insert("type".to_string(), Value::String("object".to_string()));
            }
        }
    }

    let input_schema = ToolInputSchema::from_root_schema(root);
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
    }
}

impl ToolDefinition {
    #[must_use]
    pub fn builder() -> ToolDefinitionBuilder {
        ToolDefinitionBuilder {
            name: String::new(),
            description: String::new(),
            properties: Map::new(),
            required: Vec::new(),
            additional: Map::new(),
        }
    }
    /// Validate a JSON input against this tool's schema.
    ///
    /// # Errors
    ///
    /// Returns a [`ToolValidationError`] if:
    /// - the input is not a JSON object
    /// - a required field is missing
    /// - a field's value type does not match the schema
    /// - a field's schema is missing a `type` key
    pub fn validate_input(&self, input: &Value) -> Result<(), ToolValidationError> {
        if let Value::Object(input_obj) = input {
            // 1. validate input object doesn't miss required fields.
            for required_field in &self.input_schema.required {
                if !input_obj.contains_key(required_field) {
                    return Err(ToolValidationError::MissingRequiredField {
                        field: required_field.clone(),
                        tool: self.name.clone(),
                    });
                }
            }

            // 2. validate field schema by field name.
            for (field_name, field_value) in input_obj {
                if let Some(property_schema) = self.input_schema.properties.get(field_name) {
                    self.validate_field_type(field_name, field_value, property_schema)?;
                }
            }

            Ok(())
        } else {
            Err(ToolValidationError::InvalidInputType {
                expected: "object".to_string(),
                actual: input.to_string(),
                tool: self.name.clone(),
            })
        }
    }

    fn validate_field_type(
        &self,
        field_name: &str,
        value: &Value,
        schema: &Value,
    ) -> Result<(), ToolValidationError> {
        if let Some(expected_type) = schema.get("type").and_then(|t| t.as_str()) {
            let actual_type = match value {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };

            if expected_type != actual_type {
                return Err(ToolValidationError::InvalidFieldType {
                    field: field_name.to_string(),
                    expected: expected_type.to_string(),
                    actual: actual_type.to_string(),
                    tool: self.name.clone(),
                });
            }
        } else {
            return Err(ToolValidationError::MissingSchema {
                field: field_name.to_string(),
                tool: self.name.clone(),
                schema: schema.clone(),
                actual: match value {
                    Value::Null => "null".to_string(),
                    Value::Bool(_) => "boolean".to_string(),
                    Value::Number(_) => "number".to_string(),
                    Value::String(_) => "string".to_string(),
                    Value::Array(_) => "array".to_string(),
                    Value::Object(_) => "object".to_string(),
                },
            });
        }

        Ok(())
    }
}

impl ToolChoice {
    #[must_use]
    pub fn auto() -> Self {
        Self::Auto
    }

    #[must_use]
    pub fn any() -> Self {
        Self::Any
    }

    #[must_use]
    pub fn tool(name: impl Into<String>) -> Self {
        Self::Tool { name: name.into() }
    }
}

impl ToolResult {
    #[must_use]
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            tool_use_id: String::new(),
            content: ToolResultContent::Text(content.into()),
            is_error: None,
        }
    }

    #[must_use]
    pub fn success_json(content: Value) -> Self {
        Self {
            tool_use_id: String::new(),
            content: ToolResultContent::Json(content),
            is_error: None,
        }
    }

    #[must_use]
    pub fn error(error_message: impl Into<String>) -> Self {
        Self {
            tool_use_id: String::new(),
            content: ToolResultContent::Text(error_message.into()),
            is_error: Some(true),
        }
    }

    #[must_use]
    pub fn with_blocks(blocks: Vec<ToolResultBlock>) -> Self {
        Self {
            tool_use_id: String::new(),
            content: ToolResultContent::Blocks(blocks),
            is_error: None,
        }
    }

    /// Create an error result with a specific `tool_use_id` (for orchestration layer).
    #[must_use]
    pub fn error_with_id(tool_use_id: impl Into<String>, error_message: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(error_message.into()),
            is_error: Some(true),
        }
    }

    /// Extract text content, joining multiple text segments. Non-text (images) are skipped.
    #[must_use]
    pub fn text_content(&self) -> String {
        match &self.content {
            ToolResultContent::Text(s) => s.clone(),
            ToolResultContent::Json(v) => v.to_string(),
            ToolResultContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ToolResultBlock::Text { text } => Some(text.as_str()),
                    ToolResultBlock::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

impl ToolResultBlock {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    #[must_use]
    pub fn image_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self::Image {
            source: ImageSource::Base64 {
                media_type: media_type.into(),
                data: data.into(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ToolValidationError {
    #[error("Missing required field '{field}' for tool '{tool}'")]
    MissingRequiredField { field: String, tool: String },
    #[error("Invalid input type for tool '{tool}': expected {expected}, got {actual}")]
    InvalidInputType {
        expected: String,
        actual: String,
        tool: String,
    },
    #[error("Invalid type for field '{field}' in tool '{tool}': expected {expected}, got {actual}")]
    InvalidFieldType {
        field: String,
        expected: String,
        actual: String,
        tool: String,
    },
    #[error(
        "Field '{field}' in tool '{tool}' has no 'type' in its schema (got: {schema}). \
         Received value of type '{actual}'. \
         Ensure the field type implements schemars::JsonSchema and produces a schema with a 'type' key."
    )]
    MissingSchema {
        field: String,
        tool: String,
        schema: Value,
        actual: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerTool {
    #[serde(rename = "web_search_20250305")]
    WebSearch {
        #[serde(skip_serializing_if = "Option::is_none")]
        parameters: Option<WebSearchParameters>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebSearchParameters {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_results: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    region: Option<String>,
}

impl ServerTool {
    #[must_use]
    pub fn web_search() -> Self {
        Self::WebSearch { parameters: None }
    }

    #[must_use]
    pub fn web_search_with_params(parameters: WebSearchParameters) -> Self {
        Self::WebSearch {
            parameters: Some(parameters),
        }
    }
}

impl WebSearchParameters {
    #[must_use]
    pub fn with_max_results(max_results: u32) -> Self {
        Self {
            max_results: Some(max_results),
            language: None,
            region: None,
        }
    }

    #[must_use]
    pub fn language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    #[must_use]
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_builder() {
        let tool = ToolDefinitionBuilder::new("get_weather", "Get the current weather")
            .parameter("location", "string", "The location to get weather for")
            .parameter("unit", "string", "Temperature unit")
            .enum_parameter(
                "format",
                "Response format",
                vec!["json".to_string(), "text".to_string()],
            )
            .required("location")
            .build();

        assert_eq!(tool.name, "get_weather");
        assert_eq!(tool.description, "Get the current weather");
        assert_eq!(tool.input_schema.required, vec!["location"]);
        assert_eq!(tool.input_schema.properties.len(), 3);
    }

    #[test]
    fn test_tool_validation() {
        let tool = ToolDefinitionBuilder::new("test_tool", "Test tool")
            .parameter("required_field", "string", "Required field")
            .parameter("optional_field", "number", "Optional field")
            .required("required_field")
            .build();

        let valid_input = json!({
            "required_field": "test",
            "optional_field": 42
        });
        assert!(tool.validate_input(&valid_input).is_ok());

        let invalid_input = json!({
            "optional_field": 42
        });
        assert!(tool.validate_input(&invalid_input).is_err());

        let wrong_type_input = json!({
            "required_field": 123
        });
        assert!(tool.validate_input(&wrong_type_input).is_err());
    }

    #[test]
    fn test_tool_choice_serialization() {
        let auto_choice = ToolChoice::auto();
        let json = serde_json::to_value(&auto_choice).unwrap();
        assert_eq!(json, json!({"type": "auto"}));

        let tool_choice = ToolChoice::tool("get_weather");
        let json = serde_json::to_value(&tool_choice).unwrap();
        assert_eq!(json, json!({"type": "tool", "name": "get_weather"}));
    }

    #[test]
    fn test_tool_result_creation() {
        let success_result = ToolResult::success("Success message");
        assert_eq!(success_result.tool_use_id, "");
        assert!(success_result.is_error.is_none());

        let error_result = ToolResult::error("Error message");
        assert_eq!(error_result.tool_use_id, "");
        assert_eq!(error_result.is_error, Some(true));

        let error_with_id = ToolResult::error_with_id("tool_456", "Error message");
        assert_eq!(error_with_id.tool_use_id, "tool_456");
        assert_eq!(error_with_id.is_error, Some(true));

        let json_result = ToolResult::success_json(json!({"temperature": 72}));
        if let ToolResultContent::Json(value) = json_result.content {
            assert_eq!(value["temperature"], 72);
        } else {
            panic!("Expected JSON content");
        }
    }

    #[test]
    fn test_server_tool_creation() {
        let web_search = ServerTool::web_search();
        assert!(matches!(
            web_search,
            ServerTool::WebSearch { parameters: None }
        ));

        let params = WebSearchParameters::with_max_results(10)
            .language("en")
            .region("US");
        let web_search_with_params = ServerTool::web_search_with_params(params);

        if let ServerTool::WebSearch {
            parameters: Some(p),
        } = web_search_with_params
        {
            assert_eq!(p.max_results, Some(10));
            assert_eq!(p.language, Some("en".to_string()));
            assert_eq!(p.region, Some("US".to_string()));
        } else {
            panic!("Expected web search with parameters");
        }
    }

    #[test]
    fn test_tool_serialization() {
        let tool = ToolDefinitionBuilder::new("calculate", "Perform mathematical calculations")
            .parameter(
                "expression",
                "string",
                "Mathematical expression to evaluate",
            )
            .required("expression")
            .build();

        let json = serde_json::to_string(&tool).unwrap();
        let deserialized: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(tool, deserialized);
    }

    #[test]
    fn test_tool_use_deserialization() {
        let json = r#"
        {
            "id": "toolu_123456",
            "name": "get_weather",
            "input": {
                "location": "San Francisco, CA",
                "unit": "celsius"
            }
        }"#;

        let tool_use: ToolUse = serde_json::from_str(json).unwrap();
        assert_eq!(tool_use.id, "toolu_123456");
        assert_eq!(tool_use.name, "get_weather");
        assert_eq!(tool_use.input["location"], "San Francisco, CA");
        assert_eq!(tool_use.input["unit"], "celsius");
    }
}
