use std::collections::BTreeSet;

use reqwest::Url;
use scraper::{Html, Selector};
use serde::Deserialize;

use crate::error::{AllenBrainError, Result};
use crate::types::{BrainArea, Gene, QueryPage, TaxonomyReference, classify_taxonomy_filename};

pub const DEFAULT_API_ENDPOINT: &str = "https://celltypes.brain-map.org/api/v2";
pub const DEFAULT_TAXONOMY_INDEX_URL: &str =
    "https://brain-map.org/our-research/cell-types-taxonomies/taxonomies-index";

#[derive(Debug, Clone)]
pub struct AllenBrainClient {
    http: reqwest::Client,
    endpoint: String,
    taxonomy_index_url: String,
}

#[derive(Debug, Deserialize)]
struct RawApiPage {
    success: bool,
    #[serde(default)]
    total_rows: Option<u64>,
    #[serde(default)]
    msg: serde_json::Value,
}

impl Default for AllenBrainClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AllenBrainClient {
    pub fn new() -> Self {
        Self::with_endpoint(DEFAULT_API_ENDPOINT)
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("autonomics-allen-brain-sdk/0.1 (+https://brain-map.org)")
                .build()
                .expect("failed to build Allen Brain HTTP client"),
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
            taxonomy_index_url: DEFAULT_TAXONOMY_INDEX_URL.to_owned(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn areas(
        &self,
        graph_id: u64,
        start_row: u32,
        rows: u32,
    ) -> Result<QueryPage<BrainArea>> {
        let criteria = format!(
            "model::Structure,rma::criteria,[graph_id$eq{graph_id}],\
             rma::options[start_row$eq{start_row}][num_rows$eq{rows}]"
        );
        self.query(&criteria).await
    }

    pub async fn genes(&self, symbol: &str, start_row: u32, rows: u32) -> Result<QueryPage<Gene>> {
        let symbol = symbol.trim();
        if symbol.is_empty() || symbol.contains('\'') {
            return Err(AllenBrainError::Param(
                "gene symbol must be non-empty and cannot contain an apostrophe".to_owned(),
            ));
        }
        let criteria = format!(
            "model::Gene,rma::criteria,[acronym$eq'{symbol}'],\
             rma::options[start_row$eq{start_row}][num_rows$eq{rows}]"
        );
        self.query(&criteria).await
    }

    pub async fn taxonomies(
        &self,
        species: Option<&str>,
        assay: Option<&str>,
    ) -> Result<Vec<TaxonomyReference>> {
        let response = self.http.get(&self.taxonomy_index_url).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(AllenBrainError::Status {
                status: status.as_u16(),
                body,
            });
        }
        let mut references = parse_taxonomy_references(&body);
        if let Some(species) = species.map(str::trim).filter(|value| !value.is_empty()) {
            references.retain(|reference| {
                reference
                    .species
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(species))
            });
        }
        if let Some(assay) = assay.map(str::trim).filter(|value| !value.is_empty()) {
            references.retain(|reference| {
                reference
                    .assay
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(assay))
            });
        }
        Ok(references)
    }

    async fn query<T>(&self, criteria: &str) -> Result<QueryPage<T>>
    where
        T: serde::de::DeserializeOwned,
    {
        let mut url = Url::parse(&format!("{}/data/query.json", self.endpoint))
            .map_err(|error| AllenBrainError::Param(format!("invalid API endpoint: {error}")))?;
        url.query_pairs_mut().append_pair("criteria", criteria);
        let response = self.http.get(url).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(AllenBrainError::Status {
                status: status.as_u16(),
                body,
            });
        }
        let page: RawApiPage = serde_json::from_str(&body)?;
        if !page.success {
            let message = match &page.msg {
                serde_json::Value::String(value) => value.clone(),
                value => value.to_string(),
            };
            return Err(AllenBrainError::Api(message));
        }
        let rows: Vec<T> = serde_json::from_value(page.msg.clone())?;
        let total_rows = page.total_rows.unwrap_or(rows.len() as u64);
        Ok(QueryPage { rows, total_rows })
    }
}

pub fn parse_taxonomy_references(html: &str) -> Vec<TaxonomyReference> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("a[href$='.h5ad']").expect("valid H5AD selector");
    let mut urls = BTreeSet::new();
    for element in document.select(&selector) {
        if let Some(href) = element.value().attr("href") {
            urls.insert(href.to_owned());
        }
    }
    urls.into_iter()
        .filter_map(|url| {
            let filename = Url::parse(&url)
                .ok()
                .and_then(|parsed| {
                    parsed
                        .path_segments()
                        .and_then(|mut segments| segments.next_back().map(str::to_owned))
                })
                .filter(|filename| filename.ends_with(".h5ad"))?;
            let mut reference = classify_taxonomy_filename(&filename);
            reference.download_url = url;
            Some(reference)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_official_taxonomy_reference_links() {
        let html = r#"<html><body>
            <a href="https://example.test/Mouse_VISp_ALM_SMART_seq_04042025.h5ad">Mouse</a>
            <a href="https://example.test/not-a-reference.csv">CSV</a>
        </body></html>"#;
        let references = parse_taxonomy_references(html);
        assert_eq!(references.len(), 1);
        assert_eq!(
            references[0].filename,
            "Mouse_VISp_ALM_SMART_seq_04042025.h5ad"
        );
        assert_eq!(references[0].release_date.as_deref(), Some("2025-04-04"));
    }
}
