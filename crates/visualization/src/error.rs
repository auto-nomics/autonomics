use thiserror::Error;

/// Structured diagnostics captured from the R subprocess.
///
/// The R wrapper script wraps the caller's code in `tryCatch` +
/// `withCallingHandlers` and writes the result to `diagnostics.json`. The
/// script always exits 0; success/failure is decided in Rust by reading this
/// file, not by the subprocess exit code.
///
/// `error` being `Some` means R threw a fatal error (`stop()`, parse error,
/// missing package, …). `warnings` and `messages` are non-fatal conditions
/// captured during execution and are populated even on success, so callers
/// can surface them (e.g. in a node report) for debugging.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Diagnostics {
    /// Fatal error message from R. `None` means execution completed.
    #[serde(default)]
    pub error: Option<String>,

    /// Non-fatal warnings emitted during execution.
    #[serde(default)]
    pub warnings: Vec<String>,

    /// `message()` output captured during execution.
    #[serde(default)]
    pub messages: Vec<String>,

    /// Tail of `sys.calls()` at the error site (empty when no error).
    #[serde(default)]
    pub call_stack: Vec<String>,
}

impl Diagnostics {
    /// Returns `true` when no fatal error occurred.
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

impl std::fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(err) = &self.error {
            writeln!(f, "error: {err}")?;
        }
        if !self.warnings.is_empty() {
            writeln!(f, "warnings ({}):", self.warnings.len())?;
            for w in &self.warnings {
                writeln!(f, "  • {w}")?;
            }
        }
        if !self.messages.is_empty() {
            writeln!(f, "messages ({}):", self.messages.len())?;
            for m in &self.messages {
                writeln!(f, "  • {m}")?;
            }
        }
        if !self.call_stack.is_empty() {
            writeln!(f, "call_stack ({} frames):", self.call_stack.len())?;
            for (i, c) in self.call_stack.iter().enumerate() {
                writeln!(f, "  {i}: {c}")?;
            }
        }
        Ok(())
    }
}

/// Errors produced by the visualization renderer.
#[derive(Debug, Error)]
pub enum VizError {
    /// A column referenced by the spec was not found in the source data.
    #[error("column not found: {0}")]
    ColumnNotFound(String),

    /// A column exists but its Arrow type cannot be coerced to the required
    /// shape (numeric or text).
    #[error("column `{column}` has unsupported type for this mapping: {ty}")]
    UnsupportedType { column: String, ty: String },

    /// The system has no `Rscript` on PATH.
    #[error("Rscript not found on PATH — is R installed?")]
    RscriptNotFound,

    /// `Rscript` failed: either it exited non-zero (syntax error in the
    /// generated script, segfault, …) **or** the structured diagnostics
    /// reported a fatal R error. Carries the parsed `diagnostics` when the
    /// JSON file was written, plus the raw `stderr` as a fallback.
    #[error(
        "Rscript failed (exit code {code}):\n{stderr}\n--- diagnostics ---\n{diagnostics}"
    )]
    RscriptFailed {
        code: i32,
        stderr: String,
        diagnostics: Diagnostics,
    },

    /// `Rscript` was killed because it exceeded the render timeout.
    #[error("Rscript timed out after {0} seconds")]
    RscriptTimeout(u64),

    /// The supplied plot code is empty or not valid R.
    #[error("invalid plot code: {0}")]
    InvalidPlotCode(String),

    /// Arrow serialization error (writing the IPC stream for R to read).
    #[error(transparent)]
    Arrow(#[from] arrow::error::ArrowError),

    /// I/O error while writing temp files or the output artifact.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = VizError> = std::result::Result<T, E>;
