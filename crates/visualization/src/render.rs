//! Render Arrow `RecordBatch`es to a PNG via R's ggplot2.
//!
//! The renderer hands data to R through the **Arrow IPC stream** format (a
//! stable, version-tolerant wire format): the batches are serialized in Rust,
//! written next to a generated R script in a tempdir, and an `Rscript`
//! subprocess reads them back with `arrow::read_ipc_stream` into a
//! `data.frame`, runs the caller-supplied ggplot2 code, and saves the PNG.
//!
//! ## Diagnostics capture
//!
//! The R wrapper wraps the caller's code in `tryCatch` +
//! `withCallingHandlers` and writes structured diagnostics
//! (`error`/`warnings`/`messages`/`call_stack`) to `diagnostics.json`. The
//! script **always exits 0**; the Rust side reads the JSON file to decide
//! success/failure. This gives callers structured access to non-fatal
//! warnings/messages even when the render succeeds — see [`Diagnostics`].
//!
//! Why a subprocess instead of in-process [`extendr_api`][crate]?  Linking
//! `libR` would force the *entire* data-engine to depend on R being present at
//! build- and runtime. Visualization is an optional capability, so the core
//! pipeline must keep working without R. Subprocess invocation keeps that
//! boundary clean: the node returns a clear [`VizError::RscriptNotFound`] when
//! R is unavailable, and the rest of the DAG is unaffected.

use std::path::{Path, PathBuf};
use std::time::Duration;

use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;

use crate::error::{Diagnostics, Result, VizError};

/// Default figure dimensions (inches) and resolution.
const DEFAULT_WIDTH: f64 = 8.0;
const DEFAULT_HEIGHT: f64 = 6.0;
const DEFAULT_DPI: f64 = 150.0;
/// Hard cap on a single render before the subprocess is killed.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Render `batches` to a PNG and return the **bytes** plus R-side
/// [`Diagnostics`], using the supplied ggplot2 code. The PNG is produced in a
/// private tempdir and never touches a caller-chosen path — use this when you
/// want to forward the bytes elsewhere (e.g. upload into a virtualized object
/// store) rather than write to disk.
///
/// The returned `Diagnostics` carry any non-fatal R warnings/messages even on
/// success, so callers can surface them (e.g. in a node report) for debugging.
///
/// See [`render_png`] for the `r_code` contract (`df` bound, must assign `p`).
pub async fn render_png_bytes(
    batches: &[RecordBatch],
    r_code: &str,
    width: Option<f64>,
    height: Option<f64>,
    dpi: Option<f64>,
) -> Result<(Vec<u8>, Diagnostics)> {
    let (ipc, width, height, dpi) = prepare(batches, r_code, width, height, dpi)?;
    run_rscript(&ipc, r_code, width, height, dpi).await
}

/// Render `batches` to a PNG at `output_path` using the supplied ggplot2 code.
///
/// # The `r_code` contract
///
/// The caller's `r_code` runs in an environment where a `data.frame` named
/// **`df`** is already bound to the input data. The code must build a ggplot
/// object and assign it to a variable named **`p`** — e.g.
///
/// ```r
/// p <- ggplot(df, aes(x = bp, y = pval)) + geom_point()
/// ```
///
/// The wrapper then calls `ggsave(output_path, plot = p, ...)` with the given
/// `width`/`height`/`dpi`.
///
/// `output_path`'s parent directory must exist and be writable. Returns the
/// captured [`Diagnostics`] (warnings/messages from R, even on success).
pub async fn render_png(
    batches: &[RecordBatch],
    r_code: &str,
    output_path: &Path,
    width: Option<f64>,
    height: Option<f64>,
    dpi: Option<f64>,
) -> Result<Diagnostics> {
    let (bytes, diag) = render_png_bytes(batches, r_code, width, height, dpi).await?;
    std::fs::write(output_path, bytes)?;
    Ok(diag)
}

/// Shared front-end: validate input, derive the schema, serialize the IPC
/// stream, and resolve default dimensions.
fn prepare(
    batches: &[RecordBatch],
    r_code: &str,
    width: Option<f64>,
    height: Option<f64>,
    dpi: Option<f64>,
) -> Result<(Vec<u8>, f64, f64, f64)> {
    let r_code = r_code.trim();
    if r_code.is_empty() {
        return Err(VizError::InvalidPlotCode("plot code is empty".to_string()));
    }

    // All batches share one schema; take it from the first non-empty batch,
    // falling back to an empty schema for a zero-row render.
    let schema = batches
        .iter()
        .map(|b| b.schema())
        .next()
        .unwrap_or_else(|| arrow::datatypes::SchemaRef::new(arrow::datatypes::Schema::empty()));

    let ipc = batches_to_ipc_stream(&schema, batches)?;
    let width = width.unwrap_or(DEFAULT_WIDTH);
    let height = height.unwrap_or(DEFAULT_HEIGHT);
    let dpi = dpi.unwrap_or(DEFAULT_DPI);
    Ok((ipc, width, height, dpi))
}

/// Serialize `RecordBatch`es (sharing one schema) into Arrow **IPC stream**
/// bytes — the format R's `arrow::read_ipc_stream` consumes.
fn batches_to_ipc_stream(
    schema: &arrow::datatypes::SchemaRef,
    batches: &[RecordBatch],
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut writer = StreamWriter::try_new(&mut buf, schema)?;
    for b in batches {
        writer.write(b)?;
    }
    writer.finish()?;
    drop(writer);
    Ok(buf)
}

/// Resolve `Rscript` on `PATH`, returning [`VizError::RscriptNotFound`] if it
/// is missing. Split out so the error is distinct from a real subprocess
/// failure.
fn resolve_rscript() -> Result<PathBuf> {
    let candidate = std::env::var_os("VISUALIZATION_RSCRIPT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("Rscript"));
    Ok(candidate)
}

/// Write the IPC bytes + a generated R script to a fresh tempdir, invoke
/// `Rscript`, and return the rendered PNG bytes plus structured diagnostics.
/// The output PNG and `diagnostics.json` live inside the tempdir (a private
/// scratch path), so the caller's filesystem layout is never touched — only
/// the returned bytes leave this function.
async fn run_rscript(
    ipc: &[u8],
    r_code: &str,
    width: f64,
    height: f64,
    dpi: f64,
) -> Result<(Vec<u8>, Diagnostics)> {
    let rscript = resolve_rscript()?;

    let tmp = tempfile::tempdir()?;
    let data_path = tmp.path().join("data.arrow_stream");
    let out_path = tmp.path().join("out.png");
    let diag_path = tmp.path().join("diagnostics.json");
    let script_path = tmp.path().join("plot.R");

    std::fs::write(&data_path, ipc)?;

    // The R wrapper: wrap *everything* (including `library()` calls) in
    // `tryCatch` + `withCallingHandlers`. Fatal errors land in `.diag$error`,
    // warnings/messages are accumulated and muffled so they don't abort. The
    // script always exits 0 — the Rust side reads `diagnostics.json` to
    // decide success/failure.
    //
    // Paths are injected as `r_escape`-d literals to avoid shell/quote
    // injection. `{{` / `}}` are literal R braces inside the format string.
    let data_lit = r_escape(&data_path.to_string_lossy());
    let out_lit = r_escape(&out_path.to_string_lossy());
    let diag_lit = r_escape(&diag_path.to_string_lossy());
    let script = format!(
        r#".diag <- list(
  error = NULL,
  warnings = character(0),
  messages = character(0),
  call_stack = character(0)
)

# --- minimal JSON encoder (no jsonlite dependency) ---
.json_str <- function(s) {{
  if (is.null(s)) return("null")
  s <- as.character(s)
  s <- gsub("\\", "\\\\", s, fixed = TRUE)
  s <- gsub('"',  '\\"',  s, fixed = TRUE)
  s <- gsub("\n", "\\n",  s, fixed = TRUE)
  s <- gsub("\r", "\\r",  s, fixed = TRUE)
  s <- gsub("\t", "\\t",  s, fixed = TRUE)
  paste0('"', s, '"')
}}
.json_arr <- function(v) {{
  if (is.null(v) || length(v) == 0L) return("[]")
  paste0("[", paste(vapply(v, .json_str, character(1)), collapse = ","), "]")
}}

tryCatch(
  withCallingHandlers({{
    library(arrow)
    library(ggplot2)
    df <- as.data.frame(arrow::read_ipc_stream("{data_lit}"))
{r_code}
    if (!exists("p")) {{
      stop("plot code must assign the ggplot object to a variable named `p`")
    }}
    ggsave("{out_lit}", plot = p, device = png,
           width = {width}, height = {height}, dpi = {dpi}, units = "in")
  }},
  warning = function(w) {{
    .diag$warnings <<- c(.diag$warnings, conditionMessage(w))
    invokeRestart("muffleWarning")
  }},
  message = function(m) {{
    .diag$messages <<- c(.diag$messages, conditionMessage(m))
    invokeRestart("muffleMessage")
  }}),
  error = function(e) {{
    .diag$error <<- conditionMessage(e)
    calls <- sys.calls()
    if (length(calls) > 0L) {{
      n <- length(calls)
      keep <- if (n >= 20L) seq.int(from = n - 19L, to = n) else seq_len(n)
      .diag$call_stack <<- vapply(
        calls[keep],
        function(c) paste(deparse(c), collapse = " "),
        character(1)
      )
    }}
  }}
)

# Serialize diagnostics. The script always exits 0 — the Rust side reads this
# file, not the subprocess exit code.
.diag_json <- paste0("{{",
  '"error":',     if (is.null(.diag$error)) "null" else .json_str(.diag$error), ",",
  '"warnings":',  .json_arr(.diag$warnings),  ",",
  '"messages":',  .json_arr(.diag$messages),  ",",
  '"call_stack":', .json_arr(.diag$call_stack),
"}}")
cat(.diag_json, file = "{diag_lit}")
"#
    );
    std::fs::write(&script_path, script)?;

    let mut cmd = tokio::process::Command::new(&rscript);
    cmd.arg(&script_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(VizError::RscriptNotFound);
        }
        Err(e) => return Err(e.into()),
    };

    let output = match tokio::time::timeout(
        Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        child.wait_with_output(),
    )
    .await
    {
        Ok(o) => o?,
        Err(_) => {
            return Err(VizError::RscriptTimeout(DEFAULT_TIMEOUT_SECS));
        }
    };

    // Read diagnostics.json — the structured source of truth. If the file is
    // missing or unparseable (shouldn't happen unless R itself is broken), we
    // fall back to an empty Diagnostics and rely on the exit-code/stderr path.
    let diag_bytes = std::fs::read(&diag_path).unwrap_or_default();
    let diagnostics: Diagnostics = if diag_bytes.is_empty() {
        Diagnostics::default()
    } else {
        serde_json::from_slice(&diag_bytes).unwrap_or_default()
    };

    // Two failure modes:
    // 1. Non-zero exit — unexpected by design (the script always exits 0).
    //    Fires on syntax errors in the generated script or R segfaults.
    // 2. Structured error in diagnostics.json — the normal R failure path.
    if !output.status.success() || diagnostics.error.is_some() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(VizError::RscriptFailed {
            code: output.status.code().unwrap_or(-1),
            stderr,
            diagnostics,
        });
    }

    // Read the rendered PNG back as bytes.
    let bytes = std::fs::read(&out_path)?;
    if bytes.is_empty() {
        return Err(VizError::InvalidPlotCode(
            "ggsave produced an empty file".to_string(),
        ));
    }

    // The tempdir (IPC + script + PNG + diagnostics) is removed when `tmp`
    // drops.
    drop(tmp);
    Ok((bytes, diagnostics))
}

/// Escape a path for safe interpolation into a double-quoted R string literal:
/// escape backslash and double-quote. Combined with the surrounding `"..."` in
/// the format string, this keeps arbitrary paths out of R's parser.
fn r_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Float64Array, Int32Array};
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    fn sample_batches() -> Vec<RecordBatch> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("x", DataType::Int32, false),
            Field::new("y", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5])),
                Arc::new(Float64Array::from(vec![1.0, 4.0, 9.0, 16.0, 25.0])),
            ],
        )
        .expect("build batch");
        vec![batch]
    }

    /// End-to-end: serialize batches, run ggplot2 via Rscript, check the PNG.
    /// Requires R + the `arrow`/`ggplot2` packages on PATH (the r45 conda env).
    #[tokio::test]
    async fn test_render_png_via_rscript() {
        let out = tempfile::NamedTempFile::new().unwrap().keep().unwrap().1;
        let code = "p <- ggplot(df, aes(x = x, y = y)) + geom_point() + geom_line()";

        let diag = render_png(
            &sample_batches(),
            code,
            &out,
            Some(6.0),
            Some(4.0),
            Some(100.0),
        )
        .await
        .expect("render should succeed");

        assert!(diag.is_ok(), "no fatal error expected: {diag:?}");

        let bytes = std::fs::read(&out).expect("read output png");
        assert!(bytes.len() > 100, "PNG too small");
        assert_eq!(
            &bytes[0..4],
            &[0x89, b'P', b'N', b'G'],
            "not a PNG signature"
        );
        eprintln!("render_png OK: {} bytes", bytes.len());
        let _ = std::fs::remove_file(&out);
    }

    /// Empty plot code is rejected before touching R.
    #[tokio::test]
    async fn test_empty_plot_code_rejected() {
        let out = tempfile::NamedTempFile::new().unwrap().keep().unwrap().1;
        let err = render_png(&sample_batches(), "   ", &out, None, None, None)
            .await
            .expect_err("empty code should fail");
        assert!(matches!(err, VizError::InvalidPlotCode(_)));
        let _ = std::fs::remove_file(&out);
    }

    /// Plot code that does not assign `p` surfaces an RscriptFailed error with
    /// a structured `diagnostics.error`.
    #[tokio::test]
    async fn test_missing_p_assignment_fails() {
        let out = tempfile::NamedTempFile::new().unwrap().keep().unwrap().1;
        let err = render_png(
            &sample_batches(),
            "ggplot(df, aes(x = x, y = y))",
            &out,
            None,
            None,
            None,
        )
        .await
        .expect_err("missing p should fail");
        match err {
            VizError::RscriptFailed { diagnostics, .. } => {
                let msg = diagnostics
                    .error
                    .as_ref()
                    .expect("diagnostics.error should be set");
                assert!(
                    msg.contains("assign")
                        || msg.contains("p")
                        || msg.contains("plot code"),
                    "error message should mention `p` assignment: {msg}"
                );
            }
            other => panic!("expected RscriptFailed, got {other:?}"),
        }
        let _ = std::fs::remove_file(&out);
    }

    /// R `warning()` calls are captured as structured diagnostics even though
    /// the render succeeds. The PNG is still produced.
    #[tokio::test]
    async fn test_warning_captured_as_diagnostics() {
        let out = tempfile::NamedTempFile::new().unwrap().keep().unwrap().1;
        let code = r#"
            warning("this is a test warning from R")
            p <- ggplot(df, aes(x = x, y = y)) + geom_point()
        "#;

        let diag = render_png(&sample_batches(), code, &out, None, None, None)
            .await
            .expect("render should succeed despite the warning");

        assert!(diag.is_ok(), "no fatal error: {diag:?}");
        assert!(
            diag.warnings
                .iter()
                .any(|w| w.contains("test warning")),
            "warning should be captured in diagnostics: {:?}",
            diag.warnings
        );

        let bytes = std::fs::read(&out).expect("png produced despite warning");
        assert_eq!(&bytes[0..4], &[0x89, b'P', b'N', b'G']);
        let _ = std::fs::remove_file(&out);
    }

    /// R `message()` calls are captured as structured diagnostics.
    #[tokio::test]
    async fn test_message_captured_as_diagnostics() {
        let out = tempfile::NamedTempFile::new().unwrap().keep().unwrap().1;
        let code = r#"
            message("hello from R message()")
            p <- ggplot(df, aes(x = x, y = y)) + geom_point()
        "#;

        let diag = render_png(&sample_batches(), code, &out, None, None, None)
            .await
            .expect("render should succeed");

        assert!(
            diag.messages
                .iter()
                .any(|m| m.contains("hello from R")),
            "message should be captured in diagnostics: {:?}",
            diag.messages
        );
        let _ = std::fs::remove_file(&out);
    }

    /// A fatal R error inside the caller's code produces a structured error in
    /// diagnostics, plus a populated call_stack.
    #[tokio::test]
    async fn test_fatal_error_captured() {
        let out = tempfile::NamedTempFile::new().unwrap().keep().unwrap().1;
        let code = r#"
            stop("deliberate fatal error for testing")
            p <- ggplot(df, aes(x = x, y = y)) + geom_point()
        "#;

        let err = render_png(&sample_batches(), code, &out, None, None, None)
            .await
            .expect_err("fatal error should fail the render");

        match err {
            VizError::RscriptFailed { diagnostics, .. } => {
                let msg = diagnostics.error.expect("error should be set");
                assert!(
                    msg.contains("deliberate fatal error"),
                    "error message should match: {msg}"
                );
                assert!(
                    !diagnostics.call_stack.is_empty(),
                    "call_stack should be populated on fatal error"
                );
            }
            other => panic!("expected RscriptFailed, got {other:?}"),
        }
        let _ = std::fs::remove_file(&out);
    }
}
