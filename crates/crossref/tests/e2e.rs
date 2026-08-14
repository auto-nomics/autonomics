//! End-to-end integration tests that call the **real** Crossref REST API.
//!
//! These tests require network access and are therefore `#[ignore]` by default.
//! Run them explicitly with:
//!
//! ```bash
//! cargo test -p crossref --test e2e -- --ignored --nocapture
//! ```

use agentik_types::ToolResultContent;
use crossref::client::{ListQuery, WorksQuery};
use crossref::{CrossrefClient, crossref_registrations};
use serial_test::serial;

/// A well-known DOI that is extremely unlikely to disappear.
const FAMOUS_DOI: &str = "10.1037/0003-066X.59.1.29"; // Baumeister — ego depletion

/// Helper to extract the text from a tool result.
fn tool_text(result: &agentik_types::ToolResult) -> &str {
    match &result.content {
        ToolResultContent::Text(s) => s.as_str(),
        _ => "",
    }
}

// ===========================================================================
// 1.  SDK: fetch a single work by DOI
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn sdk_works_by_doi() {
    let client = CrossrefClient::builder()
        .mailto("test@autonomics.dev")
        .build();

    let resp = client.works_by_doi(FAMOUS_DOI).await.expect("DOI fetch");

    let work = resp.message.expect("message");
    assert!(!work.doi.is_empty(), "DOI should be populated");
    assert!(!work.title.is_empty(), "title should be present");
    assert!(!work.author.is_empty(), "should have authors");
    assert_eq!(work.r#type, "journal-article");

    println!("✓ works_by_doi: {}", work.title_str());
    println!(
        "  Authors: {}",
        work.author
            .iter()
            .map(|a| a.display())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("  Year: {:?}", work.year());
    println!("  Cited by: {}", work.is_referenced_by_count);
}

// ===========================================================================
// 2.  SDK: search /works with free-text query
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn sdk_works_search() {
    let client = CrossrefClient::with_mailto("test@autonomics.dev");

    let query = WorksQuery::new()
        .with_query("CRISPR gene editing")
        .with_rows(5)
        .with_sort("is-referenced-by-count")
        .with_order("desc");

    let resp = client.works(&query).await.expect("works search");
    let msg = &resp.message;

    assert!(msg.total_results > 0, "should find results for CRISPR");
    assert!(msg.items.len() <= 5, "should respect rows=5");
    assert!(!msg.items.is_empty(), "should have items");

    let first = &msg.items[0];
    assert!(!first.doi.is_empty(), "first result should have a DOI");

    println!(
        "✓ works search: {} total results, showing {}",
        msg.total_results,
        msg.items.len()
    );
    println!(
        "  Top result: {} (cited {}×)",
        first.title_str(),
        first.is_referenced_by_count
    );
}

// ===========================================================================
// 3.  SDK: StructuredSearch → WorksQuery translation + live query
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn sdk_structured_search_live() {
    use bib_types::query::{BoolOp, StructuredSearch, YearRange};

    let sq = StructuredSearch {
        keywords: Some(vec!["p53".into(), "cancer".into()]),
        keywords_op: Some(BoolOp::And),
        year_range: Some(YearRange {
            from: 2023,
            to: 2024,
        }),
        ..Default::default()
    };

    let query = crossref::query::to_crossref_works_query(&sq).expect("translation");
    let query = query.with_rows(3);

    let client = CrossrefClient::with_mailto("test@autonomics.dev");
    let resp = client.works(&query).await.expect("structured search");

    let msg = &resp.message;
    assert!(msg.total_results > 0, "should find p53 AND cancer papers");

    // Verify year filter is respected (all results should be 2023 or 2024).
    for work in &msg.items {
        if let Some(y) = work.year() {
            assert!(
                (2023..=2024).contains(&y),
                "work year {y} outside filter range"
            );
        }
    }

    println!(
        "✓ structured search: {} results for p53 AND cancer (2023-2024)",
        msg.total_results
    );
    for w in &msg.items {
        println!("  - {} ({:?})", w.title_str(), w.year());
    }
}

// ===========================================================================
// 4.  SDK: works_agency — check DOI registration agency
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn sdk_works_agency() {
    let client = CrossrefClient::new();

    let resp = client
        .works_agency(FAMOUS_DOI)
        .await
        .expect("agency lookup");
    let info = resp.message.expect("agency message");

    assert_eq!(info.doi.to_lowercase(), FAMOUS_DOI.to_lowercase());
    assert_eq!(info.agency.id, "crossref");

    println!(
        "✓ works_agency: DOI {} → {} ({})",
        info.doi, info.agency.label, info.agency.id
    );
}

// ===========================================================================
// 5.  SDK: list members (publishers)
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn sdk_members_list() {
    let client = CrossrefClient::with_mailto("test@autonomics.dev");

    let list = ListQuery::new().with_query("Elsevier").with_rows(3);
    let resp = client.members(&list).await.expect("members search");

    let msg = &resp.message;
    assert!(!msg.items.is_empty(), "should find Elsevier");

    let first = &msg.items[0];
    // Some members may not report total_doi_count in all API versions.
    println!(
        "✓ members: {} (ID: {}, {} DOIs)",
        first.primary_name, first.id, first.total_doi_count
    );
}

// ===========================================================================
// 6.  SDK: list types
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn sdk_types_list() {
    let client = CrossrefClient::new();
    let resp = client.types().await.expect("types list");

    let msg = &resp.message;
    assert!(!msg.items.is_empty(), "should have work types");

    let ids: Vec<&str> = msg.items.iter().map(|t| t.id.as_str()).collect();
    assert!(
        ids.contains(&"journal-article"),
        "should include journal-article"
    );

    println!("✓ types: {} work types found", msg.items.len());
    for t in msg.items.iter().take(5) {
        println!("  - {} ({})", t.id, t.label);
    }
}

// ===========================================================================
// 7.  Convert: Work → bib_types::Article roundtrip
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn convert_work_to_article_live() {
    let client = CrossrefClient::with_mailto("test@autonomics.dev");
    let resp = client.works_by_doi(FAMOUS_DOI).await.expect("DOI fetch");
    let work = resp.message.expect("message");

    let article = crossref::work_to_article(&work);

    assert_eq!(article.source, bib_types::ArticleSource::CrossRef);
    assert!(!article.title.is_empty());
    assert!(!article.authors.is_empty());
    assert!(article.doi().is_some());

    println!("✓ work_to_article: {}", article.short_cite());
    println!("  DOI: {:?}", article.doi());
    println!("  Journal: {:?}", article.journal);
    println!("  Year: {:?}", article.year);
}

// ===========================================================================
// 8.  Agent tool: registration verification
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn agent_tool_registration() {
    use std::sync::Arc;

    let client = Arc::new(CrossrefClient::with_mailto("test@autonomics.dev"));
    let tools = crossref_registrations(client);

    assert_eq!(tools.len(), 3, "should register 3 crossref tools");

    let names: Vec<&str> = tools.iter().map(|t| t.definition.name.as_str()).collect();
    println!("✓ registered tools: {names:?}");

    assert!(names.contains(&"crossref_search"));
    assert!(names.contains(&"crossref_doi"));
    assert!(names.contains(&"crossref_types"));
}

// ===========================================================================
// 9.  Agent tool: crossref_search via ToolFunction trait
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn agent_tool_search_execution() {
    use std::sync::Arc;

    use agentik_core::tools::ToolFunction;
    use crossref::tools::search::{CrossrefSearchInput, CrossrefSearchTool};

    let client = Arc::new(CrossrefClient::with_mailto("test@autonomics.dev"));
    let tool = CrossrefSearchTool { client };

    let input = CrossrefSearchInput {
        structured: None,
        query: Some("Mendelian randomization".into()),
        rows: Some(3),
        sort: Some("is-referenced-by-count".into()),
        order: Some("desc".into()),
        cursor: None,
    };

    let result = tool.run(input).await.expect("tool execution");
    let output = tool_text(&result);

    assert!(!output.is_empty(), "should produce Markdown output");
    assert!(output.contains("results found"), "should show result count");

    println!("✓ crossref_search tool output (first 500 chars):");
    println!("{}", &output[..output.len().min(500)]);
}

// ===========================================================================
// 10.  Agent tool: crossref_doi via ToolFunction trait
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn agent_tool_doi_execution() {
    use std::sync::Arc;

    use agentik_core::tools::ToolFunction;
    use crossref::tools::doi::{CrossrefDoiInput, CrossrefDoiTool};

    let client = Arc::new(CrossrefClient::with_mailto("test@autonomics.dev"));
    let tool = CrossrefDoiTool { client };

    // Full metadata lookup.
    let input = CrossrefDoiInput {
        doi: FAMOUS_DOI.into(),
        agency: false,
    };
    let result = tool.run(input).await.expect("DOI tool");
    let output = tool_text(&result);
    assert!(output.contains("DOI:"), "should contain DOI field");

    println!("✓ crossref_doi tool output (first 300 chars):");
    println!("{}", &output[..output.len().min(300)]);

    // Agency lookup.
    let input = CrossrefDoiInput {
        doi: FAMOUS_DOI.into(),
        agency: true,
    };
    let result = tool.run(input).await.expect("agency tool");
    let output = tool_text(&result);
    assert!(
        output.contains("crossref"),
        "should identify Crossref as agency"
    );

    println!("✓ crossref_doi agency: {output}");
}

// ===========================================================================
// 11.  DAG node: source_crossref_works produces a DataFrame
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn dag_node_works_dataframe() {
    use std::sync::Arc;

    use arrow_array::{RecordBatch, StringArray};
    use dag_core::dag::{DagNode, NodeInput};
    use dag_core::registry::{NodeCtx, NodeFactory};
    use datafusion::prelude::SessionContext;

    // Build the node via the factory, exactly as the engine would.
    let factory = crossref::nodes::works::CrossrefWorksNodeFactory {};
    let spec = serde_json::json!({
        "query": "genome-wide association",
        "rows": 5,
        "mailto": "test@autonomics.dev"
    });

    let ctx = NodeCtx {
        runtime_env: SessionContext::new().runtime_env(),
        opendal: None,
        resources: std::sync::Arc::new(dag_core::resource_catalog::ResourceCatalog::new(
            std::path::PathBuf::from("."),
        )),
        global_sem: None,
    };

    let mut node = factory
        .build(spec, NodeCtx::clone(&ctx))
        .expect("build node");
    assert_eq!(node.kind(), "source_crossref_works");

    let inputs: &[NodeInput] = &[];
    let reporter = dag_core::dag::node_event::NodeReporter::noop();

    let mut outputs = node
        .execute(&ctx, inputs, &reporter)
        .await
        .expect("node execute");

    let df = outputs.remove(&0).expect("output port 0");
    let batches: Vec<RecordBatch> = df.collect().await.expect("collect dataframe");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(total_rows > 0, "should have at least 1 row");
    assert!(total_rows <= 5, "should respect rows=5");

    // Verify schema has expected columns.
    let schema = batches[0].schema();
    let fields: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(fields.contains(&"doi"), "schema should have 'doi' column");
    assert!(
        fields.contains(&"title"),
        "schema should have 'title' column"
    );
    assert!(
        fields.contains(&"cited_by_count"),
        "schema should have 'cited_by_count'"
    );

    println!(
        "✓ source_crossref_works: {} rows, {} columns",
        total_rows,
        fields.len()
    );
    println!("  Schema: {}", fields.join(", "));

    // Print first row's DOI + title.
    let doi_col = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let title_col = batches[0]
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    println!("  Row 0: {} — {}", doi_col.value(0), title_col.value(0));
}
