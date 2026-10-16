use crate::error::{CellxgeneError, Result};
use crate::types::{
    AssetResponse, Collection, CollectionDetail, Dataset, DatasetAsset, DatasetFilter,
};

pub const DEFAULT_ENDPOINT: &str = "https://api.cellxgene.cziscience.com/dp/v1";

#[derive(Debug, Clone)]
pub struct CellxgeneClient {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for CellxgeneClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CellxgeneClient {
    pub fn new() -> Self {
        Self::with_endpoint(
            std::env::var("ENDPOINT_CELLXGENE_URL").unwrap_or_else(|_| DEFAULT_ENDPOINT.into()),
        )
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("autonomics-cellxgene-sdk/0.1 (+https://cellxgene.cziscience.com)")
                .build()
                .expect("failed to build CELLxGENE HTTP client"),
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    async fn get_json<T>(&self, path: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let response = self
            .http
            .get(format!("{}/{path}", self.endpoint))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(CellxgeneError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(serde_json::from_str(&body)?)
    }

    pub async fn collections(&self) -> Result<Vec<Collection>> {
        self.get_json("collections/index").await
    }

    pub async fn collection(&self, collection_id: &str) -> Result<CollectionDetail> {
        validate_id(collection_id, "collection_id")?;
        self.get_json(&format!("collections/{collection_id}")).await
    }

    pub async fn dataset_index(&self) -> Result<Vec<Dataset>> {
        self.get_json("datasets/index").await
    }

    pub async fn datasets(&self, filter: &DatasetFilter) -> Result<Vec<Dataset>> {
        let datasets = self.dataset_index().await?;
        Ok(crate::types::filter_datasets(&datasets, filter))
    }

    pub async fn dataset(&self, dataset_id: &str) -> Result<Dataset> {
        validate_id(dataset_id, "dataset_id")?;
        let datasets = self.dataset_index().await?;
        datasets
            .into_iter()
            .find(|dataset| dataset.id == dataset_id)
            .ok_or_else(|| CellxgeneError::NotFound(format!("CELLxGENE dataset {dataset_id}")))
    }

    pub async fn dataset_assets(&self, dataset_id: &str) -> Result<Vec<DatasetAsset>> {
        validate_id(dataset_id, "dataset_id")?;
        let response: AssetResponse = self
            .get_json(&format!("datasets/{dataset_id}/assets"))
            .await?;
        Ok(response.assets)
    }
}

fn validate_id(value: &str, field: &str) -> Result<()> {
    let value = value.trim();
    if value.len() < 8
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(CellxgeneError::Param(format!("invalid {field}: '{value}'")));
    }
    Ok(())
}
