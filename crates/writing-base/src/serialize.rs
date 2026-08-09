//! LaTeX serialisation — render the document AST into compilable `.tex` source.
//!
//! The serialiser is a recursive descent renderer that walks the [`Document`]
//! tree and produces a single `.tex` string.  It does not compile anything;
//! that is the job of the compile module (Phase 3).

use writing_types::{
    Block, CitationStyle, ColumnAlign, Document, FigureSource, Inline, ListMarker, Placement,
    Section, SectionLevel, TableFormat, TableSource,
};

// ---------------------------------------------------------------------------
// Render entry point
// ---------------------------------------------------------------------------

/// Rendered document output.
#[derive(Debug, Clone)]
pub struct RenderedDocument {
    /// The complete `.tex` file content (documentclass + preamble + body).
    pub main_tex: String,
}

/// Render a [`Document`] into a complete `.tex` file string.
pub fn render_document(doc: &Document) -> RenderedDocument {
    let mut out = String::with_capacity(8192);

    // --- documentclass ---
    let class = doc.document_class.class_name();
    out.push_str(&format!("\\documentclass{{{class}}}\n\n"));

    // --- required packages (always) ---
    out.push_str("\\usepackage{graphicx}\n");
    out.push_str("\\usepackage{amsmath}\n");
    out.push_str("\\usepackage{booktabs}\n");
    out.push_str("\\usepackage{hyperref}\n");

    // --- citation package ---
    out.push_str("\\usepackage{natbib}\n");
    out.push_str("\\bibliographystyle{plainnat}\n");

    // --- user preamble ---
    for pkg in &doc.preamble.packages {
        if !is_required_package(pkg) {
            out.push_str(&format!("\\usepackage{{{pkg}}}\n"));
        }
    }
    for macro_def in &doc.preamble.macros {
        let args = if macro_def.n_args > 0 {
            format!("[{}]", macro_def.n_args)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "\\newcommand{{\\{name}}}{args}{{{body}}}\n",
            name = macro_def.name,
            body = macro_def.body
        ));
    }
    for line in &doc.preamble.custom {
        out.push_str(line);
        out.push('\n');
    }

    // --- title / author ---
    out.push_str(&format!("\n\\title{{{}}}\n", escape_latex(&doc.title)));

    if doc.authors.is_empty() {
        out.push_str("\\author{}\n");
    } else {
        let author_str: Vec<String> = doc
            .authors
            .iter()
            .map(|a| {
                let mut s = escape_latex(&a.name);
                if let Some(ref aff) = a.affiliation {
                    s.push_str(&format!("\\\\{aff}"));
                }
                if let Some(ref email) = a.email {
                    s.push_str(&format!("\\\\\\texttt{{{email}}}"));
                }
                s
            })
            .collect();
        out.push_str(&format!("\\author{{{}}}\n", author_str.join(" \\and ")));
    }

    out.push_str("\\date{\\today}\n\n");

    // --- begin document ---
    out.push_str("\\begin{document}\n");
    out.push_str("\\maketitle\n\n");

    // --- abstract ---
    if let Some(ref abs) = doc.metadata.abstract_text {
        out.push_str("\\begin{abstract}\n");
        out.push_str(&escape_latex(abs));
        out.push_str("\n\\end{abstract}\n\n");
    }

    // --- keywords ---
    if !doc.metadata.keywords.is_empty() {
        out.push_str("\\noindent\\textbf{Keywords:} ");
        out.push_str(&doc.metadata.keywords.join(", "));
        out.push_str("\n\n");
    }

    // --- body (section tree) ---
    render_section(&doc.root, &mut out);

    // --- bibliography ---
    out.push_str("\n\\bibliography{references}\n");

    // --- end document ---
    out.push_str("\\end{document}\n");

    RenderedDocument { main_tex: out }
}

/// Packages we always include — skip duplicates from user preamble.
fn is_required_package(pkg: &str) -> bool {
    matches!(
        pkg.trim(),
        "graphicx" | "amsmath" | "booktabs" | "hyperref" | "natbib"
    )
}

// ---------------------------------------------------------------------------
// Section rendering
// ---------------------------------------------------------------------------

fn render_section(section: &Section, out: &mut String) {
    // Skip the implicit root.
    if section.level != SectionLevel::Root {
        let cmd = section.level.command();
        if !cmd.is_empty() {
            out.push_str(&format!(
                "\\{cmd}{{{title}}}",
                title = escape_latex(&section.title)
            ));
            out.push('\n');
            if let Some(ref label) = section.label {
                out.push_str(&format!("\\label{{{label}}}\n"));
            }
            out.push('\n');
        }
    }

    // Blocks belonging to this section.
    for block in &section.blocks {
        render_block(block, out);
    }

    // Child sections.
    for child in &section.children {
        render_section(child, out);
    }
}

// ---------------------------------------------------------------------------
// Block rendering
// ---------------------------------------------------------------------------

fn render_block(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(p) => {
            let text: String = p.inlines.iter().map(render_inline).collect();
            out.push_str(&text);
            out.push_str("\n\n");
        }

        Block::Equation(eq) => {
            if eq.numbered {
                out.push_str("\\begin{equation}\n");
            } else {
                out.push_str("\\begin{equation*}\n");
            }
            out.push_str(&eq.latex);
            out.push('\n');
            if let Some(ref label) = eq.meta.label {
                out.push_str(&format!("\\label{{{label}}}\n"));
            }
            if eq.numbered {
                out.push_str("\\end{equation}\n\n");
            } else {
                out.push_str("\\end{equation*}\n\n");
            }
        }

        Block::Figure(fig) => {
            let spec = if fig.placement == Placement::Default {
                String::new()
            } else {
                format!("[{}]", fig.placement.specifier())
            };
            out.push_str(&format!("\\begin{{figure}}{spec}\n"));
            out.push_str("  \\centering\n");

            if fig.subfigures.is_empty() {
                render_figure_source(&fig.source, fig.width.as_ref(), out);
            } else {
                for sub in &fig.subfigures {
                    out.push_str("  \\begin{subfigure}{0.45\\textwidth}\n");
                    out.push_str("    \\centering\n");
                    render_figure_source(&sub.source, sub.width.as_ref(), out);
                    if !sub.caption.is_empty() {
                        let cap: String = sub.caption.iter().map(render_inline).collect();
                        out.push_str(&format!("    \\caption{{{cap}}}\n"));
                    }
                    out.push_str("  \\end{subfigure}\n");
                }
            }

            if !fig.caption.is_empty() {
                let cap: String = fig.caption.iter().map(render_inline).collect();
                out.push_str(&format!("  \\caption{{{cap}}}\n"));
            }
            if let Some(ref label) = fig.meta.label {
                out.push_str(&format!("  \\label{{{label}}}\n"));
            }
            out.push_str("\\end{figure}\n\n");
        }

        Block::Table(tbl) => {
            let spec = if tbl.placement == Placement::Default {
                String::new()
            } else {
                format!("[{}]", tbl.placement.specifier())
            };
            out.push_str(&format!("\\begin{{table}}{spec}\n"));
            out.push_str("  \\centering\n");

            if !tbl.caption.is_empty() {
                let cap: String = tbl.caption.iter().map(render_inline).collect();
                out.push_str(&format!("  \\caption{{{cap}}}\n"));
            }

            match &tbl.source {
                TableSource::Cells {
                    header,
                    rows,
                    alignment,
                } => {
                    render_tabular(header, rows, alignment, TableFormat::Booktabs, out);
                }
                TableSource::DagOutput { format, .. } => {
                    // Placeholder — DAG outputs are resolved before compilation.
                    out.push_str("  % DAG output (unresolved)\n");
                    out.push_str("  \\begin{tabular}{l}\n");
                    out.push_str("    \\toprule\n");
                    out.push_str("    (unresolved DAG output) \\\\\n");
                    out.push_str("    \\bottomrule\n");
                    out.push_str("  \\end{tabular}\n");
                    let _ = format;
                }
                TableSource::IcebergQuery { sql, .. } => {
                    out.push_str(&format!("  % Iceberg query: {sql}\n"));
                    out.push_str("  \\begin{tabular}{l}\n");
                    out.push_str("    (unresolved query) \\\\\n");
                    out.push_str("  \\end{tabular}\n");
                }
            }

            if let Some(ref label) = tbl.meta.label {
                out.push_str(&format!("  \\label{{{label}}}\n"));
            }
            out.push_str("\\end{table}\n\n");
        }

        Block::Code(code) => {
            let lang = code.language.as_deref().unwrap_or("");
            out.push_str(&format!("\\begin{{lstlisting}}[language={lang}]\n"));
            out.push_str(&code.code);
            out.push_str("\n\\end{lstlisting}\n\n");
        }

        Block::List(list) => {
            let env = match list.marker {
                ListMarker::Bullet => "itemize",
                ListMarker::Numbered => "enumerate",
                ListMarker::Description => "description",
            };
            out.push_str(&format!("\\begin{{{env}}}\n"));
            for item in &list.items {
                let body: String = item.inlines.iter().map(render_inline).collect();
                if let Some(ref term) = item.term {
                    out.push_str(&format!("  \\item[{term}] {body}\n"));
                } else {
                    out.push_str(&format!("  \\item {body}\n"));
                }
            }
            out.push_str(&format!("\\end{{{env}}}\n\n"));
        }

        Block::Quote(q) => {
            let text: String = q.inlines.iter().map(render_inline).collect();
            out.push_str("\\begin{quote}\n");
            out.push_str(&text);
            out.push_str("\n\\end{quote}\n\n");
        }

        Block::RawLatex(raw) => {
            out.push_str(&raw.content);
            out.push('\n');
        }

        Block::HorizontalRule(_) => {
            out.push_str("\\noindent\\rule{\\linewidth}{0.4pt}\n\n");
        }

        Block::PageBreak(_) => {
            out.push_str("\\newpage\n\n");
        }
    }
}

// ---------------------------------------------------------------------------
// Inline rendering
// ---------------------------------------------------------------------------

fn render_inline(inline: &Inline) -> String {
    match inline {
        Inline::Text { content } => escape_latex(content),

        Inline::Formatted { content, format } => {
            let escaped = escape_latex(content);
            match format {
                writing_types::TextFormat::Bold => format!("\\textbf{{{escaped}}}"),
                writing_types::TextFormat::Italic => format!("\\textit{{{escaped}}}"),
                writing_types::TextFormat::Underline => format!("\\underline{{{escaped}}}"),
                writing_types::TextFormat::Strikethrough => {
                    format!("\\sout{{{escaped}}}")
                }
                writing_types::TextFormat::Monospace => {
                    format!("\\texttt{{{escaped}}}")
                }
                writing_types::TextFormat::SmallCaps => format!("\\textsc{{{escaped}}}"),
            }
        }

        Inline::InlineMath { latex } => format!("${latex}$"),

        Inline::Citation(cluster) => {
            let cmd = cluster.style.natbib_cmd();
            let keys: Vec<&str> = cluster.keys.iter().map(|k| k.key.as_str()).collect();
            let keys_str = keys.join(", ");

            match (&cluster.prefix, &cluster.suffix) {
                (None, None) => format!("\\{cmd}{{{keys_str}}}"),
                (Some(pre), None) => format!("\\{cmd}[{pre}]{{{keys_str}}}"),
                (None, Some(suf)) => format!("\\{cmd}[][]{suf}]{{{keys_str}}}"),
                (Some(pre), Some(suf)) => {
                    format!("\\{cmd}[{pre}][{suf}]{{{keys_str}}}")
                }
            }
        }

        Inline::CrossRef {
            label, auto_prefix, ..
        } => {
            if *auto_prefix {
                format!("\\cref{{{label}}}")
            } else {
                format!("\\ref{{{label}}}")
            }
        }

        Inline::Link { url, text } => {
            if text.is_empty() {
                format!("\\url{{{url}}}")
            } else {
                format!("\\href{{{url}}}{{{text}}}")
            }
        }

        Inline::Footnote { content } => {
            let inner: String = content.iter().map(render_inline).collect();
            format!("\\footnote{{{inner}}}")
        }
    }
}

// ---------------------------------------------------------------------------
// Tabular rendering
// ---------------------------------------------------------------------------

fn render_tabular(
    header: &[String],
    rows: &[Vec<writing_types::TableCell>],
    alignment: &[ColumnAlign],
    format: TableFormat,
    out: &mut String,
) {
    // Build column spec.
    let n_cols = header
        .len()
        .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
    let col_spec: String = (0..n_cols)
        .map(|i| alignment.get(i).map(|a| a.latex_char()).unwrap_or('l'))
        .collect();

    let env = if format == TableFormat::Longtable {
        "longtable"
    } else {
        "tabular"
    };

    out.push_str(&format!("  \\begin{{{env}}}{{{col_spec}}}\n"));

    match format {
        TableFormat::Booktabs => {
            out.push_str("    \\toprule\n");
            // Header.
            let header_row: Vec<String> = header.iter().map(|h| escape_latex(h)).collect();
            out.push_str(&format!("    {} \\\\\n", header_row.join(" & ")));
            out.push_str("    \\midrule\n");
            // Data rows.
            for row in rows {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| {
                        if c.raw_latex {
                            c.content.clone()
                        } else {
                            escape_latex(&c.content)
                        }
                    })
                    .collect();
                out.push_str(&format!("    {} \\\\\n", cells.join(" & ")));
            }
            out.push_str("    \\bottomrule\n");
        }
        TableFormat::Plain => {
            out.push_str("    \\hline\n");
            let header_row: Vec<String> = header.iter().map(|h| escape_latex(h)).collect();
            out.push_str(&format!("    {} \\\\\n", header_row.join(" & ")));
            out.push_str("    \\hline\n");
            for row in rows {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| {
                        if c.raw_latex {
                            c.content.clone()
                        } else {
                            escape_latex(&c.content)
                        }
                    })
                    .collect();
                out.push_str(&format!("    {} \\\\\n", cells.join(" & ")));
            }
            out.push_str("    \\hline\n");
        }
        TableFormat::Longtable => {
            // Same as booktabs but in longtable env.
            out.push_str("    \\toprule\n");
            let header_row: Vec<String> = header.iter().map(|h| escape_latex(h)).collect();
            out.push_str(&format!("    {} \\\\\n", header_row.join(" & ")));
            out.push_str("    \\midrule\n");
            for row in rows {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| {
                        if c.raw_latex {
                            c.content.clone()
                        } else {
                            escape_latex(&c.content)
                        }
                    })
                    .collect();
                out.push_str(&format!("    {} \\\\\n", cells.join(" & ")));
            }
            out.push_str("    \\bottomrule\n");
        }
    }

    out.push_str(&format!("  \\end{{{env}}}\n"));
}

// ---------------------------------------------------------------------------
// Figure source rendering
// ---------------------------------------------------------------------------

fn render_figure_source(
    source: &FigureSource,
    width: Option<&writing_types::Size>,
    out: &mut String,
) {
    match source {
        FigureSource::FilePath { path } => {
            let w = width
                .map(|s| format!("[width={}] ", s.to_latex()))
                .unwrap_or_default();
            out.push_str(&format!("  \\includegraphics{w}{{{path}}}\n"));
        }
        FigureSource::DagArtifact { dag_id, node_id } => {
            // Placeholder — resolved before compilation.
            out.push_str(&format!("  % DAG artifact: {dag_id}/{node_id}\n"));
            out.push_str(&format!(
                "  \\includegraphics{{dag_{dag_id}_{node_id}.pdf}}\n"
            ));
        }
        FigureSource::Tikz { code } => {
            out.push_str("  \\begin{tikzpicture}\n");
            out.push_str(code);
            out.push_str("\n  \\end{tikzpicture}\n");
        }
    }
}

// ---------------------------------------------------------------------------
// LaTeX escaping
// ---------------------------------------------------------------------------

/// Escape special LaTeX characters in plain text.
fn escape_latex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '$' => out.push_str("\\$"),
            '#' => out.push_str("\\#"),
            '_' => out.push_str("\\_"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            '\\' => out.push_str("\\textbackslash{}"),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------===========

#[cfg(test)]
mod tests {
    use super::*;
    use writing_types::*;

    fn make_simple_doc() -> Document {
        let mut doc = Document::new("d1", "A Test Paper");
        doc.metadata.abstract_text = Some("This is an abstract.".into());
        doc.metadata.keywords = vec!["GWAS".into(), "genetics".into()];

        let mut intro = Section::new(SectionLevel::Section, "Introduction");
        intro.label = Some("sec:intro".into());
        intro.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
            Inline::text("GWAS have identified "),
            Inline::Citation(CitationCluster {
                keys: vec![CiteKey::new("smith2024"), CiteKey::new("jones2023")],
                style: CitationStyle::Parenthetical,
                prefix: None,
                suffix: None,
                claim_id: None,
            }),
            Inline::text(" many loci."),
        ])));
        doc.root.children.push(intro);

        let mut methods = Section::new(SectionLevel::Section, "Methods");
        methods.blocks.push(Block::Equation(EquationBlock {
            meta: BlockMeta::new().with_label("eq:model"),
            latex: "y = X\\beta + \\epsilon".into(),
            numbered: true,
        }));

        let mut results = Section::new(SectionLevel::Section, "Results");
        results.blocks.push(Block::Figure(FigureBlock {
            meta: BlockMeta::new().with_label("fig:main"),
            caption: vec![Inline::text("Main result figure.")],
            placement: Placement::Top,
            source: FigureSource::FilePath {
                path: "figures/main.pdf".into(),
            },
            width: Some(Size::text_width(0.8)),
            subfigures: vec![],
        }));
        results.blocks.push(Block::Table(TableBlock {
            meta: BlockMeta::new().with_label("tab:results"),
            caption: vec![Inline::text("Summary of results.")],
            placement: Placement::Default,
            source: TableSource::Cells {
                header: vec!["Method".into(), "Beta".into(), "P-value".into()],
                rows: vec![vec![
                    TableCell::plain("IVW"),
                    TableCell::raw("0.45^{***}"),
                    TableCell::plain("1.2e-5"),
                ]],
                alignment: vec![ColumnAlign::Left, ColumnAlign::Center, ColumnAlign::Center],
            },
        }));
        doc.root.children.push(methods);
        doc.root.children.push(results);

        doc
    }

    #[test]
    fn render_contains_documentclass() {
        let doc = make_simple_doc();
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\documentclass{article}"));
    }

    #[test]
    fn render_contains_title_and_maketitle() {
        let doc = make_simple_doc();
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\title{A Test Paper}"));
        assert!(rendered.main_tex.contains("\\maketitle"));
    }

    #[test]
    fn render_contains_abstract() {
        let doc = make_simple_doc();
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\begin{abstract}"));
        assert!(rendered.main_tex.contains("This is an abstract."));
        assert!(rendered.main_tex.contains("\\end{abstract}"));
    }

    #[test]
    fn render_contains_keywords() {
        let doc = make_simple_doc();
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("Keywords:"));
        assert!(rendered.main_tex.contains("GWAS, genetics"));
    }

    #[test]
    fn render_contains_sections() {
        let doc = make_simple_doc();
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\section{Introduction}"));
        assert!(rendered.main_tex.contains("\\label{sec:intro}"));
        assert!(rendered.main_tex.contains("\\section{Methods}"));
        assert!(rendered.main_tex.contains("\\section{Results}"));
    }

    #[test]
    fn render_contains_citation() {
        let doc = make_simple_doc();
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\citep{smith2024, jones2023}"));
    }

    #[test]
    fn render_contains_equation() {
        let doc = make_simple_doc();
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\begin{equation}"));
        assert!(rendered.main_tex.contains("y = X\\beta + \\epsilon"));
        assert!(rendered.main_tex.contains("\\label{eq:model}"));
        assert!(rendered.main_tex.contains("\\end{equation}"));
    }

    #[test]
    fn render_contains_figure() {
        let doc = make_simple_doc();
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\begin{figure}[t]"));
        assert!(
            rendered
                .main_tex
                .contains("\\includegraphics[width=0.8\\textwidth] {figures/main.pdf}")
        );
        assert!(rendered.main_tex.contains("\\caption{Main result figure.}"));
        assert!(rendered.main_tex.contains("\\label{fig:main}"));
        assert!(rendered.main_tex.contains("\\end{figure}"));
    }

    #[test]
    fn render_contains_table() {
        let doc = make_simple_doc();
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\begin{table}"));
        assert!(rendered.main_tex.contains("\\begin{tabular}{lcc}"));
        assert!(rendered.main_tex.contains("\\toprule"));
        assert!(rendered.main_tex.contains("Method & Beta & P-value"));
        assert!(rendered.main_tex.contains("IVW & 0.45^{***} & 1.2e-5"));
        assert!(rendered.main_tex.contains("\\bottomrule"));
        assert!(rendered.main_tex.contains("\\end{table}"));
    }

    #[test]
    fn render_contains_bibliography() {
        let doc = make_simple_doc();
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\bibliography{references}"));
        assert!(rendered.main_tex.contains("\\end{document}"));
    }

    #[test]
    fn render_textual_citation() {
        let mut doc = Document::new("d1", "Test");
        let mut sec = Section::new(SectionLevel::Section, "S");
        sec.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
            Inline::Citation(CitationCluster {
                keys: vec![CiteKey::new("doe2021")],
                style: CitationStyle::Textual,
                prefix: None,
                suffix: None,
                claim_id: None,
            }),
        ])));
        doc.root.children.push(sec);
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\citet{doe2021}"));
    }

    #[test]
    fn render_escape_special_chars() {
        let escaped = escape_latex("100% increase & decrease");
        assert_eq!(escaped, "100\\% increase \\& decrease");

        let escaped2 = escape_latex("cost $_ {}");
        assert_eq!(escaped2, "cost \\$\\_ \\{\\}");
    }

    #[test]
    fn render_raw_latex_passthrough() {
        let mut doc = Document::new("d1", "Test");
        let mut sec = Section::new(SectionLevel::Section, "S");
        sec.blocks.push(Block::RawLatex(RawLatexBlock {
            meta: BlockMeta::new(),
            content: "\\customcommand{data}".into(),
        }));
        doc.root.children.push(sec);
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\customcommand{data}"));
    }

    #[test]
    fn render_list_block() {
        let mut doc = Document::new("d1", "Test");
        let mut sec = Section::new(SectionLevel::Section, "S");
        sec.blocks.push(Block::List(ListBlock {
            meta: BlockMeta::new(),
            marker: ListMarker::Bullet,
            items: vec![
                ListItem {
                    term: None,
                    inlines: vec![Inline::text("First item")],
                },
                ListItem {
                    term: None,
                    inlines: vec![Inline::text("Second item")],
                },
            ],
        }));
        doc.root.children.push(sec);
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\begin{itemize}"));
        assert!(rendered.main_tex.contains("\\item First item"));
        assert!(rendered.main_tex.contains("\\end{itemize}"));
    }

    #[test]
    fn render_custom_document_class() {
        let mut doc = Document::new("d1", "Test");
        doc.document_class = DocumentClass::Custom("revtex4-2".into());
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\documentclass{revtex4-2}"));
    }

    #[test]
    fn render_preamble_packages() {
        let mut doc = Document::new("d1", "Test");
        doc.preamble.packages.push("cleveref".into());
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\usepackage{cleveref}"));
        // graphicx should not be duplicated.
        let count = rendered.main_tex.matches("usepackage{graphicx}").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn render_cross_reference() {
        let mut doc = Document::new("d1", "Test");
        let mut sec = Section::new(SectionLevel::Section, "S");
        sec.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
            Inline::text("See "),
            Inline::CrossRef {
                label: "fig:main".into(),
                kind: RefKind::Fig,
                auto_prefix: true,
            },
            Inline::text(" for details."),
        ])));
        doc.root.children.push(sec);
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\cref{fig:main}"));
    }

    #[test]
    fn render_subsection() {
        let mut doc = Document::new("d1", "Test");
        let mut sec = Section::new(SectionLevel::Section, "Methods");
        sec.children.push(Section::new(
            SectionLevel::Subsection,
            "Statistical Analysis",
        ));
        doc.root.children.push(sec);
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\section{Methods}"));
        assert!(
            rendered
                .main_tex
                .contains("\\subsection{Statistical Analysis}")
        );
    }

    #[test]
    fn render_tikz_figure() {
        let mut doc = Document::new("d1", "Test");
        let mut sec = Section::new(SectionLevel::Section, "S");
        sec.blocks.push(Block::Figure(FigureBlock {
            meta: BlockMeta::new(),
            caption: vec![Inline::text("TikZ figure.")],
            placement: Placement::Default,
            source: FigureSource::Tikz {
                code: "\\draw (0,0) -- (1,1);".into(),
            },
            width: None,
            subfigures: vec![],
        }));
        doc.root.children.push(sec);
        let rendered = render_document(&doc);
        assert!(rendered.main_tex.contains("\\begin{tikzpicture}"));
        assert!(rendered.main_tex.contains("\\draw (0,0) -- (1,1);"));
        assert!(rendered.main_tex.contains("\\end{tikzpicture}"));
    }
}
