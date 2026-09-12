//! LaTeX compilation pipeline — pluggable engine + full document compilation.
//!
//! Provides a [`LatexEngine`] trait with a subprocess-based [`XelatexEngine`]
//! backend, a [`NullEngine`] for pipeline testing, a LaTeX log parser, and a
//! high-level [`compile_document`] function that orchestrates the full
//! pipeline: AST → .tex → .bib → compile → PDF.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use bib_types::Article;
use tokio::fs;
use tokio::process::Command;

use writing_types::Document;

use crate::citation::CitationResolver;
use crate::serialize::render_document;
use crate::{Error, Result};

// ===========================================================================
// Types
// ===========================================================================

/// Input for a LaTeX compilation.
#[derive(Debug, Clone)]
pub struct CompileInput {
    /// Main `.tex` file content.
    pub main_tex: String,
    /// `.bib` file content (written as `references.bib`).
    pub bib_content: Option<String>,
    /// Auxiliary files: `(relative_path, content)` pairs.
    pub auxiliary_files: Vec<(String, String)>,
    /// Working directory. `None` = create a fresh temp directory.
    pub working_dir: Option<PathBuf>,
    /// Main file name (default `main.tex`).
    pub main_filename: String,
    /// Force a specific engine (overrides auto-detection).
    pub force_engine: Option<String>,
}

impl Default for CompileInput {
    fn default() -> Self {
        Self {
            main_tex: String::new(),
            bib_content: None,
            auxiliary_files: Vec::new(),
            working_dir: None,
            main_filename: "main.tex".into(),
            force_engine: None,
        }
    }
}

/// Output of a LaTeX compilation.
#[derive(Debug, Clone)]
pub struct CompileOutput {
    /// Whether compilation produced a PDF without fatal errors.
    pub success: bool,
    /// PDF file bytes (if successful).
    pub pdf_bytes: Option<Vec<u8>>,
    /// Raw `.log` file content.
    pub log: String,
    /// Parsed warnings.
    pub warnings: Vec<CompileIssue>,
    /// Parsed errors.
    pub errors: Vec<CompileIssue>,
    /// Number of pages (if extractable from log).
    pub pages: Option<usize>,
    /// All output files (relative_path, bytes).
    pub output_files: Vec<(String, Vec<u8>)>,
    /// Which engine was used.
    pub engine_name: String,
}

impl CompileOutput {
    /// Whether there are any errors.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Whether there are any warnings.
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }
}

/// A single compile issue (error or warning) from the log.
#[derive(Debug, Clone)]
pub struct CompileIssue {
    pub severity: IssueSeverity,
    pub message: String,
    pub line: Option<usize>,
    pub file: Option<String>,
}

/// Issue severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueSeverity {
    Error,
    Warning,
    Info,
}

// ===========================================================================
// LatexEngine trait
// ===========================================================================

/// A LaTeX compilation engine.
#[async_trait]
pub trait LatexEngine: Send + Sync {
    /// Compile the input and return the output.
    async fn compile(&self, input: &CompileInput) -> Result<CompileOutput>;

    /// Engine name (e.g. `"xelatex"`, `"pdflatex"`).
    fn name(&self) -> &'static str;

    /// Whether the engine is available on this system.
    fn is_available(&self) -> bool;
}

// ===========================================================================
// XelatexEngine — subprocess-based
// ===========================================================================

/// XeLaTeX/pdflatex subprocess engine.
///
/// Runs the LaTeX binary as a subprocess in a working directory. Supports
/// both `xelatex` (default, CJK-capable) and `pdflatex` (lighter).
pub struct XelatexEngine {
    binary: String,
    /// Number of LaTeX passes (default 3 for cross-references + citations).
    passes: usize,
    /// Whether to run bibtex between passes.
    run_bibtex: bool,
}

impl XelatexEngine {
    /// Create a new XeLaTeX engine.
    pub fn new() -> Self {
        Self {
            binary: "xelatex".into(),
            passes: 3,
            run_bibtex: true,
        }
    }

    /// Use `pdflatex` instead of `xelatex`.
    pub fn pdflatex() -> Self {
        Self {
            binary: "pdflatex".into(),
            passes: 3,
            run_bibtex: true,
        }
    }

    /// Set the number of compilation passes.
    pub fn with_passes(mut self, passes: usize) -> Self {
        self.passes = passes;
        self
    }

    /// Disable bibtex (for documents without citations).
    pub fn without_bibtex(mut self) -> Self {
        self.run_bibtex = false;
        self
    }

    /// Check if the binary exists on the system.
    fn binary_exists(&self) -> bool {
        std::process::Command::new(&self.binary)
            .arg("--version")
            .output()
            .is_ok()
    }
}

impl Default for XelatexEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LatexEngine for XelatexEngine {
    fn name(&self) -> &'static str {
        if self.binary == "pdflatex" {
            "pdflatex"
        } else {
            "xelatex"
        }
    }

    fn is_available(&self) -> bool {
        self.binary_exists()
    }

    async fn compile(&self, input: &CompileInput) -> Result<CompileOutput> {
        if !self.is_available() {
            return Err(Error::Other(format!(
                "LaTeX engine '{}' not found on this system",
                self.binary
            )));
        }

        // Create working directory.
        let work_dir = match &input.working_dir {
            Some(dir) => dir.clone(),
            None => {
                let id = uuid_like();
                let dir = std::env::temp_dir().join(format!("writing-compile-{id}"));
                fs::create_dir_all(&dir).await?;
                dir
            }
        };

        // Write main .tex file.
        let main_path = work_dir.join(&input.main_filename);
        fs::write(&main_path, &input.main_tex).await?;

        // Write .bib file.
        if let Some(ref bib) = input.bib_content {
            let bib_path = work_dir.join("references.bib");
            fs::write(&bib_path, bib).await?;
        }

        // Write auxiliary files.
        for (rel_path, content) in &input.auxiliary_files {
            let path = work_dir.join(rel_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(&path, content).await?;
        }

        // Run LaTeX passes.
        let jobname = Path::new(&input.main_filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("main");

        let mut last_log = String::new();
        let mut pdf_bytes = None;
        let mut had_error = false;

        for pass in 0..self.passes {
            let result = Command::new(&self.binary)
                .arg("-interaction=nonstopmode")
                .arg("-halt-on-error")
                .arg("-file-line-error")
                .arg(&input.main_filename)
                .current_dir(&work_dir)
                .output()
                .await
                .map_err(|e| Error::Other(format!("failed to run {}: {e}", self.binary)))?;

            // Read log.
            let log_path = work_dir.join(format!("{jobname}.log"));
            if log_path.exists() {
                last_log = fs::read_to_string(&log_path).await.unwrap_or_default();
            }

            // Check for PDF.
            let pdf_path = work_dir.join(format!("{jobname}.pdf"));
            if pdf_path.exists() {
                pdf_bytes = Some(fs::read(&pdf_path).await.unwrap_or_default());
            }

            // Run bibtex after first pass.
            if self.run_bibtex && pass == 0 && input.bib_content.is_some() {
                let _ = Command::new("bibtex")
                    .arg(jobname)
                    .current_dir(&work_dir)
                    .output()
                    .await;
            }

            // Check exit status — with nonstopmode, LaTeX may exit non-zero
            // but still produce output.
            if !result.status.success() && pdf_bytes.is_none() {
                had_error = true;
                break;
            }
        }

        // Parse log.
        let warnings = parse_warnings(&last_log);
        let errors = parse_errors(&last_log);
        let pages = parse_page_count(&last_log);

        // Collect output files.
        let mut output_files = Vec::new();
        if let Some(ref pdf) = pdf_bytes {
            output_files.push((format!("{jobname}.pdf"), pdf.clone()));
        }

        // Clean up temp dir if we created it.
        if input.working_dir.is_none() {
            let _ = fs::remove_dir_all(&work_dir).await;
        }

        Ok(CompileOutput {
            success: pdf_bytes.is_some() && !had_error,
            pdf_bytes,
            log: last_log,
            warnings,
            errors,
            pages,
            output_files,
            engine_name: self.name().into(),
        })
    }
}

// ===========================================================================
// NullEngine — for testing the pipeline without LaTeX
// ===========================================================================

/// A no-op engine that validates the pipeline without compiling.
///
/// Useful for testing and environments without LaTeX installed.
pub struct NullEngine;

#[async_trait]
impl LatexEngine for NullEngine {
    fn name(&self) -> &'static str {
        "null"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn compile(&self, input: &CompileInput) -> Result<CompileOutput> {
        // Validate that we have content.
        if input.main_tex.is_empty() {
            return Err(Error::Other("main_tex is empty".into()));
        }

        // Validate basic LaTeX structure.
        if !input.main_tex.contains("\\documentclass") {
            return Err(Error::Other(
                "main_tex does not contain \\documentclass".into(),
            ));
        }
        if !input.main_tex.contains("\\begin{document}") {
            return Err(Error::Other(
                "main_tex does not contain \\begin{document}".into(),
            ));
        }
        if !input.main_tex.contains("\\end{document}") {
            return Err(Error::Other(
                "main_tex does not contain \\end{document}".into(),
            ));
        }

        // Simulate output.
        let errors = if input.bib_content.is_none() && input.main_tex.contains("\\bibliography{") {
            vec![CompileIssue {
                severity: IssueSeverity::Warning,
                message: "No .bib content provided but \\bibliography is used".into(),
                line: None,
                file: None,
            }]
        } else {
            vec![]
        };

        Ok(CompileOutput {
            success: true,
            pdf_bytes: None, // NullEngine never produces PDF.
            log: "NullEngine: pipeline validated successfully.".into(),
            warnings: Vec::new(),
            errors,
            pages: None,
            output_files: Vec::new(),
            engine_name: "null".into(),
        })
    }
}

// ===========================================================================
// Engine selection
// ===========================================================================

/// Select the best available engine.
///
/// Prefers XeLaTeX (CJK-capable), falls back to NullEngine.
pub fn default_engine() -> Box<dyn LatexEngine> {
    let xelatex = XelatexEngine::new();
    if xelatex.is_available() {
        return Box::new(xelatex);
    }
    Box::new(NullEngine)
}

// ===========================================================================
// Full compilation pipeline
// ===========================================================================

/// Full document compilation pipeline.
///
/// 1. Serialize the Document AST → `main.tex`
/// 2. Resolve citations → `references.bib`
/// 3. Assemble the compilation input
/// 4. Run the engine
pub async fn compile_document(
    doc: &Document,
    resolver: Option<&CitationResolver>,
    engine: &dyn LatexEngine,
) -> Result<CompileOutput> {
    // 1. Serialize.
    let rendered = render_document(doc);

    // 2. Generate .bib if we have a resolver.
    let bib_content = if let Some(resolver) = resolver {
        let bib = resolver.generate_bib(doc).await;
        if bib.trim().is_empty() || bib.starts_with("% Auto-generated") && !bib.contains("@article")
        {
            // No real entries — don't include .bib to avoid bibtex errors.
            None
        } else {
            Some(bib)
        }
    } else {
        None
    };

    // 3. Assemble input.
    let input = CompileInput {
        main_tex: rendered.main_tex,
        bib_content,
        ..Default::default()
    };

    // 4. Compile.
    engine.compile(&input).await
}

// ===========================================================================
// Log parser
// ===========================================================================

/// Parse LaTeX log for errors.
fn parse_errors(log: &str) -> Vec<CompileIssue> {
    let mut errors = Vec::new();

    for line in log.lines() {
        let trimmed = line.trim();

        // Standard format: starts with "!"
        // File-line-error format: "./main.tex:42: ! Error" or "./main.tex:42: LaTeX Error: ..."
        // Also: "LaTeX Error:" or "Emergency stop" anywhere in line.
        let is_error = trimmed.starts_with('!')
            || (trimmed.contains(".tex:") && trimmed.contains('!'))
            || trimmed.contains("LaTeX Error:")
            || trimmed.contains("Emergency stop")
            || trimmed.contains("! ");

        if is_error {
            let (file, line_num) = parse_file_line_prefix(trimmed);
            let message = if trimmed.starts_with('!') {
                trimmed.trim_start_matches('!').trim().to_string()
            } else {
                // Extract message after the last "!" or after "LaTeX Error:".
                if let Some(pos) = trimmed.rfind("! ") {
                    trimmed[pos + 2..].trim().to_string()
                } else if let Some(pos) = trimmed.find("LaTeX Error:") {
                    trimmed[pos..].trim().to_string()
                } else {
                    trimmed.to_string()
                }
            };

            errors.push(CompileIssue {
                severity: IssueSeverity::Error,
                message,
                line: line_num,
                file,
            });
        }
    }

    errors
}

/// Parse `./main.tex:42:` prefix to extract filename and line number.
fn parse_file_line_prefix(line: &str) -> (Option<String>, Option<usize>) {
    // Look for pattern: path:number:
    let parts: Vec<&str> = line.splitn(3, ':').collect();
    if parts.len() >= 2 {
        let file = parts[0].trim().to_string();
        if !file.is_empty() && file != "!" {
            let line_num = parts[1].trim().parse::<usize>().ok();
            return (Some(file), line_num);
        }
    }
    (None, None)
}

/// Parse LaTeX log for warnings.
fn parse_warnings(log: &str) -> Vec<CompileIssue> {
    let mut warnings = Vec::new();

    for line in log.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("LaTeX Warning:") || trimmed.starts_with("Package natbib Warning:") {
            warnings.push(CompileIssue {
                severity: IssueSeverity::Warning,
                message: trimmed.to_string(),
                line: extract_line_number(trimmed),
                file: None,
            });
        }

        if trimmed.starts_with("Overfull \\hbox") || trimmed.starts_with("Underfull \\hbox") {
            warnings.push(CompileIssue {
                severity: IssueSeverity::Warning,
                message: trimmed.to_string(),
                line: None,
                file: None,
            });
        }
    }

    warnings
}

/// Extract a line number from a string like "on input line 42".
fn extract_line_number(s: &str) -> Option<usize> {
    if let Some(pos) = s.find("line ") {
        let rest = &s[pos + 5..];
        let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        num.parse().ok()
    } else {
        None
    }
}

/// Parse the total page count from the log.
fn parse_page_count(log: &str) -> Option<usize> {
    // Look for "Output written on main.pdf (5 pages, ...)".
    if let Some(pos) = log.find("Output written on") {
        let rest = &log[pos..];
        if let Some(p) = rest.find("(") {
            let after_paren = &rest[p + 1..];
            let num: String = after_paren
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            return num.parse().ok();
        }
    }
    None
}

/// Generate a short unique ID (for temp directory names).
fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Log parser tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_errors_basic() {
        let log = r#"
This is XeTeX, Version 3.141592653-2.6-0.999998 (TeX Live 2026/Arch Linux)
./main.tex:10: ! Undefined control sequence.
l.10 \nonexistent
                  command
./main.tex:15: ! Missing $ inserted.
"#;
        let errors = parse_errors(log);
        assert!(errors.len() >= 2);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Undefined control sequence"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Missing $ inserted"))
        );
    }

    #[test]
    fn parse_errors_file_line_format() {
        let log = "./main.tex:42: ! LaTeX Error: Environment example undefined.";
        let errors = parse_errors(log);
        assert!(!errors.is_empty());
        let e = &errors[0];
        assert!(e.message.contains("Environment example undefined"));
        assert_eq!(e.line, Some(42));
    }

    #[test]
    fn parse_warnings_basic() {
        let log = r#"
LaTeX Warning: Reference `fig:nonexistent' on page 3 undefined on input line 45.
Package natbib Warning: Citation `smith2024' on page 1 undefined on input line 10.
Overfull \hbox (10.0pt too wide) in paragraph at lines 5--10
Underfull \hbox (badness 10000) in paragraph at lines 20--25
"#;
        let warnings = parse_warnings(log);
        assert!(
            warnings
                .iter()
                .any(|w| w.message.contains("fig:nonexistent"))
        );
        assert!(warnings.iter().any(|w| w.message.contains("smith2024")));
        assert!(warnings.iter().any(|w| w.message.starts_with("Overfull")));
        assert!(warnings.iter().any(|w| w.message.starts_with("Underfull")));
    }

    #[test]
    fn parse_page_count_basic() {
        let log = "Output written on main.pdf (5 pages, 123456 bytes).";
        assert_eq!(parse_page_count(log), Some(5));

        let log2 = "Output written on main.pdf (1 page, 1000 bytes).";
        assert_eq!(parse_page_count(log2), Some(1));

        let log3 = "No output.";
        assert_eq!(parse_page_count(log3), None);
    }

    #[test]
    fn extract_line_number_from_string() {
        assert_eq!(extract_line_number("on input line 42"), Some(42));
        assert_eq!(extract_line_number("on page 3 on input line 15"), Some(15));
        assert_eq!(extract_line_number("no line number"), None);
    }

    #[test]
    fn parse_file_line_prefix_extracts_both() {
        let (file, line) = parse_file_line_prefix("./main.tex:42: ! Error");
        assert_eq!(file.as_deref(), Some("./main.tex"));
        assert_eq!(line, Some(42));
    }

    // -----------------------------------------------------------------------
    // NullEngine tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn null_engine_validates_basic_structure() {
        let engine = NullEngine;
        let input = CompileInput {
            main_tex: r#"\documentclass{article}
\begin{document}
Hello world.
\end{document}
"#
            .into(),
            ..Default::default()
        };
        let output = engine.compile(&input).await.unwrap();
        assert!(output.success);
        assert_eq!(output.engine_name, "null");
    }

    #[tokio::test]
    async fn null_engine_rejects_empty() {
        let engine = NullEngine;
        let input = CompileInput::default();
        assert!(engine.compile(&input).await.is_err());
    }

    #[tokio::test]
    async fn null_engine_rejects_missing_documentclass() {
        let engine = NullEngine;
        let input = CompileInput {
            main_tex: "\\begin{document}\\end{document}".into(),
            ..Default::default()
        };
        assert!(engine.compile(&input).await.is_err());
    }

    #[tokio::test]
    async fn null_engine_warns_missing_bib() {
        let engine = NullEngine;
        let input = CompileInput {
            main_tex: r#"\documentclass{article}
\begin{document}
\bibliography{references}
\end{document}
"#
            .into(),
            bib_content: None,
            ..Default::default()
        };
        let output = engine.compile(&input).await.unwrap();
        assert!(!output.errors.is_empty()); // warning about missing .bib
    }

    // -----------------------------------------------------------------------
    // XelatexEngine availability
    // -----------------------------------------------------------------------

    #[test]
    fn xelatex_engine_availability() {
        let engine = XelatexEngine::new();
        if !engine.is_available() {
            return;
        }
        assert!(engine.is_available());
        assert_eq!(engine.name(), "xelatex");
    }

    #[test]
    fn pdflatex_engine_availability() {
        let engine = XelatexEngine::pdflatex();
        if !engine.is_available() {
            return;
        }
        assert_eq!(engine.name(), "pdflatex");
    }

    #[test]
    fn default_engine_selects_available() {
        let engine = default_engine();
        if XelatexEngine::new().is_available() {
            assert!(engine.is_available());
            assert_eq!(engine.name(), "xelatex");
        } else {
            assert_eq!(engine.name(), "null");
        }
    }

    // -----------------------------------------------------------------------
    // Full pipeline tests (NullEngine)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn compile_document_with_null_engine() {
        use writing_types::*;

        let mut doc = Document::new("d1", "Test Document");
        doc.metadata.abstract_text = Some("A test abstract.".into());
        let mut sec = Section::new(SectionLevel::Section, "Introduction");
        sec.blocks.push(Block::Paragraph(ParagraphBlock::text(
            "This is a test paragraph.",
        )));
        doc.root.children.push(sec);

        let engine = NullEngine;
        let output = compile_document(&doc, None, &engine).await.unwrap();
        assert!(output.success);
        assert_eq!(output.engine_name, "null");
    }

    #[tokio::test]
    async fn compile_document_with_citations_null() {
        use bib_base::BibBase;
        use bib_types::Article;
        use writing_types::*;

        let bib = std::sync::Arc::new(BibBase::open_in_memory().await.unwrap());

        let mut a1 = Article::new("art-1", "A breakthrough in genomics");
        a1.authors.push(bib_types::Author {
            last_name: "Smith".into(),
            fore_name: Some("J".into()),
            initials: Some("J".into()),
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
        a1.year = Some(2024);
        a1.journal = Some("Nature".into());
        bib.upsert_article(&a1).await.unwrap();

        let resolver = CitationResolver::new(bib);

        let mut doc = Document::new("d1", "Cited Paper");
        let mut sec = Section::new(SectionLevel::Section, "Intro");
        sec.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
            Inline::text("Prior work "),
            Inline::Citation(CitationCluster {
                keys: vec![CiteKey::new("smith2024a")],
                style: CitationStyle::Parenthetical,
                prefix: None,
                suffix: None,
                claim_id: None,
            }),
            Inline::text(" was important."),
        ])));
        doc.root.children.push(sec);

        let engine = NullEngine;
        let output = compile_document(&doc, Some(&resolver), &engine)
            .await
            .unwrap();
        assert!(output.success);
    }

    // -----------------------------------------------------------------------
    // Real XeLaTeX compilation (only if available)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn xelatex_compiles_simple_document() {
        let engine = XelatexEngine::new().without_bibtex().with_passes(1);
        if !engine.is_available() {
            eprintln!("skipping: xelatex not available");
            return;
        }

        let input = CompileInput {
            main_tex: r#"\documentclass{article}
\usepackage{amsmath}
\title{Test}
\author{Test Author}
\date{\today}
\begin{document}
\maketitle
\section{Introduction}
This is a test document with an equation:
\begin{equation}
E = mc^2
\end{equation}
\end{document}
"#
            .into(),
            ..Default::default()
        };

        let output = engine.compile(&input).await.unwrap();
        assert!(output.success, "compilation failed: {}", output.log);
        assert!(output.pdf_bytes.is_some());
        assert!(output.pdf_bytes.as_ref().unwrap().len() > 1000); // Real PDF.
        assert!(output.pages.unwrap_or(0) >= 1);
    }

    #[tokio::test]
    async fn xelatex_compiles_with_table() {
        let engine = XelatexEngine::new().without_bibtex().with_passes(1);
        if !engine.is_available() {
            return;
        }

        let input = CompileInput {
            main_tex: r#"\documentclass{article}
\usepackage{booktabs}
\begin{document}
\begin{table}[h]
\centering
\caption{Test table.}
\begin{tabular}{lcc}
\toprule
Method & Beta & P-value \\
\midrule
IVW & 0.42 & 0.001 \\
Egger & 0.38 & 0.005 \\
\bottomrule
\end{tabular}
\end{table}
\end{document}
"#
            .into(),
            ..Default::default()
        };

        let output = engine.compile(&input).await.unwrap();
        assert!(output.success, "table compilation failed: {}", output.log);
        assert!(output.pdf_bytes.is_some());
    }

    #[tokio::test]
    async fn xelatex_detects_errors() {
        let engine = XelatexEngine::new().without_bibtex().with_passes(1);
        if !engine.is_available() {
            return;
        }

        // Undefined command should cause an error.
        let input = CompileInput {
            main_tex: r#"\documentclass{article}
\begin{document}
\nonexistentcommand
\end{document}
"#
            .into(),
            ..Default::default()
        };

        let result = engine.compile(&input).await;
        // Should either error or produce output with errors.
        match result {
            Ok(output) => {
                // With halt-on-error, xelatex exits non-zero, no PDF.
                assert!(!output.success || output.has_errors());
            }
            Err(_) => {
                // Also acceptable — compilation failed.
            }
        }
    }
}
