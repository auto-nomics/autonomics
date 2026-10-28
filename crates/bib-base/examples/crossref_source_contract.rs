//! Bounded, public Crossref contract probe. No environment credentials, private
//! queries or library records are read. Prints IDs/counts only, never abstracts.
use bib_base::query::{CrossrefSource, LiteratureSource};
use bib_types::{IdKind, discovery::*};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
const IDS: [&str; 2] = ["10.1038/nature12373", "10.1038/nature14539"];
fn request(sort: DiscoverySort) -> DiscoveryRequest {
    DiscoveryRequest {
        sources: vec!["crossref".into()],
        native: Some(NativeSearch {
            source: "crossref".into(),
            parameters: [(
                "filter".into(),
                IDS.iter()
                    .map(|id| format!("doi:{id}"))
                    .collect::<Vec<_>>()
                    .join(","),
            )]
            .into(),
        }),
        sort,
        page_size: 1,
        ..Default::default()
    }
}
async fn page(
    source: &CrossrefSource,
    plan: &SourcePlan,
    cursor: Option<&str>,
) -> Result<SourcePage, Box<dyn std::error::Error>> {
    Ok(tokio::time::timeout(
        Duration::from_secs(20),
        source.discovery_page(plan, cursor, 1),
    )
    .await??)
}
fn ids(page: &SourcePage) -> Vec<String> {
    page.articles
        .iter()
        .flat_map(|a| {
            a.identifiers
                .iter()
                .filter(|id| id.kind == IdKind::Doi)
                .map(|id| id.value.to_ascii_lowercase())
        })
        .collect()
}
async fn ordered(
    source: &CrossrefSource,
    sort: DiscoverySort,
    expected: [&str; 2],
) -> Result<Value, Box<dyn std::error::Error>> {
    let plan = source.plan_discovery(&request(sort))?;
    let first = page(source, &plan, None).await?;
    let cursor = first
        .next_cursor
        .as_deref()
        .ok_or("two DOI records must have a continuation")?;
    let second = page(source, &plan, Some(cursor)).await?;
    if ids(&first) != [expected[0]] || ids(&second) != [expected[1]] || second.next_cursor.is_some()
    {
        return Err("Crossref publication order or expected exact DOI range differs".into());
    }
    for p in [&first, &second] {
        if p.executed_parameters.contains_key("cursor") {
            return Err("publication sort used cursor".into());
        }
    }
    Ok(
        json!({"sort":sort,"pages":[{"ids":ids(&first),"executed":first.executed_parameters,"total":first.total},{"ids":ids(&second),"executed":second.executed_parameters,"total":second.total}],"passed":true}),
    )
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(
        crossref::CrossrefClient::builder()
            .base_url("https://api.crossref.org")
            .user_agent("Autonomics-public-source-contract/0.1")
            .build(),
    );
    let source = CrossrefSource::new(client.clone());
    let newest = ordered(&source, DiscoverySort::Newest, [IDS[1], IDS[0]]).await?;
    let oldest = ordered(&source, DiscoverySort::Oldest, IDS).await?;
    let plan = source.plan_discovery(&request(DiscoverySort::Relevance))?;
    let mut cursor = None;
    let mut obtained = Vec::new();
    let mut pages = Vec::new();
    for _ in 0..4 {
        let p = page(&source, &plan, cursor.as_deref()).await?;
        if p.executed_parameters.contains_key("offset")
            || p.executed_parameters.get("sort").map(String::as_str) != Some("indexed")
        {
            return Err("filter-only cursor traversal changed its declared ordering".into());
        }
        obtained.extend(ids(&p));
        pages.push(json!({"ids":ids(&p),"executed":p.executed_parameters,"total":p.total,"continued":p.next_cursor.is_some()}));
        cursor = p.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    obtained.sort();
    if cursor.is_some() || obtained != IDS {
        return Err(
            "indexed traversal did not exhaust the two exact DOI records within the probe budget"
                .into(),
        );
    }
    let report = json!({"schema":"bib-crossref-public-source-contract-v1","observed_at":chrono::Utc::now().to_rfc3339(),"public_endpoint":"https://api.crossref.org","credentials_used":false,"private_library_or_queries_used":false,"requested_dois":IDS,"publication_newest":newest,"publication_oldest":oldest,"filter_only_indexed_cursor":{"pages":pages,"passed":true},"passed":true,"scope":"Small public exact-DOI transport and ordering check, not relevance or universal upstream coverage"});
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
