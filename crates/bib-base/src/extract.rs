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
use bib_types::FileFormat;

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Plain text extracted from a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedText {
    /// The extracted text content.
    pub text: String,
}

impl ExtractedText {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
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

#[async_trait]
impl TextExtractor for SimpleExtractor {
    fn name(&self) -> &'static str {
        "simple"
    }

    async fn extract(&self, content: &[u8], format: FileFormat) -> Result<ExtractedText> {
        match format {
            FileFormat::Pdf => {
                // PDF extraction is CPU-bound — run on the blocking pool.
                let owned = content.to_vec();
                let text =
                    tokio::task::spawn_blocking(move || pdf_extract::extract_text_from_mem(&owned))
                        .await
                        .map_err(|e| Error::Unknown(format!("extraction task panicked: {e}")))?
                        .map_err(|e| Error::Unknown(format!("PDF extraction failed: {e}")))?;

                Ok(ExtractedText {
                    text: normalize_whitespace(&text),
                })
            }
            FileFormat::Html => {
                let raw = String::from_utf8_lossy(content);
                Ok(ExtractedText {
                    text: normalize_whitespace(&strip_html_tags(&raw)),
                })
            }
            FileFormat::Txt => {
                let raw = String::from_utf8_lossy(content);
                Ok(ExtractedText {
                    text: raw.to_string(),
                })
            }
        }
    }
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
