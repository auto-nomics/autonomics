use crate::error::{Result, ScpError};
use crate::types::{SearchResponse, SiteStudyFilter, StudyDetail};

pub const DEFAULT_ENDPOINT: &str = "https://singlecell.broadinstitute.org/single_cell/api/v1";

#[derive(Debug, Clone)]
pub struct ScpClient {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for ScpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ScpClient {
    pub fn new() -> Self {
        Self::with_endpoint(
            std::env::var("ENDPOINT_SCP_URL").unwrap_or_else(|_| DEFAULT_ENDPOINT.into()),
        )
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("autonomics-scp-sdk/0.1 (+https://singlecell.broadinstitute.org)")
                .build()
                .expect("failed to build SCP HTTP client"),
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
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(ScpError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(serde_json::from_str(&body)?)
    }

    pub async fn site_studies(
        &self,
        filter: &SiteStudyFilter,
    ) -> Result<Vec<crate::types::SiteStudy>> {
        let studies: Vec<crate::types::SiteStudy> = self.get_json("site/studies").await?;
        Ok(crate::types::filter_site_studies(&studies, filter))
    }

    pub async fn search(
        &self,
        query_type: &str,
        terms: &str,
        page: Option<u32>,
    ) -> Result<SearchResponse> {
        if !matches!(query_type, "study" | "gene") {
            return Err(ScpError::Param(
                "search type must be 'study' or 'gene'".into(),
            ));
        }
        if terms.trim().is_empty() {
            return Err(ScpError::Param("search terms must not be empty".into()));
        }
        let path = match page {
            Some(page) => format!(
                "search?type={query_type}&terms={}&page={page}",
                urlencode(terms)
            ),
            None => format!("search?type={query_type}&terms={}", urlencode(terms)),
        };
        self.get_json(&path).await
    }

    pub async fn study(&self, accession: &str) -> Result<StudyDetail> {
        validate_accession(accession)?;
        self.get_json(&format!("site/studies/{}", accession.trim()))
            .await
    }
}

fn urlencode(value: &str) -> String {
    let bytes: Vec<u8> = value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => vec![byte],
            other => format!("%{other:02X}").into_bytes(),
        })
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn validate_accession(value: &str) -> Result<()> {
    let value = value.trim();
    let valid = value.len() > 3
        && value.starts_with("SCP")
        && value[3..].bytes().all(|byte| byte.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        Err(ScpError::Param(format!("invalid SCP accession: '{value}'")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encodes_search_terms() {
        assert_eq!(urlencode("lung cancer"), "lung%20cancer");
    }

    #[test]
    fn validates_accessions() {
        assert!(validate_accession("SCP1").is_ok());
        assert!(validate_accession("SCP").is_err());
        assert!(validate_accession("GSE1").is_err());
    }
}
