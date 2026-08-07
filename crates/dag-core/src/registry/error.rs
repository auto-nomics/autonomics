use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced while resolving / building a node from its JSON spec.
///
/// The agent-facing variant is [`Error::SpecRejection`] — it carries the node
/// kind, the underlying reason, the full expected JSON Schema, and concrete
/// remediation guidance so a tool-calling LLM can self-correct on the next
/// turn. [`Error::SpecDeserialize`] is an internal form kept only so factories
/// can keep the ergonomic `serde_json::from_value(spec)?`; `NodeRegistry::
/// build_node` always upgrades it into a `SpecRejection` before it can leave
/// the registry.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Unknown(String),

    #[error("cannot found node factory for kind '{kind}'")]
    FactoryNotFound { kind: String },

    /// Raw `serde_json` deserialization failure of a node spec. Internal — see
    /// the type-level docs. Produced via `#[from]` so factories can write
    /// `serde_json::from_value(spec)?`.
    #[error("cannot deserialize node spec: {source}")]
    SpecDeserialize {
        #[from]
        source: serde_json::Error,
    },

    /// A node `spec` was rejected because it does not match the kind's JSON
    /// Schema. This is what `add_node` surfaces to the agent.
    #[error(
        "node spec for kind `{kind}` could not be parsed.\n\
         \n\
         Why it failed: {reason}\\
         \n\
         \n\
         The `spec` MUST be a JSON object matching this schema:\n\
         {schema_pretty}\n\
         \n\
         How to fix — retry `add_node` with a corrected `spec`:\n\
           • Arrays use JSON array syntax, e.g. `\"field\": [1.0, 2.0]` — never an object like `{{\"item\": ...}}`.\n\
           • Numbers are unquoted, e.g. `\"n_blocks\": 200` — never a string like `\"200\"`.\n\
           • Booleans are `true` / `false`, not `\"true\"` / `\"false\"`.\n\
           • Include every field listed under `\"required\"`; omit keys the schema does not list.\n\
           • Re-fetch the exact schema any time with `get_node_spec` (kind = \"{kind}\")."
    )]
    SpecRejection {
        /// The node kind whose spec failed to parse (e.g. "ldsc_rg").
        kind: String,
        /// Human-readable reason, translated from serde's Rust type jargon
        /// into JSON terms an agent reliably understands.
        reason: String,
        /// Pretty-printed JSON Schema the spec must conform to.
        schema_pretty: String,
    },
}

impl Error {
    /// Build an agent-facing [`Error::SpecRejection`] from a raw serde failure,
    /// attaching the node kind and the factory's JSON Schema.
    pub fn spec_rejection_from(
        kind: &str,
        schema: &serde_json::Value,
        source: serde_json::Error,
    ) -> Self {
        let schema_pretty =
            serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string());
        Self::SpecRejection {
            kind: kind.to_string(),
            reason: humanize_serde_error(&source.to_string()),
            schema_pretty,
        }
    }
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Unknown(value.to_string())
    }
}

/// Translate `serde_json`'s Rust-centric deserialization jargon into JSON terms
/// a tool-calling agent reliably understands.
///
/// serde reports the *expected Rust type* (`a sequence`, `floating point \`f64\``,
/// `struct LdscRgConfig`) and the *received Rust shape* (`map`, `string`) —
/// phrasing that a weak LLM may misread (e.g. "sequence" is not obviously
/// "JSON array"). We restate each canonical phrase as its JSON equivalent so
/// the "Why it failed" line is unambiguous. Unknown phrases pass through
/// unchanged.
fn humanize_serde_error(msg: &str) -> String {
    let mut out = msg.to_string();
    // Expected-type phrases.
    out = out.replace("expected a sequence", "expected a JSON array");
    out = out.replace("expected a map", "expected a JSON object");
    out = out.replace("expected struct", "expected a JSON object");
    out = out.replace("expected a boolean", "expected a boolean (true/false)");
    out = out.replace("expected a string", "expected a JSON string");
    out = out.replace("expected floating point", "expected a number");
    out = out.replace("expected integer", "expected an integer number");
    // Received-type phrases (the "got" half of "invalid type: X, expected Y").
    out = out.replace("invalid type: map", "got a JSON object");
    out = out.replace("invalid type: sequence", "got a JSON array");
    out = out.replace("invalid type: string", "got a string");
    out = out.replace("invalid type: boolean", "got a boolean");
    out = out.replace("invalid type: floating point", "got a number");
    out = out.replace("invalid type: integer", "got an integer");
    out = out.replace("invalid type: null", "got null");
    // Missing fields.
    out = out.replace("missing field", "missing required field");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_translates_sequence_and_map() {
        let raw = "invalid type: map, expected a sequence";
        assert_eq!(
            humanize_serde_error(raw),
            "got a JSON object, expected a JSON array"
        );
    }

    #[test]
    fn humanize_translates_number_and_integer() {
        assert_eq!(
            humanize_serde_error("invalid type: string \"200\", expected integer `usize`"),
            "got a string \"200\", expected an integer number `usize`"
        );
        assert_eq!(
            humanize_serde_error("invalid type: string \"1.5\", expected floating point `f64`"),
            "got a string \"1.5\", expected a number `f64`"
        );
    }

    #[test]
    fn humanize_marks_missing_fields() {
        assert_eq!(
            humanize_serde_error("missing field `sql_query`"),
            "missing required field `sql_query`"
        );
    }

    #[test]
    fn humanize_leaves_unknown_phrases_untouched() {
        let raw = "unknown variant `ftp`, expected `file` or `iceberg`";
        assert_eq!(humanize_serde_error(raw), raw);
    }

    #[test]
    fn spec_rejection_message_names_kind_and_lists_schema_and_fix() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "m": { "type": "array", "items": { "type": "number" } }
            },
            "required": ["m"]
        });
        // Fabricate the exact serde error the reported bug produced
        // (`m: {"item": ...}` into a `Vec<f64>` target, pre-normalization).
        let serde_err =
            serde_json::from_value::<Vec<f64>>(serde_json::json!({ "a": 1 })).unwrap_err();
        assert_eq!(
            serde_err.to_string(),
            "invalid type: map, expected a sequence"
        );
        let err = Error::spec_rejection_from("ldsc_rg", &schema, serde_err);
        let msg = format!("{err}");

        assert!(
            msg.contains("`ldsc_rg`"),
            "message must name the kind: {msg}"
        );
        assert!(
            msg.contains("got a JSON object, expected a JSON array"),
            "message must include the humanized reason: {msg}"
        );
        assert!(
            msg.contains("\"type\": \"array\""),
            "message must embed the expected schema: {msg}"
        );
        assert!(
            msg.contains("get_node_spec"),
            "message must point the agent at get_node_spec: {msg}"
        );
        assert!(
            msg.contains("retry `add_node`"),
            "message must tell the agent to retry: {msg}"
        );
    }
}
