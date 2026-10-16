use crate::error::{Result, SceaError};
use crate::types::{ExperimentFilter, ExperimentIndex, ExperimentPageContent, MarkerGene};

pub const DEFAULT_ENDPOINT: &str = "https://www.ebi.ac.uk/gxa/sc";

#[derive(Debug, Clone)]
pub struct SceaClient {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for SceaClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SceaClient {
    pub fn new() -> Self {
        Self::with_endpoint(
            std::env::var("ENDPOINT_SCEA_URL").unwrap_or_else(|_| DEFAULT_ENDPOINT.into()),
        )
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("autonomics-scea-sdk/0.1 (+https://www.ebi.ac.uk/gxa/sc)")
                .build()
                .expect("failed to build SCEA HTTP client"),
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
            return Err(SceaError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(serde_json::from_str(&body)?)
    }

    pub async fn experiments(
        &self,
        filter: &ExperimentFilter,
    ) -> Result<Vec<crate::types::Experiment>> {
        let index: ExperimentIndex = self.get_json("json/experiments").await?;
        Ok(crate::types::filter_experiments(&index.experiments, filter))
    }

    pub async fn experiment_page(&self, accession: &str) -> Result<ExperimentPageContent> {
        validate_accession(accession)?;
        let response = self
            .http
            .get(format!(
                "{}/experiments/{}/results",
                self.endpoint,
                accession.trim()
            ))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(SceaError::Status {
                status: status.as_u16(),
                body,
            });
        }
        extract_page_content(&body)
    }

    pub async fn marker_genes(&self, accession: &str, k: u32) -> Result<Vec<MarkerGene>> {
        validate_accession(accession)?;
        self.get_json(&format!(
            "json/experiments/{}/marker-genes/clusters?k={k}",
            accession.trim()
        ))
        .await
    }
}

pub(crate) fn extract_page_content(html: &str) -> Result<ExperimentPageContent> {
    let render_start = html
        .find("experimentPage.render(")
        .ok_or_else(|| SceaError::Param("experiment page render call not found".into()))?;
    let content_marker = "content: ";
    let start = html[render_start..]
        .find(content_marker)
        .ok_or_else(|| SceaError::Param("experiment page content not found".into()))?
        + render_start
        + content_marker.len();
    let wrapper_marker = "}, 'experiment-page');";
    let wrapper_start = html[start..]
        .find(wrapper_marker)
        .ok_or_else(|| SceaError::Param("experiment page JSON terminator not found".into()))?
        + start;
    let json_end = html[start..wrapper_start]
        .rfind('}')
        .ok_or_else(|| SceaError::Param("experiment page JSON object not found".into()))?
        + start
        + 1;
    Ok(serde_json::from_str(&html[start..json_end])?)
}

fn validate_accession(value: &str) -> Result<()> {
    let value = value.trim();
    let valid = value.len() > 2
        && value.starts_with("E-")
        && value[2..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(SceaError::Param(format!(
            "invalid SCEA accession: '{value}'"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_embedded_experiment_content() {
        let html = r#"
            experimentPage.render({
              content: {"experimentAccession":"E-CURD-10","accessKey":"","species":"homo sapiens","disclaimer":"","tabs":[]},
            }, 'experiment-page');
        "#;
        let content = extract_page_content(html).unwrap();
        assert_eq!(content.accession, "E-CURD-10");
        assert_eq!(content.species, "homo sapiens");
    }
}
