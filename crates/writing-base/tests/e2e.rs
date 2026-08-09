//! End-to-end integration tests for the writing system.
//!
//! These tests exercise the full pipeline: create → edit → store →
//! serialise → version, covering realistic writing workflows.

use std::sync::Arc;

use writing_base::{
    CitationResolver, LatexEngine, NullEngine, WritingStore, XelatexEngine, ast, compile_document,
    render_document,
};
use writing_types::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a realistic paper outline.
fn build_paper() -> Document {
    let mut doc = Document::new("paper-001", "Genetic Architecture of Complex Traits");
    doc.metadata.abstract_text = Some(
        "We investigate the genetic architecture of complex traits using \
         Mendelian randomization and LD score regression."
            .into(),
    );
    doc.metadata.keywords = vec!["GWAS".into(), "Mendelian randomization".into()];
    doc.authors.push(DocumentAuthor {
        name: "Jane Doe".into(),
        affiliation: Some("Department of Genetics".into()),
        orcid: Some("0000-0001-0002-0003".into()),
        corresponding: true,
        email: Some("jane@university.edu".into()),
    });

    // Introduction
    let mut intro = Section::new(SectionLevel::Section, "Introduction");
    intro.label = Some("sec:intro".into());
    intro.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
        Inline::text(
            "Genome-wide association studies (GWAS) have identified thousands of \
                     genetic variants associated with complex traits ",
        ),
        Inline::Citation(CitationCluster {
            keys: vec![
                CiteKey {
                    key: "smith2024gwas".into(),
                    article_id: Some("art-1".into()),
                },
                CiteKey::new("jones2023mr"),
            ],
            style: CitationStyle::Parenthetical,
            prefix: None,
            suffix: None,
            claim_id: None,
        }),
        Inline::text(
            ". However, identifying causal variants from these associations \
                     remains challenging ",
        ),
        Inline::Citation(CitationCluster {
            keys: vec![CiteKey::new("doe2022causal")],
            style: CitationStyle::Textual,
            prefix: None,
            suffix: None,
            claim_id: None,
        }),
        Inline::text(" demonstrated that horizontal pleiotropy can inflate estimates."),
    ])));
    doc.root.children.push(intro);

    // Methods
    let mut methods = Section::new(SectionLevel::Section, "Methods");
    methods.label = Some("sec:methods".into());

    // Methods > Data
    let mut data = Section::new(SectionLevel::Subsection, "Data Sources");
    data.blocks.push(Block::Paragraph(ParagraphBlock::text(
        "We used summary statistics from the GWAS Catalog and UK Biobank.",
    )));
    methods.children.push(data);

    // Methods > Statistical Analysis
    let mut stats = Section::new(SectionLevel::Subsection, "Statistical Analysis");
    stats.label = Some("sec:stats".into());
    stats.blocks.push(Block::Equation(EquationBlock {
        meta: BlockMeta::new().with_label("eq:ivw"),
        latex: "\\hat{\\beta}_{IVW} = \\frac{\\sum_j \\hat{\\gamma}_j \\hat{\\Gamma}_j}{\\sum_j \\hat{\\gamma}_j^2}".into(),
        numbered: true,
    }));
    stats.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
        Inline::text("We applied the IVW estimator (Equation "),
        Inline::CrossRef {
            label: "eq:ivw".into(),
            kind: RefKind::Eq,
            auto_prefix: true,
        },
        Inline::text(") with a significance threshold of "),
        Inline::InlineMath {
            latex: "p < 5 \\times 10^{-8}".into(),
        },
        Inline::text("."),
    ])));
    methods.children.push(stats);
    doc.root.children.push(methods);

    // Results
    let mut results = Section::new(SectionLevel::Section, "Results");
    results.label = Some("sec:results".into());

    results
        .blocks
        .push(Block::Paragraph(ParagraphBlock::new(vec![
            Inline::text(
                "We identified 45 instrument variables after clumping. \
                     The IVW analysis yielded a causal estimate of ",
            ),
            Inline::InlineMath {
                latex: "\\beta = 0.42".into(),
            },
            Inline::text(" ("),
            Inline::Citation(CitationCluster {
                keys: vec![CiteKey::new("smith2024gwas")],
                style: CitationStyle::Parenthetical,
                prefix: None,
                suffix: Some("Table 1".into()),
                claim_id: None,
            }),
            Inline::text(")."),
        ])));

    results.blocks.push(Block::Table(TableBlock {
        meta: BlockMeta::new().with_label("tab:mr_results"),
        caption: vec![Inline::text("Mendelian randomization results.")],
        placement: Placement::Top,
        source: TableSource::Cells {
            header: vec![
                "Method".into(),
                "Beta".into(),
                "SE".into(),
                "P-value".into(),
            ],
            rows: vec![
                vec![
                    TableCell::plain("IVW"),
                    TableCell::raw("0.42^{***}"),
                    TableCell::plain("0.08"),
                    TableCell::plain("1.2e-07"),
                ],
                vec![
                    TableCell::plain("MR-Egger"),
                    TableCell::raw("0.38^{**}"),
                    TableCell::plain("0.12"),
                    TableCell::plain("0.002"),
                ],
                vec![
                    TableCell::plain("Weighted Median"),
                    TableCell::raw("0.40^{***}"),
                    TableCell::plain("0.09"),
                    TableCell::plain("8.5e-06"),
                ],
            ],
            alignment: vec![
                ColumnAlign::Left,
                ColumnAlign::Center,
                ColumnAlign::Center,
                ColumnAlign::Center,
            ],
        },
    }));

    results.blocks.push(Block::Figure(FigureBlock {
        meta: BlockMeta::new().with_label("fig:scatter"),
        caption: vec![Inline::text("Scatter plot of SNP-effect estimates.")],
        placement: Placement::Top,
        source: FigureSource::FilePath {
            path: "figures/scatter.pdf".into(),
        },
        width: Some(Size::text_width(0.8)),
        subfigures: vec![],
    }));

    doc.root.children.push(results);

    // Discussion
    let discussion = Section::new(SectionLevel::Section, "Discussion");
    doc.root.children.push(discussion);

    // Preamble
    doc.preamble.packages.push("cleveref".into());
    doc.preamble.packages.push("enumitem".into());

    doc
}

// ---------------------------------------------------------------------------
// E2E Tests
// ---------------------------------------------------------------------------

/// Full lifecycle: create, edit, store, retrieve, serialise.
#[tokio::test]
async fn full_document_lifecycle() {
    let store = WritingStore::open_in_memory().await.unwrap();

    // Build a complete paper and save it.
    let mut doc = build_paper();
    store
        .save_document(&mut doc, "initial draft", Some("agent_writer"))
        .await
        .unwrap();

    // Retrieve and verify structure.
    let loaded = store.get_document(&doc.id).await.unwrap();
    assert_eq!(loaded.title, "Genetic Architecture of Complex Traits");
    assert_eq!(loaded.root.children.len(), 4); // intro, methods, results, discussion
    assert_eq!(loaded.root.children[0].title, "Introduction");

    // Verify nested sections.
    let methods = &loaded.root.children[1];
    assert_eq!(methods.children.len(), 2); // Data Sources, Statistical Analysis
    assert_eq!(methods.children[1].title, "Statistical Analysis");
}

/// Edit a stored document and verify version history.
#[tokio::test]
async fn edit_and_version_history() {
    let store = WritingStore::open_in_memory().await.unwrap();
    let mut doc = build_paper();
    store
        .save_document(&mut doc, "v1: initial", None)
        .await
        .unwrap();

    // Version 1: Add a paragraph to Discussion.
    let discussion_id = doc.root.children[3].id.clone();
    ast::apply_edit(
        &mut doc,
        &EditOp::InsertBlock {
            section_id: discussion_id,
            after_block: None,
            block: Block::Paragraph(ParagraphBlock::text(
                "Our findings replicate previous results.",
            )),
        },
    )
    .unwrap();
    store
        .save_document(&mut doc, "v2: add discussion", None)
        .await
        .unwrap();

    // Version 2: Add a citation to Introduction.
    let intro_block_id = doc.root.children[0].blocks[0].id().to_string();
    ast::apply_edit(
        &mut doc,
        &EditOp::InsertCitation {
            block_id: intro_block_id,
            position: CitationPosition::End,
            keys: vec!["williams2023replication".into()],
            style: CitationStyle::Parenthetical,
        },
    )
    .unwrap();
    store
        .save_document(&mut doc, "v3: add citation", None)
        .await
        .unwrap();

    // Check version history.
    let versions = store.list_versions(&doc.id).await.unwrap();
    assert_eq!(versions.len(), 3); // 3 saves

    // Verify v1 had no discussion paragraph.
    let v1 = store.get_version(&doc.id, 1).await.unwrap();
    assert!(v1.root.children[3].blocks.is_empty());

    // Verify v2 had discussion paragraph.
    let v2 = store.get_version(&doc.id, 2).await.unwrap();
    assert_eq!(v2.root.children[3].blocks.len(), 1);

    // Verify v3 had the extra citation.
    let v3 = store.get_version(&doc.id, 3).await.unwrap();
    let keys = ast::all_cite_keys(&v3);
    assert!(keys.contains(&"williams2023replication".to_string()));

    // Latest version should also have it.
    let latest = store.get_document(&doc.id).await.unwrap();
    let keys = ast::all_cite_keys(&latest);
    assert!(keys.contains(&"williams2023replication".to_string()));
}

/// Serialise a complex document and verify the LaTeX output.
#[tokio::test]
async fn serialise_complex_document() {
    let doc = build_paper();
    let rendered = render_document(&doc);
    let tex = &rendered.main_tex;

    // Document class.
    assert!(tex.contains("\\documentclass{article}"));

    // Title and author.
    assert!(tex.contains("\\title{Genetic Architecture of Complex Traits}"));
    assert!(tex.contains("Jane Doe"));

    // Abstract.
    assert!(tex.contains("\\begin{abstract}"));
    assert!(tex.contains("Mendelian randomization"));

    // Sections.
    assert!(tex.contains("\\section{Introduction}"));
    assert!(tex.contains("\\section{Methods}"));
    assert!(tex.contains("\\subsection{Data Sources}"));
    assert!(tex.contains("\\subsection{Statistical Analysis}"));
    assert!(tex.contains("\\section{Results}"));
    assert!(tex.contains("\\section{Discussion}"));

    // Labels.
    assert!(tex.contains("\\label{sec:intro}"));
    assert!(tex.contains("\\label{eq:ivw}"));
    assert!(tex.contains("\\label{tab:mr_results}"));
    assert!(tex.contains("\\label{fig:scatter}"));

    // Citations.
    assert!(tex.contains("\\citep{smith2024gwas, jones2023mr}"));
    assert!(tex.contains("\\citet{doe2022causal}"));

    // Equation.
    assert!(tex.contains("\\begin{equation}"));
    assert!(tex.contains("\\hat{\\beta}_{IVW}"));
    assert!(tex.contains("\\end{equation}"));

    // Cross-reference.
    assert!(tex.contains("\\cref{eq:ivw}"));

    // Inline math.
    assert!(tex.contains("$p < 5 \\times 10^{-8}$"));

    // Table with booktabs.
    assert!(tex.contains("\\begin{table}[t]"));
    assert!(tex.contains("\\begin{tabular}{lccc}"));
    assert!(tex.contains("IVW & 0.42^{***} & 0.08 & 1.2e-07"));
    assert!(tex.contains("MR-Egger & 0.38^{**} & 0.12 & 0.002"));
    assert!(tex.contains("\\toprule"));
    assert!(tex.contains("\\bottomrule"));

    // Figure.
    assert!(tex.contains("\\begin{figure}[t]"));
    assert!(tex.contains("\\includegraphics[width=0.8\\textwidth] {figures/scatter.pdf}"));

    // Preamble packages.
    assert!(tex.contains("\\usepackage{cleveref}"));
    assert!(tex.contains("\\usepackage{enumitem}"));

    // Bibliography.
    assert!(tex.contains("\\bibliography{references}"));
    assert!(tex.contains("\\end{document}"));
}

/// EditScript atomicity: complex multi-op script.
#[tokio::test]
async fn edit_script_complex_restructure() {
    let store = WritingStore::open_in_memory().await.unwrap();
    let mut doc = Document::new("d1", "Restructure Test");

    // Build a 3-section structure.
    let script = EditScript::new()
        .op(EditOp::InsertSection {
            parent_id: None,
            after_section: None,
            section: Section::new(SectionLevel::Section, "A"),
        })
        .op(EditOp::InsertSection {
            parent_id: None,
            after_section: None,
            section: Section::new(SectionLevel::Section, "B"),
        })
        .op(EditOp::InsertSection {
            parent_id: None,
            after_section: None,
            section: Section::new(SectionLevel::Section, "C"),
        })
        .message("Add A, B, C sections");

    ast::apply_edit_script(&mut doc, &script).unwrap();
    assert_eq!(doc.root.children.len(), 3);

    // Restructure: move C between A and B, rename B, delete C.
    let a_id = doc.root.children[0].id.clone();
    let b_id = doc.root.children[1].id.clone();
    let c_id = doc.root.children[2].id.clone();

    let script2 = EditScript::new()
        .op(EditOp::MoveSection {
            section_id: c_id.clone(),
            new_parent: None,
            after_section: Some(a_id),
        })
        .op(EditOp::RenameSection {
            section_id: b_id.clone(),
            new_title: "B Renamed".into(),
        })
        .message("restructure");

    ast::apply_edit_script(&mut doc, &script2).unwrap();
    // Order should be A, C, B Renamed.
    assert_eq!(doc.root.children[0].title, "A");
    assert_eq!(doc.root.children[1].title, "C");
    assert_eq!(doc.root.children[2].title, "B Renamed");

    store
        .save_document(&mut doc, "restructured", None)
        .await
        .unwrap();
}

/// Citation management: add, collect, and verify.
#[tokio::test]
async fn citation_management_e2e() {
    let store = WritingStore::open_in_memory().await.unwrap();
    let mut doc = build_paper();

    // Verify existing citations.
    let keys = ast::all_cite_keys(&doc);
    assert!(keys.contains(&"smith2024gwas".to_string()));
    assert!(keys.contains(&"jones2023mr".to_string()));
    assert!(keys.contains(&"doe2022causal".to_string()));

    // Add a new citation to the Results section.
    let results_block_id = doc.root.children[2].blocks[0].id().to_string();
    ast::apply_edit(
        &mut doc,
        &EditOp::InsertCitation {
            block_id: results_block_id,
            position: CitationPosition::End,
            keys: vec!["newref2024".into()],
            style: CitationStyle::Textual,
        },
    )
    .unwrap();

    // Verify.
    let keys = ast::all_cite_keys(&doc);
    assert!(keys.contains(&"newref2024".to_string()));

    // Store and reload.
    store
        .save_document(&mut doc, "with citations", None)
        .await
        .unwrap();

    let loaded = store.get_document(&doc.id).await.unwrap();
    let keys = ast::all_cite_keys(&loaded);
    assert!(keys.contains(&"newref2024".to_string()));
    assert!(keys.contains(&"smith2024gwas".to_string()));
}

/// Atomic rollback: failed script leaves document untouched.
#[tokio::test]
async fn edit_script_rollback_e2e() {
    let mut doc = build_paper();
    let original_section_count = doc.root.children.len();
    let original_title = doc.title.clone();

    // Script with a valid op followed by an invalid one.
    let valid_section_id = doc.root.children[0].id.clone();
    let script = EditScript::new()
        .op(EditOp::RenameSection {
            section_id: valid_section_id,
            new_title: "Changed Title".into(),
        })
        .op(EditOp::InsertBlock {
            section_id: "nonexistent-section".into(),
            after_block: None,
            block: Block::Paragraph(ParagraphBlock::text("...")),
        })
        .message("should roll back");

    let result = ast::apply_edit_script(&mut doc, &script);
    assert!(result.is_err());

    // Document is unchanged.
    assert_eq!(doc.root.children.len(), original_section_count);
    assert_eq!(doc.root.children[0].title, "Introduction"); // not "Changed Title"
    assert_eq!(doc.title, original_title);
}

/// Outline extraction and verification.
#[tokio::test]
async fn outline_extraction_e2e() {
    let doc = build_paper();
    let outline = ast::outline(&doc);

    // Should have: Introduction, Methods, Data Sources, Statistical Analysis,
    // Results, Discussion = 6 items.
    assert_eq!(outline.items.len(), 6);

    // Verify ordering and hierarchy.
    assert_eq!(outline.items[0].title, "Introduction");
    assert_eq!(outline.items[0].level, SectionLevel::Section.depth());

    assert_eq!(outline.items[1].title, "Methods");
    assert_eq!(outline.items[1].level, SectionLevel::Section.depth());

    // Subsections are deeper.
    assert_eq!(outline.items[2].title, "Data Sources");
    assert!(outline.items[2].level > outline.items[1].level);

    assert_eq!(outline.items[3].title, "Statistical Analysis");
    assert!(outline.items[3].level > outline.items[1].level);

    // Block counts.
    let stats_item = &outline.items[3]; // Statistical Analysis
    assert!(stats_item.block_count >= 2); // equation + paragraph
}

/// Multiple documents in one store.
#[tokio::test]
async fn multi_document_store_e2e() {
    let store = WritingStore::open_in_memory().await.unwrap();

    let mut doc1 = Document::new("paper-a", "Paper A");
    doc1.root
        .children
        .push(Section::new(SectionLevel::Section, "Intro A"));
    store
        .save_document(&mut doc1, "draft A", None)
        .await
        .unwrap();

    let mut doc2 = Document::new("paper-b", "Paper B");
    doc2.root
        .children
        .push(Section::new(SectionLevel::Section, "Intro B"));
    store
        .save_document(&mut doc2, "draft B", None)
        .await
        .unwrap();

    let docs = store.list_documents().await.unwrap();
    assert_eq!(docs.len(), 2);

    // Verify they don't interfere.
    let loaded_a = store.get_document("paper-a").await.unwrap();
    assert_eq!(loaded_a.root.children[0].title, "Intro A");

    let loaded_b = store.get_document("paper-b").await.unwrap();
    assert_eq!(loaded_b.root.children[0].title, "Intro B");

    // Delete one doesn't affect the other.
    store.delete_document("paper-a").await.unwrap();
    assert!(store.get_document("paper-a").await.is_err());
    assert!(store.get_document("paper-b").await.is_ok());

    let docs = store.list_documents().await.unwrap();
    assert_eq!(docs.len(), 1);
}

/// Block-level operations: insert, move, delete, replace.
#[tokio::test]
async fn block_operations_e2e() {
    let store = WritingStore::open_in_memory().await.unwrap();
    let mut doc = build_paper();

    // Insert a new equation into Methods > Statistical Analysis.
    let stats_id = doc.root.children[1].children[1].id.clone();
    ast::apply_edit(
        &mut doc,
        &EditOp::InsertBlock {
            section_id: stats_id.clone(),
            after_block: None,
            block: Block::Equation(EquationBlock {
                meta: BlockMeta::new().with_label("eq:egger"),
                latex: "\\beta_{Egger} = \\beta_0 + \\beta_1 \\theta".into(),
                numbered: true,
            }),
        },
    )
    .unwrap();

    // Verify block count increased.
    let stats_section = ast::find_section(&doc, &stats_id).unwrap();
    assert_eq!(stats_section.blocks.len(), 3); // was 2, now 3

    // Delete the original IVW equation.
    let ivw_block_id = stats_section.blocks[0].id().to_string();
    ast::apply_edit(
        &mut doc,
        &EditOp::DeleteBlock {
            block_id: ivw_block_id,
        },
    )
    .unwrap();

    let stats_section = ast::find_section(&doc, &stats_id).unwrap();
    assert_eq!(stats_section.blocks.len(), 2);

    // Replace a paragraph.
    let para_id = stats_section.blocks[0].id().to_string();
    ast::apply_edit(
        &mut doc,
        &EditOp::ReplaceParagraph {
            block_id: para_id,
            inlines: vec![Inline::text("Replaced content.")],
        },
    )
    .unwrap();

    let stats_section = ast::find_section(&doc, &stats_id).unwrap();
    if let Block::Paragraph(p) = &stats_section.blocks[0] {
        assert_eq!(p.plain_text(), "Replaced content.");
    } else {
        panic!("expected paragraph");
    }

    // Store final state.
    store
        .save_document(&mut doc, "block ops", None)
        .await
        .unwrap();
}

/// Render and verify LaTeX for a document with all block types.
#[test]
fn render_all_block_types() {
    let mut doc = Document::new("d1", "All Block Types");

    let mut sec = Section::new(SectionLevel::Section, "Test Section");

    // Paragraph with all inline types.
    sec.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
        Inline::text("This has "),
        Inline::formatted("bold", TextFormat::Bold),
        Inline::text(" and "),
        Inline::formatted("italic", TextFormat::Italic),
        Inline::text(" text, a citation "),
        Inline::Citation(CitationCluster {
            keys: vec![CiteKey::new("ref1")],
            style: CitationStyle::Parenthetical,
            prefix: None,
            suffix: None,
            claim_id: None,
        }),
        Inline::text(", a cross-reference "),
        Inline::CrossRef {
            label: "eq:test".into(),
            kind: RefKind::Eq,
            auto_prefix: false,
        },
        Inline::text(", inline math "),
        Inline::InlineMath {
            latex: "x^2 + y^2 = z^2".into(),
        },
        Inline::text(", and a "),
        Inline::Link {
            url: "https://example.com".into(),
            text: "link".into(),
        },
        Inline::text("."),
    ])));

    // Equation.
    sec.blocks.push(Block::Equation(EquationBlock {
        meta: BlockMeta::new().with_label("eq:test"),
        latex: "E = mc^2".into(),
        numbered: true,
    }));

    // List.
    sec.blocks.push(Block::List(ListBlock {
        meta: BlockMeta::new(),
        marker: ListMarker::Numbered,
        items: vec![
            ListItem {
                term: None,
                inlines: vec![Inline::text("First")],
            },
            ListItem {
                term: None,
                inlines: vec![Inline::text("Second")],
            },
        ],
    }));

    // Quote.
    sec.blocks.push(Block::Quote(QuoteBlock {
        meta: BlockMeta::new(),
        inlines: vec![Inline::text("A notable quote.")],
    }));

    // Raw LaTeX.
    sec.blocks.push(Block::RawLatex(RawLatexBlock {
        meta: BlockMeta::new(),
        content: "\\vspace{1em}".into(),
    }));

    // Page break.
    sec.blocks
        .push(Block::PageBreak(PageBreak { id: "pb1".into() }));

    doc.root.children.push(sec);

    let rendered = render_document(&doc);
    let tex = rendered.main_tex;

    // Verify all elements are present.
    assert!(tex.contains("\\textbf{bold}"));
    assert!(tex.contains("\\textit{italic}"));
    assert!(tex.contains("\\citep{ref1}"));
    assert!(tex.contains("\\ref{eq:test}"));
    assert!(tex.contains("$x^2 + y^2 = z^2$"));
    assert!(tex.contains("\\href{https://example.com}{link}"));
    assert!(tex.contains("\\begin{equation}"));
    assert!(tex.contains("E = mc^2"));
    assert!(tex.contains("\\begin{enumerate}"));
    assert!(tex.contains("\\item First"));
    assert!(tex.contains("\\begin{quote}"));
    assert!(tex.contains("A notable quote."));
    assert!(tex.contains("\\vspace{1em}"));
    assert!(tex.contains("\\newpage"));
}

/// Verify document persistence round-trips citations correctly.
#[tokio::test]
async fn persistence_preserves_citations() {
    let store = WritingStore::open_in_memory().await.unwrap();

    let mut doc = build_paper();
    let original_keys = ast::all_cite_keys(&doc);

    store
        .save_document(&mut doc, "with citations", None)
        .await
        .unwrap();

    // Reload and verify all citations survived the round-trip.
    let loaded = store.get_document(&doc.id).await.unwrap();
    let loaded_keys = ast::all_cite_keys(&loaded);

    assert_eq!(original_keys, loaded_keys);

    // Also verify the serialiser sees the same citations.
    let rendered = render_document(&loaded);
    for key in &loaded_keys {
        // Each cite key should appear in at least one \cite* command.
        assert!(
            rendered.main_tex.contains(key),
            "cite key '{key}' not found in LaTeX output"
        );
    }
}

/// Nested section operations with version tracking.
#[tokio::test]
async fn nested_sections_with_versions() {
    let store = WritingStore::open_in_memory().await.unwrap();

    let mut doc = Document::new("d1", "Nested Thesis");

    // Build a deep chapter > section > subsection structure.
    let mut chapter = Section::new(SectionLevel::Chapter, "Chapter 1");
    let mut sec = Section::new(SectionLevel::Section, "1.1 Background");
    let sub = Section::new(SectionLevel::Subsection, "1.1.1 History");
    let sub_id = sub.id.clone();
    sec.children.push(sub);
    let sec_id = sec.id.clone();
    chapter.children.push(sec);
    let chap_id = chapter.id.clone();
    doc.root.children.push(chapter);

    store
        .save_document(&mut doc, "chapter 1 structure", None)
        .await
        .unwrap();

    // Add content to the deeply nested subsection.
    ast::apply_edit(
        &mut doc,
        &EditOp::InsertBlock {
            section_id: sub_id.clone(),
            after_block: None,
            block: Block::Paragraph(ParagraphBlock::text(
                "The history of this field dates back to the 1990s.",
            )),
        },
    )
    .unwrap();
    store
        .save_document(&mut doc, "add subsection content", None)
        .await
        .unwrap();

    // Verify structure survived storage.
    let loaded = store.get_document(&doc.id).await.unwrap();
    let chap = loaded.root.find(&chap_id).unwrap();
    let s = chap.find(&sec_id).unwrap();
    let sub = s.find(&sub_id).unwrap();
    assert_eq!(sub.blocks.len(), 1);
    assert!(sub.blocks[0].kind_name() == "paragraph");

    // Verify deep nesting is preserved in LaTeX.
    let rendered = render_document(&loaded);
    assert!(rendered.main_tex.contains("\\chapter{Chapter 1}"));
    assert!(rendered.main_tex.contains("\\section{1.1 Background}"));
    assert!(rendered.main_tex.contains("\\subsection{1.1.1 History}"));
}

// ---------------------------------------------------------------------------
// Citation resolver e2e tests (Phase 2)
// ---------------------------------------------------------------------------

/// Full citation lifecycle: populate bib-base, write document with cite keys,
/// resolve, generate .bib, check consistency.
#[tokio::test]
async fn citation_resolver_full_pipeline() {
    use bib_base::BibBase;
    use bib_types::{Article, Author, Identifier};

    // Set up a bib-base library with real articles (including an uncited one).
    let bib = Arc::new(BibBase::open_in_memory().await.unwrap());

    let mut a1 = Article::new("art-mr", "Mendelian randomization analysis of BMI");
    a1.authors.push(Author {
        last_name: "Smith".into(),
        fore_name: Some("John".into()),
        initials: Some("J".into()),
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    a1.year = Some(2024);
    a1.journal = Some("Nature Genetics".into());
    a1.volume = Some("56".into());
    a1.identifiers
        .push(Identifier::doi("10.1038/s41588-024-1234"));
    bib.upsert_article(&a1).await.unwrap();

    let mut a2 = Article::new("art-ldsc", "LD score regression distinguishes confounding");
    a2.authors.push(Author {
        last_name: "Bulik".into(),
        fore_name: Some("S".into()),
        initials: Some("S".into()),
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    a2.year = Some(2015);
    a2.journal = Some("Nature Genetics".into());
    bib.upsert_article(&a2).await.unwrap();

    // Uncited article — in library but not referenced in the document.
    let mut a3 = Article::new("art-uncited", "An unrelated paper about something else");
    a3.authors.push(Author {
        last_name: "Nobody".into(),
        fore_name: None,
        initials: Some("N".into()),
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    a3.year = Some(2020);
    bib.upsert_article(&a3).await.unwrap();

    let resolver = CitationResolver::new(bib);

    // Build a document citing two articles plus a missing one.
    let mut doc = Document::new("paper-cite", "MR Analysis Paper");
    let mut intro = Section::new(SectionLevel::Section, "Introduction");
    intro.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
        Inline::text("We used MR methods "),
        Inline::Citation(CitationCluster {
            keys: vec![CiteKey::new("smith2024mendelian")],
            style: CitationStyle::Parenthetical,
            prefix: None,
            suffix: None,
            claim_id: None,
        }),
        Inline::text(" and LDSC "),
        Inline::Citation(CitationCluster {
            keys: vec![CiteKey::new("bulik2015ld")],
            style: CitationStyle::Textual,
            prefix: None,
            suffix: None,
            claim_id: None,
        }),
        Inline::text(" to investigate causality. We also cite a missing paper "),
        Inline::Citation(CitationCluster {
            keys: vec![CiteKey::new("ghost2024phantom")],
            style: CitationStyle::Parenthetical,
            prefix: None,
            suffix: None,
            claim_id: None,
        }),
        Inline::text("."),
    ])));
    doc.root.children.push(intro);

    // Scan.
    let report = resolver.scan(&doc).await;
    assert_eq!(report.keys.len(), 3);
    assert_eq!(report.resolved_count(), 2);
    assert_eq!(report.unresolved_count(), 1);
    assert!(!report.all_resolved());

    // The unresolved key.
    let unresolved = report
        .statuses
        .iter()
        .find(|s| matches!(s, writing_base::CiteKeyStatus::Unresolved { .. }))
        .unwrap();
    assert_eq!(unresolved.key(), "ghost2024phantom");

    // Uncited article detected.
    assert!(
        report
            .uncited_article_ids
            .contains(&"art-uncited".to_string())
    );

    // Generate .bib file.
    let bib_content = resolver.generate_bib(&doc).await;
    assert!(bib_content.contains("@article{smith2024mendelian"));
    assert!(bib_content.contains("@article{bulik2015ld"));
    assert!(bib_content.contains("Smith"));
    assert!(bib_content.contains("Bulik"));
    assert!(bib_content.contains("Mendelian randomization"));
    assert!(bib_content.contains("UNRESOLVED"));
    assert!(bib_content.contains("ghost2024phantom"));

    // Citation graph: intro paragraph should map to 2 articles.
    let graph = resolver.citation_graph(&doc).await;
    assert_eq!(graph.len(), 1); // one paragraph with citations
    let block_articles: Vec<&Vec<String>> = graph.values().collect();
    assert_eq!(block_articles[0].len(), 2);

    // Deletion impact: removing art-mr breaks smith2024mendelian.
    let impact = resolver.check_deletion_impact("art-mr", &doc).await;
    assert!(!impact.is_empty());

    // Removing uncited article breaks nothing.
    let no_impact = resolver.check_deletion_impact("art-uncited", &doc).await;
    assert!(no_impact.is_empty());
}

/// Citation resolver with an empty library.
#[tokio::test]
async fn citation_resolver_empty_library() {
    use bib_base::BibBase;

    let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
    let resolver = CitationResolver::new(bib);

    let mut doc = Document::new("d1", "Test");
    let mut sec = Section::new(SectionLevel::Section, "S");
    sec.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
        Inline::text("Cite "),
        Inline::Citation(CitationCluster {
            keys: vec![CiteKey::new("smith2024test")],
            style: CitationStyle::Parenthetical,
            prefix: None,
            suffix: None,
            claim_id: None,
        }),
    ])));
    doc.root.children.push(sec);

    let report = resolver.scan(&doc).await;
    assert_eq!(report.unresolved_count(), 1);
    assert_eq!(report.resolved_count(), 0);

    let bib_content = resolver.generate_bib(&doc).await;
    assert!(bib_content.contains("UNRESOLVED"));
    assert!(!bib_content.contains("@article"));
}

/// Citation round-trip: resolve keys → generate bib → verify keys in bib match.
#[tokio::test]
async fn citation_bib_roundtrip() {
    use bib_base::BibBase;
    use bib_types::{Article, Author, Identifier};

    let bib = Arc::new(BibBase::open_in_memory().await.unwrap());

    let mut a1 = Article::new("art-1", "Genetic architecture of type 2 diabetes");
    a1.authors.push(Author {
        last_name: "Zhang".into(),
        fore_name: Some("Wei".into()),
        initials: Some("W".into()),
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    a1.year = Some(2023);
    a1.journal = Some("Nature".into());
    a1.identifiers.push(Identifier::doi("10.1038/nature12345"));
    bib.upsert_article(&a1).await.unwrap();

    let resolver = CitationResolver::new(bib);

    // Build a doc citing the article by its expected cite key.
    let mut doc = Document::new("d1", "Diabetes Review");
    let mut sec = Section::new(SectionLevel::Section, "Background");
    sec.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
        Inline::Citation(CitationCluster {
            keys: vec![CiteKey::new("zhang2023genetic")],
            style: CitationStyle::Parenthetical,
            prefix: None,
            suffix: None,
            claim_id: None,
        }),
    ])));
    doc.root.children.push(sec);

    // Resolve.
    let report = resolver.scan(&doc).await;
    assert!(report.all_resolved());

    // Generate bib and verify the cite key matches.
    let bib_content = resolver.generate_bib(&doc).await;
    assert!(bib_content.contains("@article{zhang2023genetic"));
    assert!(bib_content.contains("Zhang"));
    assert!(bib_content.contains("2023"));
    assert!(bib_content.contains("Genetic architecture"));
    assert!(bib_content.contains("Nature"));
}

/// Citation persistence: store document with citations, reload, resolve.
#[tokio::test]
async fn citation_persistence_e2e() {
    use bib_base::BibBase;
    use bib_types::{Article, Author, Identifier};

    let store = WritingStore::open_in_memory().await.unwrap();
    let bib = Arc::new(BibBase::open_in_memory().await.unwrap());

    let mut a1 = Article::new("art-key", "Polygenic risk scores in clinical practice");
    a1.authors.push(Author {
        last_name: "Lewis".into(),
        fore_name: Some("C".into()),
        initials: Some("C".into()),
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    a1.year = Some(2024);
    a1.identifiers.push(Identifier::doi("10.1001/prs.2024"));
    bib.upsert_article(&a1).await.unwrap();

    let resolver = CitationResolver::new(bib);

    // Build and store a document.
    let mut doc = Document::new("persist-test", "PRS Review");
    let mut sec = Section::new(SectionLevel::Section, "Introduction");
    sec.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
        Inline::text("PRS are useful "),
        Inline::Citation(CitationCluster {
            keys: vec![CiteKey::new("lewis2024polygenic")],
            style: CitationStyle::Parenthetical,
            prefix: None,
            suffix: None,
            claim_id: None,
        }),
        Inline::text("."),
    ])));
    doc.root.children.push(sec);

    store
        .save_document(&mut doc, "initial", None)
        .await
        .unwrap();

    // Reload from store and re-resolve.
    let loaded = store.get_document(&doc.id).await.unwrap();
    let report = resolver.scan(&loaded).await;
    assert!(report.all_resolved());
    assert_eq!(report.resolved_count(), 1);

    // Render and verify citation appears in LaTeX.
    let rendered = render_document(&loaded);
    assert!(rendered.main_tex.contains("\\citep{lewis2024polygenic}"));
}

// ---------------------------------------------------------------------------
// Compilation e2e tests (Phase 3)
// ---------------------------------------------------------------------------

/// Full pipeline: build document → compile with NullEngine → validate.
#[tokio::test]
async fn compile_pipeline_null_engine() {
    let doc = build_paper();
    let engine = NullEngine;
    let output = compile_document(&doc, None, &engine).await.unwrap();

    assert!(output.success);
    assert_eq!(output.engine_name, "null");
    // NullEngine doesn't produce a real PDF but validates the pipeline.
    assert!(output.pdf_bytes.is_none());
}

/// Real XeLaTeX compilation of a complex document to PDF.
#[tokio::test]
async fn compile_to_pdf_xelatex() {
    // Remove bibliography dependency for this test — we don't have a real .bib.
    // Build a simplified doc without citations.
    let mut simple_doc = Document::new("compile-test", "Compilation Test");
    simple_doc.metadata.abstract_text = Some("Testing compilation.".into());

    let mut sec = Section::new(SectionLevel::Section, "Introduction");
    sec.blocks.push(Block::Paragraph(ParagraphBlock::text(
        "This document tests the LaTeX compilation pipeline.",
    )));
    simple_doc.root.children.push(sec);

    let mut methods = Section::new(SectionLevel::Section, "Methods");
    methods.blocks.push(Block::Equation(EquationBlock {
        meta: BlockMeta::new().with_label("eq:model"),
        latex: "y = X\\beta + \\epsilon".into(),
        numbered: true,
    }));

    methods.blocks.push(Block::Table(TableBlock {
        meta: BlockMeta::new().with_label("tab:results"),
        caption: vec![Inline::text("Results table.")],
        placement: Placement::Top,
        source: TableSource::Cells {
            header: vec!["Method".into(), "Beta".into(), "P".into()],
            rows: vec![vec![
                TableCell::plain("IVW"),
                TableCell::plain("0.42"),
                TableCell::plain("0.001"),
            ]],
            alignment: vec![ColumnAlign::Left, ColumnAlign::Center, ColumnAlign::Center],
        },
    }));
    simple_doc.root.children.push(methods);

    let engine = XelatexEngine::new().without_bibtex().with_passes(1);
    if !engine.is_available() {
        eprintln!("skipping: xelatex not available");
        return;
    }

    let output = compile_document(&simple_doc, None, &engine).await;

    let output = match output {
        Ok(o) => o,
        Err(e) => panic!("compilation failed: {e}"),
    };

    assert!(output.success, "compilation failed:\n{}", output.log);
    assert!(output.pdf_bytes.is_some(), "no PDF produced");
    let pdf = output.pdf_bytes.unwrap();
    assert!(pdf.len() > 1000, "PDF too small ({} bytes)", pdf.len());

    // PDF should start with %PDF.
    assert!(pdf.starts_with(b"%PDF"), "not a valid PDF file");

    // Should have at least 1 page.
    assert!(output.pages.unwrap_or(0) >= 1, "no pages in PDF");
}

/// Full pipeline with citations: resolve → compile with bibtex → PDF.
#[tokio::test]
async fn compile_with_citations_xelatex() {
    use bib_base::BibBase;
    use bib_types::{Article, Author, Identifier};

    let bib = Arc::new(BibBase::open_in_memory().await.unwrap());

    let mut a1 = Article::new("art-1", "A test article on genetics");
    a1.authors.push(Author {
        last_name: "TestAuthor".into(),
        fore_name: Some("A".into()),
        initials: Some("A".into()),
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    a1.year = Some(2024);
    a1.journal = Some("Nature".into());
    a1.identifiers.push(Identifier::doi("10.1000/test"));
    bib.upsert_article(&a1).await.unwrap();

    let resolver = CitationResolver::new(bib);

    let mut doc = Document::new("cite-compile", "Citation Compile Test");
    let mut sec = Section::new(SectionLevel::Section, "Introduction");
    sec.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
        Inline::text("A prior study "),
        Inline::Citation(CitationCluster {
            keys: vec![CiteKey::new("testauthor2024a")],
            style: CitationStyle::Parenthetical,
            prefix: None,
            suffix: None,
            claim_id: None,
        }),
        Inline::text(" showed results."),
    ])));
    doc.root.children.push(sec);

    let engine = XelatexEngine::new().with_passes(3);
    if !engine.is_available() {
        eprintln!("skipping: xelatex not available");
        return;
    }

    let output = compile_document(&doc, Some(&resolver), &engine).await;

    let output = match output {
        Ok(o) => o,
        Err(e) => panic!("compilation failed: {e}"),
    };

    assert!(output.success, "compilation failed:\n{}", output.log);
    assert!(output.pdf_bytes.is_some(), "no PDF produced");

    // PDF should be valid.
    let pdf = output.pdf_bytes.unwrap();
    assert!(pdf.starts_with(b"%PDF"), "not a valid PDF");
}

/// Compile error detection: undefined command should produce errors.
#[tokio::test]
async fn compile_error_detection() {
    let engine = XelatexEngine::new().without_bibtex().with_passes(1);
    if !engine.is_available() {
        return;
    }

    // Manually craft broken LaTeX.
    let input = writing_base::CompileInput {
        main_tex: r#"\documentclass{article}
\begin{document}
\undefinedcommand{blah}
\end{document}
"#
        .into(),
        ..Default::default()
    };

    let output = engine.compile(&input).await.unwrap();
    // Should detect an error (undefined command).
    assert!(
        !output.success || output.has_errors() || output.pdf_bytes.is_none(),
        "expected error for undefined command"
    );
}

/// Log parser integration: compile a doc with a warning, verify parsing.
#[tokio::test]
async fn compile_log_parsing() {
    let engine = XelatexEngine::new().without_bibtex().with_passes(1);
    if !engine.is_available() {
        return;
    }

    // Reference to a non-existent label generates a warning.
    let input = writing_base::CompileInput {
        main_tex: r#"\documentclass{article}
\begin{document}
See Section~\ref{nonexistent}.
\end{document}
"#
        .into(),
        ..Default::default()
    };

    let output = engine.compile(&input).await.unwrap();
    // Should compile successfully but with warnings about undefined reference.
    if output.success {
        // The log should mention the undefined reference.
        assert!(
            output.log.contains("nonexistent") || output.log.contains("Warning"),
            "expected reference warning in log"
        );
    }
}
