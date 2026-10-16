use serde_json::Value;
use serde_json::json;

use crate::error::{HcaError, Result};
use crate::types::{FileHit, Page, ProjectHit, RepositorySample, SampleHit};

pub const DEFAULT_ENDPOINT: &str = "https://service.azul.data.humancellatlas.org";
const PAGE_SIZE: u32 = 50;

#[derive(Debug, Clone)]
pub struct HcaClient {
    http: reqwest::Client,
    endpoint: String,
    catalog: Option<String>,
}

impl Default for HcaClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HcaClient {
    pub fn new() -> Self {
        Self::with_endpoint(
            std::env::var("ENDPOINT_HCA_URL").unwrap_or_else(|_| DEFAULT_ENDPOINT.into()),
        )
        .with_catalog(std::env::var("HCA_CATALOG").ok())
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("autonomics-hca-sdk/0.1 (+https://data.humancellatlas.org)")
                .build()
                .expect("failed to build HCA HTTP client"),
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
            catalog: None,
        }
    }

    pub fn with_catalog(mut self, catalog: Option<String>) -> Self {
        self.catalog = catalog
            .as_deref()
            .map(str::trim)
            .filter(|catalog| !catalog.is_empty())
            .map(str::to_owned);
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    async fn get_page<T>(&self, url: &str, query: &[(&str, String)]) -> Result<Page<T>>
    where
        T: serde::de::DeserializeOwned,
    {
        let response = self.http.get(url).query(query).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(HcaError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(serde_json::from_str(&body)?)
    }

    async fn get_json<T>(&self, url: &str, query: &[(&str, String)]) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let response = self.http.get(url).query(query).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(HcaError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(serde_json::from_str(&body)?)
    }

    fn base_query(&self, size: u32) -> Vec<(&'static str, String)> {
        let mut query = vec![("size", size.to_string())];
        if let Some(catalog) = &self.catalog {
            query.push(("catalog", catalog.clone()));
        }
        query
    }

    pub async fn projects(
        &self,
        query: Option<&str>,
        max_results: Option<usize>,
    ) -> Result<Vec<ProjectHit>> {
        let mut url = format!("{}/index/projects", self.endpoint);
        let mut hits = Vec::new();
        let mut matched = 0_usize;
        loop {
            let request_query = self.base_query(PAGE_SIZE);
            let page: Page<ProjectHit> = self.get_page(&url, &request_query).await?;
            let next = page.pagination.next.clone();
            for hit in page.hits {
                if query
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_none_or(|query| project_hit_matches(&hit, query))
                {
                    matched += 1;
                    hits.push(hit);
                    if max_results.is_some_and(|max| matched >= max) {
                        return Ok(hits);
                    }
                }
            }
            let Some(next) = next else {
                return Ok(hits);
            };
            url = next;
        }
    }

    pub async fn project(&self, project_id: &str) -> Result<ProjectHit> {
        validate_id(project_id)?;
        let mut query = Vec::new();
        if let Some(catalog) = &self.catalog {
            query.push(("catalog", catalog.clone()));
        }
        let hit: ProjectHit = self
            .get_json(
                &format!("{}/index/projects/{project_id}", self.endpoint),
                &query,
            )
            .await?;
        Ok(hit)
    }

    pub async fn files(
        &self,
        project_id: Option<&str>,
        file_format: Option<&str>,
        size: u32,
    ) -> Result<(Vec<FileHit>, u64)> {
        if let Some(project_id) = project_id {
            validate_id(project_id)?;
        }
        let filters = filters_json(project_id, file_format);
        let mut query = self.base_query(size.min(PAGE_SIZE));
        query.push(("filters", filters.to_string()));
        let page: Page<FileHit> = self
            .get_page(&format!("{}/index/files", self.endpoint), &query)
            .await?;
        let total = page.pagination.total.unwrap_or_default();
        Ok((page.hits, total))
    }

    pub async fn samples(
        &self,
        project_id: Option<&str>,
        size: u32,
    ) -> Result<(Vec<SampleHit>, u64)> {
        if let Some(project_id) = project_id {
            validate_id(project_id)?;
        }
        let filters = filters_json(project_id, None);
        let mut query = self.base_query(size.min(PAGE_SIZE));
        query.push(("filters", filters.to_string()));
        let page: Page<SampleHit> = self
            .get_page(&format!("{}/index/samples", self.endpoint), &query)
            .await?;
        let total = page.pagination.total.unwrap_or_default();
        Ok((page.hits, total))
    }
}

fn filters_json(project_id: Option<&str>, file_format: Option<&str>) -> serde_json::Value {
    let mut filters = serde_json::Map::new();
    if let Some(project_id) = project_id.map(str::trim).filter(|value| !value.is_empty()) {
        filters.insert("projectId".into(), json!({"is": [project_id]}));
    }
    if let Some(file_format) = file_format.map(str::trim).filter(|value| !value.is_empty()) {
        filters.insert("fileFormat".into(), json!({"is": [file_format]}));
    }
    Value::Object(filters)
}

fn project_hit_matches(hit: &ProjectHit, query: &str) -> bool {
    let query = query.to_ascii_lowercase();
    let mut haystack = hit.entry_id.to_ascii_lowercase();
    for project in &hit.projects {
        haystack.push_str(&project.project_id.to_ascii_lowercase());
        haystack.push_str(&project.title.to_ascii_lowercase());
        haystack.push_str(&project.shortname.to_ascii_lowercase());
        if let Some(description) = &project.project_description {
            haystack.push_str(&description.to_ascii_lowercase());
        }
    }
    haystack.contains(&query)
}

fn validate_id(value: &str) -> Result<()> {
    let value = value.trim();
    if value.len() < 8
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(HcaError::Param(format!("invalid HCA ID: '{value}'")));
    }
    Ok(())
}
