//! Best-effort open-access full-text fetching from Europe PMC.
//!
//! When an article is saved to the local library ([`crate::BibBase`]), this
//! module attempts to retrieve its full text from Europe PMC's Open Access
//! subset. The fetch is **opportunistic** — failures (paywalled article, no
//! OA copy, network errors, malformed XML) are silently ignored so that the
//! save pipeline never fails because full text was unavailable.
//!
//! ## How it works
//!
//! 1. Resolve the article's PMC ID by searching Europe PMC via PMID or DOI.
//! 2. Call the [`fullTextXML`](https://europepmc.org/RestfulWebService#fullTextXML)
//!    endpoint for the resolved PMC ID.
//! 3. Strip JATS XML tags → plain text.
//! 4. Return a [`FullText`] record tagged `source: OpenAccess` ready for
//!    [`BibBase::upsert_fulltext`].
//!
//! Articles without a PMID or DOI (e.g. pure arXiv preprints) are skipped.

use bib_types::{FileFormat, FullText, FullTextSource, IdKind};
use chrono::Utc;
use europepmc::EuropePmcClient;
use europepmc::types::{ResultType, SearchRequest};
use tracing::debug;

/// Try to fetch an open-access full text for `article` from Europe PMC.
///
/// Creates a one-shot [`EuropePmcClient`]. Use [`try_fetch_fulltext_with`]
/// when you want to share a connection pool.
///
/// Returns `None` on any failure — this is best-effort.
pub async fn try_fetch_fulltext(article: &bib_types::Article) -> Option<FullText> {
    let client = EuropePmcClient::new();
    try_fetch_fulltext_with(&client, article).await
}

/// Same as [`try_fetch_fulltext`] but with a caller-supplied client
/// (useful for sharing a connection pool across batch saves).
pub async fn try_fetch_fulltext_with(
    client: &EuropePmcClient,
    article: &bib_types::Article,
) -> Option<FullText> {
    // 1. If the article already has a PMC identifier, use it directly.
    let pmc_id = if let Some(pmc) = article.identifier(IdKind::Pmc) {
        Some(normalize_pmc_id(pmc))
    } else {
        resolve_pmc_id(client, article).await
    };

    let pmc_id = pmc_id?;
    debug!(article_id = %article.id, pmc_id = %pmc_id, "fetching OA full text from Europe PMC");

    // 2. Fetch full-text XML (JATS).
    let xml = match client.full_text_xml(&pmc_id).await {
        Ok(xml) => xml,
        Err(e) => {
            debug!(pmc_id = %pmc_id, error = %e, "fullTextXML request failed");
            return None;
        }
    };

    // 3. Strip JATS XML tags → plain text.
    let text = strip_xml(&xml);
    if text.trim().is_empty() {
        debug!(pmc_id = %pmc_id, "full text XML yielded empty text");
        return None;
    }

    Some(FullText {
        article_id: article.id.clone(),
        file_path: format!("europepmc:{pmc_id}"),
        file_format: FileFormat::Txt,
        text_content: Some(text),
        source: FullTextSource::OpenAccess,
        file_hash: None,
        file_size: Some(xml.len() as i64),
        uploaded_at: Some(Utc::now()),
    })
}

// ---------------------------------------------------------------------------
// PMC ID resolution
// ---------------------------------------------------------------------------

/// Find the PMC ID for an article by searching Europe PMC.
///
/// Returns the normalised PMC ID (e.g. `"PMC3257301"`) when Europe PMC
/// has a full-text record, or `None` otherwise.
async fn resolve_pmc_id(client: &EuropePmcClient, article: &bib_types::Article) -> Option<String> {
    let query = if let Some(pmid) = article.pmid() {
        format!("PMID:{pmid}")
    } else {
        let doi = article.doi()?;
        format!("DOI:{doi}")
    };

    let resp = client
        .search(
            &SearchRequest::new(&query)
                .result_type(ResultType::Lite)
                .page_size(50),
        )
        .await
        .ok()?;

    // Prefer a result whose source is PMC — its `id` is the PMC number.
    for result in &resp.result_list.results {
        if result.source == "PMC" && !result.id.is_empty() {
            return Some(normalize_pmc_id(&result.id));
        }
    }

    // Fallback: if any result says inPMC=Y, the full text exists in PMC but
    // we didn't get a PMC-source row in the first page. We can't construct
    // the PMC ID from a PMID alone, so bail.
    let has_pmc = resp
        .result_list
        .results
        .iter()
        .any(|r| r.in_pmc.as_deref() == Some("Y"));
    if has_pmc {
        debug!(article_id = %article.id, "inPMC=Y but no PMC-source row found in first page");
    }

    None
}

/// Ensure a PMC ID string includes the `"PMC"` prefix.
fn normalize_pmc_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.starts_with("PMC") {
        trimmed.to_owned()
    } else {
        format!("PMC{trimmed}")
    }
}

// ---------------------------------------------------------------------------
// XML / JATS stripping
// ---------------------------------------------------------------------------

/// Strip XML/JATS tags and collapse whitespace into a single-spaced string.
fn strip_xml(xml: &str) -> String {
    normalize_ws(&strip_tags(xml))
}

fn strip_tags(s: &str) -> String {
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

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_pmc_id_adds_prefix() {
        assert_eq!(normalize_pmc_id("3257301"), "PMC3257301");
        assert_eq!(normalize_pmc_id("PMC3257301"), "PMC3257301");
        assert_eq!(normalize_pmc_id("  PMC42 "), "PMC42");
    }

    #[test]
    fn strip_xml_removes_jats_tags() {
        let xml = r#"<?xml version="1.0"?>
            <article>
                <front><article-meta><title-group>
                    <article-title>CRISPR Gene Editing</article-title>
                </title-group></article-meta></front>
                <body><sec><p>This is the full text body.</p></sec></body>
            </article>"#;
        let text = strip_xml(xml);
        assert!(text.contains("CRISPR Gene Editing"));
        assert!(text.contains("This is the full text body."));
        assert!(!text.contains("<"));
        assert!(!text.contains(">"));
    }

    #[test]
    fn strip_xml_preserves_entities_as_text() {
        let xml = "<p>5 &lt; 10 &amp; x &gt; 3</p>";
        let text = strip_xml(xml);
        // Entities are preserved as literal text after tag stripping.
        assert!(text.contains("5"));
        assert!(text.contains("10"));
    }

    #[test]
    fn strip_xml_empty_input() {
        assert_eq!(strip_xml(""), "");
        assert_eq!(strip_xml("<root></root>"), "");
    }
}
