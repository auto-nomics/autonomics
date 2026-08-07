//! Shared helpers for R/Python codegen — eliminates boilerplate for the
//! common pattern: deserialize spec → build function call → assign output.
//!
//! Every node's codegen follows the same shape:
//!   1. Deserialize spec JSON into the typed config struct
//!   2. Map spec fields + input variable names → target-language function call
//!   3. Return `NodeCodegen`
//!
//! These helpers cover step 2's string-building so each node only writes
//! the argument mapping.

// ── R string building ──────────────────────────────────────────────────────

/// Quote a Rust string as an R string literal: `hello` → `"hello"`.
pub fn r_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Build an R string vector: `["a", "b"]` → `"a", "b"`.
pub fn r_vec(items: &[String]) -> String {
    items
        .iter()
        .map(|s| r_str(s))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Build an R numeric vector: `[1.0, 2.5]` → `1, 2.5`.
pub fn r_vec_f64(items: &[f64]) -> String {
    items
        .iter()
        .map(|v| format!("{v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Build an R formula string: `y ~ x1 + x2 + x3`.
pub fn r_formula(response: &str, predictors: &[String], intercept: bool) -> String {
    let rhs = if predictors.is_empty() {
        "1".to_string()
    } else {
        predictors.join(" + ")
    };
    if intercept {
        format!("{response} ~ {rhs}")
    } else {
        format!("{response} ~ {rhs} - 1")
    }
}

/// Build an R function call from named arguments:
/// `r_call("lm", &[("formula", "y ~ x".into()), ("data", "df".into())])`
/// → `lm(formula = y ~ x, data = df)`
pub fn r_call(func: &str, args: &[(&str, String)]) -> String {
    let body: Vec<String> = args.iter().map(|(k, v)| format!("{k} = {v}")).collect();
    format!("{func}({})", body.join(", "))
}

/// Build a multi-line R function call (for long argument lists):
/// ```r
/// lm(
///   formula = y ~ x,
///   data = df
/// )
/// ```
pub fn r_call_multiline(func: &str, args: &[(&str, String)], indent: usize) -> String {
    let pad = " ".repeat(indent);
    let inner: Vec<String> = args
        .iter()
        .map(|(k, v)| format!("{pad}  {k} = {v}"))
        .collect();
    format!("{func}(\n{}\n{pad})", inner.join(",\n"))
}

/// Build a `data.frame()` from column mappings:
/// `r_dataframe(&[("snp", "input$snp"), ("beta", "input$beta")])`
/// → `data.frame(snp = input$snp, beta = input$beta)`
pub fn r_dataframe(mappings: &[(&str, &str)]) -> String {
    let fields: Vec<String> = mappings
        .iter()
        .map(|(col, expr)| format!("{col} = {expr}"))
        .collect();
    format!("data.frame({})", fields.join(", "))
}

/// Access a column from an upstream variable: `input$column_name`.
pub fn r_col(input_var: &str, column: &str) -> String {
    format!("{input_var}${column}")
}

// ── Python string building ─────────────────────────────────────────────────

/// Quote a Rust string as a Python string literal.
pub fn py_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Build a Python function call from keyword arguments.
pub fn py_call(func: &str, args: &[(&str, String)]) -> String {
    let body: Vec<String> = args.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{func}({})", body.join(", "))
}

// ── shared utilities ───────────────────────────────────────────────────────

/// Resolve the first input variable name, or fall back to a placeholder.
pub fn input_0<'a>(ctx: &'a crate::codegen::CodegenCtx) -> &'a str {
    ctx.input_vars
        .first()
        .map(|s| s.as_str())
        .unwrap_or("__missing_input")
}

/// Deserialize a spec JSON into a typed config, mapping errors.
pub fn parse_spec<T: serde::de::DeserializeOwned>(
    spec: &serde_json::Value,
    kind: &str,
) -> Result<T, crate::codegen::CodegenError> {
    serde_json::from_value(spec.clone()).map_err(|e| crate::codegen::CodegenError::BadSpec {
        kind: kind.to_string(),
        source: e,
    })
}
