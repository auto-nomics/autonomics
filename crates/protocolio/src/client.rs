use std::{sync::Arc, time::Duration};

use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;

use crate::error::{ProtocolioError, Result};
use crate::query::{ProtocolListQuery, ProtocolPdfView, ReagentListQuery};
use crate::rate::RateLimiter;
use crate::types::{
    ContentFormat, MaterialsResponse, Protocol, ProtocolListResponse, ProtocolResponse,
    ProtocolStep, Reagent, ReagentListResponse, StepsResponse,
};

pub const DEFAULT_ENDPOINT: &str = "https://www.protocols.io";
pub const ENV_TOKEN: &str = "PROTOCOLS_IO_ACCESS_TOKEN";
pub const ENV_ENDPOINT: &str = "ENDPOINT_PROTOCOLS_IO_URL";
pub const MIN_PAGE_SIZE: u32 = 1;
pub const MAX_PAGE_SIZE: u32 = 100;

const USER_AGENT: &str = "protocolio-rs-sdk/0.1 (+https://apidoc.protocols.io/)";
const API_PERIOD: Duration = Duration::from_millis(670);
const PDF_PERIOD: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct ProtocolioClient {
    http: reqwest::Client,
    endpoint: String,
    token: String,
    api_limiter: Arc<RateLimiter>,
    pdf_limiter: Arc<RateLimiter>,
}

impl ProtocolioClient {
    /// Build a client from `PROTOCOLS_IO_ACCESS_TOKEN` and
    /// `ENDPOINT_PROTOCOLS_IO_URL`.
    pub fn new() -> Result<Self> {
        let token = std::env::var(ENV_TOKEN)
            .map_err(|_| ProtocolioError::MissingToken)?
            .trim()
            .to_owned();
        if token.is_empty() {
            return Err(ProtocolioError::MissingToken);
        }
        let endpoint = std::env::var(ENV_ENDPOINT).unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned());
        Self::with_parts(Self::default_http()?, endpoint, token)
    }

    pub fn from_token(token: impl Into<String>) -> Result<Self> {
        Self::with_parts(Self::default_http()?, DEFAULT_ENDPOINT, token)
    }

    pub fn with_parts(
        http: reqwest::Client,
        endpoint: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self> {
        let token = token.into().trim().to_owned();
        if token.is_empty() {
            return Err(ProtocolioError::MissingToken);
        }
        let endpoint = endpoint.into().trim_end_matches('/').to_owned();
        Url::parse(&endpoint).map_err(|error| {
            ProtocolioError::InvalidParameter(format!("invalid endpoint: {error}"))
        })?;
        Ok(Self {
            http,
            endpoint,
            token,
            api_limiter: Arc::new(RateLimiter::from_period(API_PERIOD)),
            pdf_limiter: Arc::new(RateLimiter::from_period(PDF_PERIOD)),
        })
    }

    fn default_http() -> Result<reqwest::Client> {
        Ok(reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()?)
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn list_protocols(&self, query: &ProtocolListQuery) -> Result<ProtocolListResponse> {
        query.validate()?;
        self.get_json("/api/v3/protocols", &query.pairs()).await
    }

    pub async fn protocol(
        &self,
        identifier: &str,
        last_version: bool,
        format: ContentFormat,
    ) -> Result<Protocol> {
        let mut pairs = vec![("content_format", format.as_str().to_owned())];
        if last_version {
            pairs.push(("last_version", "1".to_owned()));
        }
        let response: ProtocolResponse = self
            .get_json(&format!("/api/v4/protocols/{}", encode(identifier)), &pairs)
            .await?;
        response
            .protocol()
            .cloned()
            .ok_or_else(|| ProtocolioError::MissingField {
                field: "payload or protocol".to_owned(),
            })
    }

    pub async fn steps(
        &self,
        identifier: &str,
        last_version: bool,
        format: ContentFormat,
    ) -> Result<Vec<ProtocolStep>> {
        let mut pairs = vec![("content_format", format.as_str().to_owned())];
        if last_version {
            pairs.push(("last_version", "1".to_owned()));
        }
        let response: StepsResponse = self
            .get_json(
                &format!("/api/v4/protocols/{}/steps", encode(identifier)),
                &pairs,
            )
            .await?;
        Ok(response.steps)
    }

    pub async fn materials(&self, identifier: &str) -> Result<Vec<Reagent>> {
        let response: MaterialsResponse = self
            .get_json(
                &format!("/api/v3/protocols/{}/materials", encode(identifier)),
                &[],
            )
            .await?;
        Ok(response.materials)
    }

    pub async fn list_reagents(&self, query: &ReagentListQuery) -> Result<ReagentListResponse> {
        query.validate()?;
        self.get_json("/api/v3/reagents", &query.pairs()).await
    }

    pub async fn pdf(&self, identifier: &str, view: ProtocolPdfView) -> Result<Vec<u8>> {
        nonempty(identifier, "identifier")?;
        let url = self.url(&format!("/view/{}.pdf", encode(identifier)), &view.pairs())?;
        for attempt in 0..3 {
            self.pdf_limiter.acquire().await;
            let response = self
                .http
                .request(Method::GET, url.clone())
                .bearer_auth(&self.token)
                .send()
                .await?;
            let status = response.status();
            if status.is_success() {
                return Ok(response.bytes().await?.to_vec());
            }
            let body = response.text().await?;
            if status == StatusCode::TOO_MANY_REQUESTS && attempt < 2 {
                tokio::time::sleep(Duration::from_secs(15)).await;
                continue;
            }
            return Err(status_error(status, &body));
        }
        unreachable!("PDF retry loop always returns or errors")
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        pairs: &[(&str, String)],
    ) -> Result<T> {
        let url = self.url(path, pairs)?;
        for attempt in 0..3 {
            self.api_limiter.acquire().await;
            let response = self
                .http
                .request(Method::GET, url.clone())
                .bearer_auth(&self.token)
                .send()
                .await?;
            let status = response.status();
            let body = response.text().await?;
            if status == StatusCode::TOO_MANY_REQUESTS {
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                return Err(ProtocolioError::RateLimited { retry_after: None });
            }
            if !status.is_success() {
                return Err(status_error(status, &body));
            }
            let value: serde_json::Value = serde_json::from_str(&body)?;
            if let Some(status_code) = value.get("status_code").and_then(|v| v.as_u64()) {
                if status_code != 0 {
                    let status_text = value
                        .get("status_text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("request failed");
                    return Err(ProtocolioError::Api {
                        status_code: status_code as u32,
                        status_text: status_text.to_owned(),
                    });
                }
            }
            return Ok(serde_json::from_value(value)?);
        }
        unreachable!("JSON retry loop always returns or errors")
    }

    fn url(&self, path: &str, pairs: &[(&str, String)]) -> Result<Url> {
        let mut url = Url::parse(&format!("{}{}", self.endpoint, path)).map_err(|error| {
            ProtocolioError::InvalidParameter(format!("invalid request URL: {error}"))
        })?;
        url.query_pairs_mut().clear();
        for (name, value) in pairs {
            url.query_pairs_mut().append_pair(name, value);
        }
        Ok(url)
    }
}

fn encode(identifier: &str) -> String {
    identifier
        .trim()
        .split('/')
        .map(percent_path)
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_path(value: &str) -> String {
    let mut out = String::new();
    for byte in value.trim().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn status_error(status: StatusCode, body: &str) -> ProtocolioError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        ProtocolioError::RateLimited { retry_after: None }
    } else {
        ProtocolioError::Status {
            status: status.as_u16(),
            body: body.trim().chars().take(500).collect(),
        }
    }
}

fn nonempty(value: &str, name: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(ProtocolioError::InvalidParameter(format!(
            "{name} must not be empty"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_doi_path_segments() {
        assert_eq!(
            encode("10.17504/protocols.io.baaciaaw/v1"),
            "10.17504/protocols.io.baaciaaw/v1"
        );
        assert_eq!(encode("space id"), "space%20id");
    }

    #[test]
    fn decodes_v3_and_v4_wrappers() {
        let list: ProtocolListResponse = serde_json::from_str(
            r#"{"items":[{"id":1,"title":"Example"}],"pagination":{"total_results":1},"status_code":0}"#,
        )
        .unwrap();
        assert_eq!(list.items.len(), 1);

        let detail: ProtocolResponse =
            serde_json::from_str(r#"{"payload":{"id":2},"status_code":0}"#).unwrap();
        assert_eq!(detail.protocol().unwrap().summary.id, Some(2));
    }
}
