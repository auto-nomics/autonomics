use crate::error::{NemoError, Result};
use crate::types::{Collection, CollectionsResponse, Dataset, Pagination, SearchResponse};

pub const DEFAULT_API_ENDPOINT: &str = "https://nemoanalytics.org";

#[derive(Debug, Clone)]
pub struct NemoClient {
    http: reqwest::Client,
    endpoint: String,
}

#[derive(Debug, Clone, Default)]
pub struct DatasetSearch {
    pub query: Option<String>,
    pub dataset_type: Option<String>,
    pub page: u32,
    pub limit: u32,
}

impl Default for NemoClient {
    fn default() -> Self {
        Self::new()
    }
}

impl NemoClient {
    pub fn new() -> Self {
        Self::with_endpoint(DEFAULT_API_ENDPOINT)
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("autonomics-nemo-sdk/0.1 (+https://nemoanalytics.org)")
                .build()
                .expect("failed to build NeMO HTTP client"),
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn collections(&self) -> Result<Vec<Collection>> {
        let response: CollectionsResponse = self
            .post_form(
                "cgi/get_user_layouts.cgi",
                &[("no_domain", "0"), ("include_members", "0")],
            )
            .await?;
        let mut collections = response.domain_layouts;
        collections.extend(response.user_layouts);
        collections.extend(response.shared_layouts);
        Ok(collections)
    }

    pub async fn datasets(&self, search: DatasetSearch) -> Result<(Vec<Dataset>, Pagination)> {
        if search.page == 0 || search.limit == 0 {
            return Err(NemoError::Param(
                "page and limit must be greater than zero".to_owned(),
            ));
        }
        let mut form: Vec<(&str, String)> = vec![
            ("page", search.page.to_string()),
            ("limit", search.limit.to_string()),
            ("sort_by", "date_added".to_owned()),
        ];
        if let Some(query) = search
            .query
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            form.push(("search_terms", query.to_owned()));
        }
        if let Some(dataset_type) = search
            .dataset_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            form.push(("dtypes", dataset_type.to_owned()));
        }
        let borrowed = form
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect::<Vec<_>>();
        let response: SearchResponse = self.post_form("cgi/search_datasets.cgi", &borrowed).await?;
        if response.success != 1 {
            return Err(NemoError::Api(response.problem));
        }
        Ok((response.datasets, response.pagination))
    }

    pub async fn dataset(&self, dataset_id: &str) -> Result<Dataset> {
        let dataset_id = dataset_id.trim();
        if dataset_id.is_empty() {
            return Err(NemoError::Param("dataset_id is required".to_owned()));
        }
        self.post_form(
            "cgi/get_dataset_info.cgi",
            &[("dataset_id", dataset_id), ("include_shape", "1")],
        )
        .await
    }

    async fn post_form<T>(&self, path: &str, form: &[(&str, &str)]) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let response = self
            .http
            .post(format!("{}/{path}", self.endpoint))
            .form(form)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(NemoError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(serde_json::from_str(&body)?)
    }
}
