use crate::types::{Pagination, Protocol, ProtocolStats, ProtocolStep, ProtocolSummary, Reagent};

pub(crate) fn format_summaries(items: &[ProtocolSummary], page: &Pagination) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "protocols.io results: {} of {} total\n\n",
        items.len(),
        page.total_results.unwrap_or(items.len() as u64)
    ));
    for item in items {
        out.push_str(&format!(
            "- {} [{}] ID={} URI={} DOI={}\n  Steps: {} | Version: {} | Published: {}\n",
            item.title.as_deref().unwrap_or("(untitled)"),
            item.url
                .as_deref()
                .or(item.uri.as_deref())
                .unwrap_or("(no link)"),
            item.id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".into()),
            item.uri.as_deref().unwrap_or("-"),
            item.doi.as_deref().unwrap_or("-"),
            item.number_of_steps
                .or(item.stats.as_ref().and_then(|stats| stats.number_of_steps))
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            item.version_id
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            item.published_on
                .map(unix_to_iso)
                .unwrap_or_else(|| "-".into()),
        ));
    }
    out.push_str(&format!(
        "\nPage {} of {}; page size {}.",
        page.current_page.unwrap_or(1),
        page.total_pages.unwrap_or(1),
        page.page_size
            .as_ref()
            .and_then(|value| value.as_u64())
            .unwrap_or(items.len() as u64)
    ));
    out
}

pub(crate) fn format_protocol(item: &Protocol) -> String {
    let summary = &item.summary;
    let stats = summary.stats.as_ref();
    format!(
        "# {}\n\nID: {}\nGUID: {}\nURI: {}\nDOI: {}\nURL: {}\nVersion: {}\nPublished: {}\nCreator: {}\nAuthors: {}\nPublic: {}\n\n## Description\n{}\n\n## Before Start\n{}\n\n## Guidelines\n{}\n\n## Warning\n{}\n\n## Materials\n{}\n\n## Statistics\n{}",
        summary.title.as_deref().unwrap_or("(untitled)"),
        summary
            .id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "-".into()),
        summary.guid.as_deref().unwrap_or("-"),
        summary.uri.as_deref().unwrap_or("-"),
        summary.doi.as_deref().unwrap_or("-"),
        summary
            .url
            .as_deref()
            .or(summary.uri.as_deref())
            .unwrap_or("-"),
        summary
            .version_id
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".into()),
        summary
            .published_on
            .map(unix_to_iso)
            .unwrap_or_else(|| "-".into()),
        summary
            .creator
            .as_ref()
            .and_then(|user| user.label())
            .unwrap_or("-"),
        summary
            .authors
            .iter()
            .filter_map(|user| user.label())
            .collect::<Vec<_>>()
            .join("; "),
        summary
            .public
            .as_ref()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".into()),
        markdown_text(item.description.as_ref()),
        markdown_text(item.before_start.as_ref()),
        markdown_text(item.guidelines.as_ref()),
        markdown_text(item.warning.as_ref()),
        markdown_text(item.materials_text.as_ref()),
        format_stats(stats),
    )
}

pub(crate) fn format_steps(identifier: &str, steps: &[ProtocolStep]) -> String {
    let ordered = crate::convert::ordered_steps(steps);
    let mut out = format!("# Steps for {identifier}\n\n");
    for (index, step) in ordered.iter().enumerate() {
        out.push_str(&format!(
            "## Step {}\n\n{}\n\n",
            index + 1,
            markdown(step.step.as_ref()).unwrap_or("(empty step)")
        ));
    }
    out
}

pub(crate) fn format_reagents(items: &[Reagent], page: Option<&Pagination>) -> String {
    let mut out = String::new();
    for item in items {
        out.push_str(&format!(
            "- {} | ID={} | SKU={} | Vendor={} | Product={}\n",
            item.name.as_deref().unwrap_or("(unnamed)"),
            item.id.unwrap_or_default(),
            item.sku.as_deref().unwrap_or("-"),
            item.vendor
                .as_ref()
                .and_then(|vendor| vendor.name.as_deref())
                .unwrap_or("-"),
            item.url.as_deref().unwrap_or("-"),
        ));
    }
    if let Some(page) = page {
        out.push_str(&format!(
            "\nPage {} of {} | Total results: {}",
            page.current_page.unwrap_or(1),
            page.total_pages.unwrap_or(1),
            page.total_results.unwrap_or(items.len() as u64)
        ));
    }
    out
}

fn format_stats(stats: Option<&ProtocolStats>) -> String {
    let Some(stats) = stats else {
        return "(not provided)".to_owned();
    };
    format!(
        "views={}; steps={}; bookmarks={}; comments={}; exports={}; runs={}; votes={}; reagents={}; equipment={}",
        show(stats.number_of_views),
        show(stats.number_of_steps),
        show(stats.number_of_bookmarks),
        show(stats.number_of_comments),
        show(stats.number_of_exports),
        show(stats.number_of_runs),
        show(stats.number_of_votes),
        show(stats.number_of_reagents),
        show(stats.number_of_equipments),
    )
}

fn markdown(value: Option<&serde_json::Value>) -> Option<&str> {
    value?.as_str()
}

fn markdown_text(value: Option<&serde_json::Value>) -> &str {
    markdown(value).unwrap_or("(not provided)")
}

fn show(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".into())
}

fn unix_to_iso(value: i64) -> String {
    chrono::DateTime::from_timestamp(value, 0)
        .map(|value| value.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| value.to_string())
}
