use crate::error::{Result, TischError};
use crate::types::{CellType, Dataset, DatasetFilter};

pub const DEFAULT_ENDPOINT: &str = "http://tisch.comp-genomics.org";

#[derive(Debug, Clone)]
pub struct TischClient {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for TischClient {
    fn default() -> Self {
        Self::new()
    }
}

impl TischClient {
    pub fn new() -> Self {
        Self::with_endpoint(
            std::env::var("ENDPOINT_TISCH_URL").unwrap_or_else(|_| DEFAULT_ENDPOINT.into()),
        )
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("autonomics-tisch-sdk/0.1 (+http://tisch.comp-genomics.org)")
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()
                .expect("failed to build TISCH HTTP client"),
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn datasets(&self, filter: &DatasetFilter) -> Result<Vec<Dataset>> {
        let html = self.get_html("gallery").await?;
        let datasets = crate::types::parse_datasets(&html)?;
        Ok(crate::types::filter_datasets(&datasets, filter))
    }

    pub async fn cell_types(&self) -> Result<Vec<CellType>> {
        let html = self.get_html("documentation").await?;
        crate::types::parse_cell_types(&html)
    }

    async fn get_html(&self, path: &str) -> Result<String> {
        let response = self
            .http
            .get(format!("{}/{path}", self.endpoint))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(TischError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(body)
    }
}
