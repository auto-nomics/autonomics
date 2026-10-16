use crate::error::{HclError, Result};
use crate::types::{Article, ArticleFile, Version, classify_file};

pub const DEFAULT_ARTICLE_ID: u64 = 7_235_471;
pub const DEFAULT_API_ENDPOINT: &str = "https://api.figshare.com/v2";

#[derive(Debug, Clone)]
pub struct HclClient {
    http: reqwest::Client,
    endpoint: String,
    article_id: u64,
}

impl Default for HclClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HclClient {
    pub fn new() -> Self {
        Self::with_endpoint(
            std::env::var("ENDPOINT_HCL_URL").unwrap_or_else(|_| DEFAULT_API_ENDPOINT.into()),
        )
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("autonomics-hcl-sdk/0.1 (+https://figshare.com/articles/dataset/HCL_DGE_Data/7235471)")
                .build()
                .expect("failed to build HCL HTTP client"),
            endpoint: endpoint.into().trim_end_matches('/').to_owned(),
            article_id: DEFAULT_ARTICLE_ID,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn versions(&self) -> Result<Vec<Version>> {
        self.get_json(&format!("articles/{}/versions", self.article_id))
            .await
    }

    pub async fn article(&self, version: Option<u32>) -> Result<Article> {
        let path = match version {
            Some(version) if version > 0 => {
                format!("articles/{}/versions/{version}", self.article_id)
            }
            Some(invalid) => {
                return Err(HclError::Param(format!("invalid HCL version: {invalid}")));
            }
            None => format!("articles/{}", self.article_id),
        };
        self.get_json(&path).await
    }

    pub async fn files(
        &self,
        version: Option<u32>,
        asset_type: Option<&str>,
    ) -> Result<(Article, Vec<ArticleFile>)> {
        let article = self.article(version).await?;
        let mut files = article.files.clone();
        if let Some(asset_type) = asset_type.map(str::trim).filter(|value| !value.is_empty()) {
            files.retain(|file| classify_file(&file.name).eq_ignore_ascii_case(asset_type));
        }
        Ok((article, files))
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
            return Err(HclError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(serde_json::from_str(&body)?)
    }
}
