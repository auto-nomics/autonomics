//! Pluggable text extraction from document content.
//!
//! [`TextExtractor`] abstracts over different extraction backends so that
//! the full-text storage pipeline can start with a simple built-in
//! extractor and later swap in more sophisticated options (docling,
//! Apache Tika, …) without changing call sites.
//!
//! ## Current backends
//!
//! | Format | Strategy                         |
//! |--------|----------------------------------|
//! | PDF    | `pdf-extract` crate (pure Rust)  |
//! | HTML   | tag stripping                    |
//! | TXT    | passthrough                      |
//!
//! [`OcrFallbackExtractor`] additionally tries the local `tesseract` command
//! when PDF extraction produces no text. Tesseract is optional; when absent,
//! callers can still retain the original file with an empty text column.
//!
//! ## Adding a backend
//!
//! Implement [`TextExtractor`] and register it where the full-text
//! pipeline is wired:
//!
//! ```no_run
//! # use async_trait::async_trait;
//! # use bib_types::FileFormat;
//! # use bib_base::extract::{ExtractedText, TextExtractor};
//! # use bib_base::error::Result;
//! struct DoclingExtractor;
//!
//! #[async_trait]
//! impl TextExtractor for DoclingExtractor {
//!     fn name(&self) -> &'static str { "docling" }
//!     async fn extract(&self, content: &[u8], format: FileFormat) -> Result<ExtractedText> {
//!         // call docling HTTP API …
//!         # unimplemented!()
//!     }
//! }
//! ```

use async_trait::async_trait;
use bib_types::{FileFormat, TextFormat};
use tokio::io::AsyncWriteExt;
use tokio::process::{ChildStdin, Command};

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Panic guard
// ---------------------------------------------------------------------------

// Thread-local flag set while running third-party extraction code whose
// panics are expected and recoverable.
//
// `pdf-extract` signals "unusable but not our bug" input problems — e.g.
// `missing unicode map and encoding` for simple fonts without a ToUnicode
// map — by panicking instead of returning `Err`. [`with_panic_guard`]
// converts those panics into ordinary errors so the OCR fallback chain can
// take over. The TUI's panic hooks consult [`is_expected_panic`] so such a
// panic neither restores the terminal nor kills the render loop.
thread_local! {
    static EXPECTED_PANIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the current thread panicked inside a [`with_panic_guard`] call.
///
/// Read by panic hooks to suppress fatal handling of recoverable
/// extraction panics. Always `false` on threads not running extraction.
pub fn is_expected_panic() -> bool {
    EXPECTED_PANIC.with(std::cell::Cell::get)
}

/// Run `f`, converting a panic into `Err(message)`.
///
/// The guard flag is set for the duration of `f` and cleared on every exit
/// path, so a panic hook firing mid-unwind observes it, while later code
/// (including subsequent extractions on the same pooled thread) does not.
fn with_panic_guard<T>(f: impl FnOnce() -> T) -> std::result::Result<T, String> {
    EXPECTED_PANIC.with(|flag| flag.set(true));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    EXPECTED_PANIC.with(|flag| flag.set(false));
    result.map_err(|payload| {
        if let Some(msg) = payload.downcast_ref::<&str>() {
            (*msg).to_string()
        } else if let Some(msg) = payload.downcast_ref::<String>() {
            msg.clone()
        } else {
            "unknown panic payload".to_string()
        }
    })
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A figure image extracted alongside the markdown text.
///
/// `name` is the bare file name referenced from the markdown
/// (`![](images/<name>.jpg)`); MinerU derives it from the image content,
/// so it is stable across re-extractions of the same PDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedImage {
    pub name: String,
    pub data: Vec<u8>,
}

/// Plain text extracted from a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedText {
    /// The extracted text content.
    pub text: String,
    /// Format of [`ExtractedText::text`] — plain (whitespace collapsed)
    /// or markdown (layout-aware, whitespace preserved).
    pub format: TextFormat,
    /// Which extractor produced this text (for provenance columns).
    pub extractor: &'static str,
    /// Figure images carried by layout-aware extractors. Empty for the
    /// plain-text extractors.
    pub images: Vec<ExtractedImage>,
}

impl ExtractedText {
    /// Plain-text result from the built-in [`SimpleExtractor`].
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            format: TextFormat::Plain,
            extractor: "simple",
            images: Vec::new(),
        }
    }

    /// Markdown result from a layout-aware extractor (MinerU).
    pub fn markdown(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            format: TextFormat::Markdown,
            extractor: "mineru",
            images: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// TextExtractor trait
// ---------------------------------------------------------------------------

/// Extract plain text from document binary content.
///
/// Implementations may use different strategies depending on the file
/// format: pure-Rust parsing, external subprocesses, HTTP microservices,
/// etc. The trait is async so that HTTP-based backends fit naturally.
#[async_trait]
pub trait TextExtractor: Send + Sync {
    /// Identifier for logging / debugging (e.g. `"simple"`, `"docling"`).
    fn name(&self) -> &'static str;

    /// Extract plain text from `content` interpreted as `format`.
    async fn extract(&self, content: &[u8], format: FileFormat) -> Result<ExtractedText>;
}

// ---------------------------------------------------------------------------
// SimpleExtractor — built-in fallback
// ---------------------------------------------------------------------------

/// Built-in text extractor using pure-Rust libraries.
///
/// - **PDF**: [`pdf_extract`] (decodes content streams, concatenates text).
/// - **HTML**: strips tags, preserves text content.
/// - **TXT**: UTF-8 passthrough (lossy on invalid bytes).
///
/// Suitable for text-based PDFs. Scanned PDFs (image-only) will produce
/// empty or near-empty output — for those, use an OCR-capable backend.
pub struct SimpleExtractor;

impl SimpleExtractor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SimpleExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// PDF extractor that falls back to an optional local Tesseract installation.
///
/// The command interface is fixed (`tesseract - stdout`) and document bytes are
/// passed through stdin, so uploaded filenames never reach the subprocess.
pub struct OcrFallbackExtractor {
    simple: SimpleExtractor,
}

impl OcrFallbackExtractor {
    pub fn new() -> Self {
        Self {
            simple: SimpleExtractor::new(),
        }
    }
}

impl Default for OcrFallbackExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TextExtractor for SimpleExtractor {
    fn name(&self) -> &'static str {
        "simple"
    }

    async fn extract(&self, content: &[u8], format: FileFormat) -> Result<ExtractedText> {
        match format {
            FileFormat::Pdf => {
                // PDF extraction is CPU-bound — run on the blocking pool.
                // pdf-extract panics on malformed fonts; the guard converts
                // the panic to an Err so the OCR fallback can take over.
                let owned = content.to_vec();
                let text = tokio::task::spawn_blocking(move || {
                    with_panic_guard(|| pdf_extract::extract_text_from_mem(&owned))
                        .and_then(|inner| inner.map_err(|e| e.to_string()))
                })
                .await
                .map_err(|e| Error::Unknown(format!("extraction task panicked: {e}")))?
                .map_err(|e| Error::Unknown(format!("PDF extraction failed: {e}")))?;

                Ok(ExtractedText::new(normalize_whitespace(&text)))
            }
            FileFormat::Html => {
                let raw = String::from_utf8_lossy(content);
                Ok(ExtractedText::new(normalize_whitespace(&strip_html_tags(
                    &raw,
                ))))
            }
            FileFormat::Txt => {
                let raw = String::from_utf8_lossy(content);
                Ok(ExtractedText::new(raw.to_string()))
            }
        }
    }
}

#[async_trait]
impl TextExtractor for OcrFallbackExtractor {
    fn name(&self) -> &'static str {
        "simple+ocr-fallback"
    }

    async fn extract(&self, content: &[u8], format: FileFormat) -> Result<ExtractedText> {
        if format != FileFormat::Pdf {
            return self.simple.extract(content, format).await;
        }

        let simple = self.simple.extract(content, format).await;
        let needs_ocr = match &simple {
            Ok(text) => text.text.trim().is_empty(),
            Err(_) => true,
        };
        if !needs_ocr {
            return simple;
        }

        match run_tesseract(content.to_vec()).await {
            Ok(text) if !text.trim().is_empty() => Ok(ExtractedText {
                text: normalize_whitespace(&text),
                format: TextFormat::Plain,
                extractor: "simple+ocr-fallback",
                images: Vec::new(),
            }),
            _ => match simple {
                Ok(_text) => Err(Error::Unknown(
                    "PDF extraction and OCR yielded no text".into(),
                )),
                Err(error) => Err(error),
            },
        }
    }
}

async fn run_tesseract(content: Vec<u8>) -> std::result::Result<String, String> {
    let mut child = Command::new("tesseract")
        .arg("-")
        .arg("stdout")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .env("OMP_THREAD_LIMIT", "1")
        .spawn()
        .map_err(|error| format!("failed to start tesseract: {error}"))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "tesseract stdin was unavailable".to_owned())?;
    let writer = tokio::spawn(write_stdin(stdin, content));
    let output = child
        .wait_with_output()
        .await
        .map_err(|error| format!("failed to wait for tesseract: {error}"))?;
    writer
        .await
        .map_err(|error| format!("tesseract writer failed: {error}"))?
        .map_err(|error| format!("failed to send PDF to tesseract: {error}"))?;

    if !output.status.success() {
        return Err(format!("tesseract exited with {}", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn write_stdin(
    mut stdin: ChildStdin,
    content: Vec<u8>,
) -> std::result::Result<(), std::io::Error> {
    stdin.write_all(&content).await?;
    stdin.shutdown().await
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Remove HTML/XML tags, keeping only text content.
fn strip_html_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Collapse runs of whitespace into single spaces and trim.
fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::content::{Content, Operation};
    use lopdf::{Document, Object, Stream, dictionary};

    #[tokio::test]
    async fn ocr_fallback_uses_builtin_for_non_pdf() {
        let ext = OcrFallbackExtractor::new();
        let result = ext.extract(b"plain text", FileFormat::Txt).await.unwrap();
        assert_eq!(result.text, "plain text");
    }

    #[tokio::test]
    async fn extract_txt() {
        let ext = SimpleExtractor::new();
        let result = ext
            .extract(b"Hello, world!", FileFormat::Txt)
            .await
            .unwrap();
        assert_eq!(result.text, "Hello, world!");
    }

    #[tokio::test]
    async fn extract_html_strips_tags() {
        let ext = SimpleExtractor::new();
        let html = b"<html><body><h1>Title</h1><p>Hello <b>world</b>.</p></body></html>";
        let result = ext.extract(html, FileFormat::Html).await.unwrap();
        assert!(result.text.contains("Title"));
        assert!(result.text.contains("Hello world."));
        assert!(!result.text.contains("<"));
    }

    #[tokio::test]
    async fn extract_html_with_entities() {
        let ext = SimpleExtractor::new();
        let html = b"<p>5 &lt; 10 &amp; 20 &gt; 15</p>";
        let result = ext.extract(html, FileFormat::Html).await.unwrap();
        // Tag stripping preserves entity text literally.
        assert!(result.text.contains("5"));
    }

    #[tokio::test]
    async fn extract_empty_txt() {
        let ext = SimpleExtractor::new();
        let result = ext.extract(b"", FileFormat::Txt).await.unwrap();
        assert_eq!(result.text, "");
    }

    #[tokio::test]
    async fn extract_invalid_utf8_txt_is_lossy() {
        let ext = SimpleExtractor::new();
        // 0xFF is invalid UTF-8 — lossy conversion replaces with U+FFFD.
        let result = ext.extract(&[0xFF, 0xAB], FileFormat::Txt).await.unwrap();
        assert!(result.text.contains('\u{FFFD}'));
    }

    #[tokio::test]
    async fn extract_pdf_with_devicen_colorspace() {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();

        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
        });
        let tint_transform_id = doc.add_object(dictionary! {
            "FunctionType" => 2,
            "Domain" => vec![0.into(), 1.into(), 0.into(), 1.into(), 0.into(), 1.into()],
            "C0" => vec![0.into(), 0.into(), 0.into(), 0.into()],
            "C1" => vec![0.into(), 0.into(), 0.into(), 0.into()],
            "N" => 1,
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! {
                "F1" => font_id,
            },
            "ColorSpace" => dictionary! {
                "CS0" => Object::Array(vec![
                    Object::Name(b"DeviceN".to_vec()),
                    Object::Array(vec![
                        Object::Name(b"Cyan".to_vec()),
                        Object::Name(b"Magenta".to_vec()),
                        Object::Name(b"Yellow".to_vec()),
                    ]),
                    Object::Name(b"DeviceCMYK".to_vec()),
                    Object::Reference(tint_transform_id),
                ]),
            },
        });

        let content = Content {
            operations: vec![
                Operation::new("cs", vec![Object::Name(b"CS0".to_vec())]),
                Operation::new("sc", vec![0.into(), 0.into(), 0.into()]),
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Tj", vec![Object::string_literal("DeviceN text")]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(
            dictionary! {},
            content.encode().expect("PDF content should encode"),
        ));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Contents" => content_id,
        });
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

        let mut pdf = Vec::new();
        doc.save_to(&mut pdf)
            .expect("generated PDF should serialize");

        let result = SimpleExtractor::new()
            .extract(&pdf, FileFormat::Pdf)
            .await
            .expect("DeviceN PDF should not panic or fail");
        assert_eq!(result.text, "DeviceN text");
    }

    #[test]
    fn panic_guard_converts_static_str_panic_to_err() {
        let result = with_panic_guard(|| panic!("boom"));
        assert_eq!(result.unwrap_err(), "boom");
        // Flag must be cleared on the panic exit path.
        assert!(!is_expected_panic());
    }

    #[test]
    fn panic_guard_converts_formatted_panic_to_err() {
        let result = with_panic_guard(|| panic!("code {}", 42));
        assert_eq!(result.unwrap_err(), "code 42");
        assert!(!is_expected_panic());
    }

    #[test]
    fn panic_guard_passes_through_ok() {
        let result = with_panic_guard(|| 7);
        assert_eq!(result.unwrap(), 7);
        assert!(!is_expected_panic());
    }

    #[tokio::test]
    async fn extract_garbage_pdf_returns_err_without_poisoning() {
        let ext = SimpleExtractor::new();
        // Not a real PDF — pdf-extract errors (or panics via the guard);
        // either way the caller sees Err and the guard flag stays clear.
        let result = ext
            .extract(b"%PDF-1.7 not a real pdf", FileFormat::Pdf)
            .await;
        assert!(result.is_err());
        assert!(!is_expected_panic());
    }

    #[test]
    fn strip_html_basic() {
        assert_eq!(strip_html_tags("<b>bold</b>"), "bold");
        assert_eq!(strip_html_tags("no tags"), "no tags");
        assert_eq!(strip_html_tags("<a href='x'>link</a>"), "link");
    }

    #[test]
    fn normalize_whitespace_collapses() {
        assert_eq!(normalize_whitespace("  hello   world  "), "hello world");
        assert_eq!(normalize_whitespace("a\n\tb\n\nc"), "a b c");
        assert_eq!(normalize_whitespace(""), "");
    }
}
