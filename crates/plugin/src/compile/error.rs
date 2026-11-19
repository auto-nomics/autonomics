use thiserror::Error;

use crate::node_definition::ParamType;

pub type Result<T> = std::result::Result<T, Error>;

/// Compile-time failures of the manifest → `ContainerCommandSpec` pipeline.
///
/// Every variant carries the node kind and the offending parameter so the
/// loader and `nodedev validate` can point at the exact manifest spot —
/// a compile failure must name what to fix, not just that it failed.
#[derive(Debug, Error)]
pub enum Error {
    /// The submitted spec value is not a JSON object.
    #[error("node `{kind}`: params must be a JSON object")]
    ParamsNotObject { kind: String },

    /// A submitted key is not declared in the manifest's params. The
    /// compiled schema is `additionalProperties: false`; this is the same
    /// gate enforced independently at compile time.
    #[error("node `{kind}`: unknown param `{name}`")]
    UnknownParam { kind: String, name: String },

    /// A required param was absent from the values and has no default.
    #[error("node `{kind}`: missing required param `{name}`")]
    MissingParam { kind: String, name: String },

    /// A value's JSON type does not match the declared param type.
    #[error("node `{kind}`: param `{name}` expects {expected}, got `{value}`")]
    TypeMismatch {
        kind: String,
        name: String,
        expected: ParamType,
        value: serde_json::Value,
    },

    /// A numeric bound or array length bound was violated.
    #[error("node `{kind}`: param `{name}` value `{value}` violates `{bound}`")]
    BoundViolation {
        kind: String,
        name: String,
        /// Which bound failed: `minimum`, `maximum`, `exclusiveMinimum`,
        /// `exclusiveMaximum`, `minItems`, `maxItems`.
        bound: &'static str,
        value: f64,
    },

    /// A `requires` gate was not satisfied: the param resolved to true
    /// while a named bool target did not.
    #[error("node `{kind}`: param `{name}` requires `{target}` to be true")]
    RequiresUnmet {
        kind: String,
        name: String,
        target: String,
    },

    /// The command template referenced an undeclared param at render time.
    /// Load-time closure validation should have caught this earlier; the
    /// renderer double-checks as defense in depth.
    #[error("node `{kind}`: template references undeclared param `{name}`")]
    UndeclaredTemplateRef { kind: String, name: String },

    /// Fallback for failures without a dedicated variant.
    #[error("{0}")]
    Other(String),
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Other(value.to_string())
    }
}
