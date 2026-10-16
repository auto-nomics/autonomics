use crate::error::{Result, TabulaSapiensError};
use crate::types::{Article, Version, classify_file};

pub const DEFAULT_ARTICLE_ID: u64 = 14_267_219;
pub const DEFAULT_API_ENDPOINT: &str = "https://api.figshare.com/v2";

#[derive(Debug, Clone)]
pub struct TabulaSapiensClient {
    http: reqwest::Client,
    endpoint: String,
    article_id: u64,
}

impl Default for TabulaSapiensClient {
    fn default() -> Self {
        Self::new()
    }
}

impl TabulaSapiensClient {
    pub fn new() -> Self {
        Self::with_endpoint(
            std::env::var("ENDPOINT_TABULA_SAPIENS_URL")
                .unwrap_or_else(|_| DEFAULT_API_ENDPOINT.into()),
        )
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(
                    "autonomics-tabula-sapiens-sdk/0.1 (+https://tabula-sapiens.sf.czbiohub.org)",
                )
                .build()
                .expect("failed to build Tabula Sapiens HTTP client"),
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
                return Err(TabulaSapiensError::Param(format!(
                    "invalid Tabula Sapiens version: {invalid}"
                )));
            }
            None => format!("articles/{}", self.article_id),
        };
        self.get_json(&path).await
    }

    pub async fn files(
        &self,
        version: Option<u32>,
        asset_type: Option<&str>,
    ) -> Result<(Article, Vec<crate::types::ArticleFile>)> {
        let article = self.article(version).await?;
        let mut files = article.files.clone();
        if let Some(asset_type) = asset_type.map(str::trim).filter(|value| !value.is_empty()) {
            files.retain(|file| classify_file(&file.name).0.eq_ignore_ascii_case(asset_type));
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
            return Err(TabulaSapiensError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(serde_json::from_str(&body)?)
    }
}
