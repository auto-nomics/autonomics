use serde_json::{Value, json};

use crate::error::{HubmapError, Result};
use crate::types::{HubmapSearchFilter, SearchResponse};

pub const DEFAULT_SEARCH_ENDPOINT: &str = "https://search.api.hubmapconsortium.org/v3/search";
pub const DEFAULT_ENTITY_ENDPOINT: &str = "https://entity.api.hubmapconsortium.org";

#[derive(Debug, Clone)]
pub struct HubmapClient {
    http: reqwest::Client,
    search_endpoint: String,
    entity_endpoint: String,
    token: Option<String>,
}

impl Default for HubmapClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HubmapClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("autonomics-hubmap-sdk/0.1 (+https://hubmapconsortium.org)")
                .build()
                .expect("failed to build HuBMAP HTTP client"),
            search_endpoint: std::env::var("ENDPOINT_HUBMAP_SEARCH_URL")
                .unwrap_or_else(|_| DEFAULT_SEARCH_ENDPOINT.into()),
            entity_endpoint: std::env::var("ENDPOINT_HUBMAP_ENTITY_URL")
                .unwrap_or_else(|_| DEFAULT_ENTITY_ENDPOINT.into()),
            token: std::env::var("HUBMAP_TOKEN").ok(),
        }
    }

    pub fn search_endpoint(&self) -> &str {
        &self.search_endpoint
    }

    pub fn entity_endpoint(&self) -> &str {
        &self.entity_endpoint
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    pub async fn search(&self, filter: &HubmapSearchFilter) -> Result<SearchResponse> {
        let body = search_request(filter);
        let response = self
            .authorize(self.http.post(&self.search_endpoint))
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(HubmapError::Status {
                status: status.as_u16(),
                body: text,
            });
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn entity(&self, identifier: &str) -> Result<crate::types::Entity> {
        validate_identifier(identifier)?;
        let response = self
            .authorize(self.http.get(format!(
                "{}/entities/{}",
                self.entity_endpoint,
                identifier.trim()
            )))
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(HubmapError::Status {
                status: status.as_u16(),
                body: text,
            });
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn entity_from_search(&self, identifier: &str) -> Result<crate::types::Entity> {
        validate_identifier(identifier)?;
        let identifier = identifier.trim();
        let filter = HubmapSearchFilter {
            query: Some(identifier.to_owned()),
            size: 10,
            ..Default::default()
        };
        let response = self.search(&filter).await?;
        response
            .hits
            .hits
            .into_iter()
            .map(|hit| hit.source)
            .find(|entity| entity.uuid == identifier || entity.hubmap_id == identifier)
            .ok_or_else(|| HubmapError::NotFound(format!("HuBMAP entity '{identifier}'")))
    }

    pub async fn globus_url(&self, identifier: &str) -> Result<String> {
        validate_identifier(identifier)?;
        let response = self
            .authorize(self.http.get(format!(
                "{}/entities/{}/globus-url",
                self.entity_endpoint,
                identifier.trim()
            )))
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(HubmapError::Status {
                status: status.as_u16(),
                body: text,
            });
        }
        let url = text.trim().trim_matches('"').to_owned();
        if url.is_empty() {
            return Err(HubmapError::NotFound(format!(
                "Globus URL for HuBMAP entity '{identifier}'"
            )));
        }
        Ok(url)
    }
}

pub(crate) fn search_request(filter: &HubmapSearchFilter) -> Value {
    let mut must = Vec::<Value>::new();
    if let Some(query) = nonempty(&filter.query) {
        must.push(json!({
            "multi_match": {
                "query": query,
                "fields": [
                    "uuid",
                    "hubmap_id",
                    "title",
                    "description",
                    "dataset_type",
                    "data_types",
                    "origin_samples.organ",
                    "metadata.assay_type",
                    "donor.group_name"
                ],
                "type": "best_fields"
            }
        }));
    }
    if must.is_empty() {
        must.push(json!({"match_all": {}}));
    }

    let mut bool_query = json!({"must": must});
    let mut filters = Vec::<Value>::new();
    if let Some(entity_type) = nonempty(&filter.entity_type) {
        filters.push(json!({"term": {"entity_type.keyword": entity_type}}));
    }
    if let Some(organ) = nonempty(&filter.organ) {
        filters.push(json!({"match": {"origin_samples.organ": organ}}));
    }
    if let Some(assay) = nonempty(&filter.assay) {
        filters.push(json!({"match": {"metadata.assay_type": assay}}));
        filters.push(json!({"match": {"data_types": assay}}));
    }
    if let Some(status) = nonempty(&filter.status) {
        filters.push(json!({"term": {"status.keyword": status}}));
    }
    if !filters.is_empty() {
        bool_query["filter"] = Value::Array(filters);
    }

    json!({
        "query": {"bool": bool_query},
        "from": filter.from.unwrap_or(0),
        "size": filter.size,
        "track_total_hits": true
    })
}

fn nonempty(value: &Option<String>) -> Option<&str> {
    let value = value.as_deref()?.trim();
    (!value.is_empty()).then_some(value)
}

fn validate_identifier(value: &str) -> Result<()> {
    let value = value.trim();
    let valid = (value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        || (value.starts_with("HBM")
            && value.len() >= 12
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.'));
    if valid {
        Ok(())
    } else {
        Err(HubmapError::Param(format!(
            "invalid HuBMAP identifier: '{value}'"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_query_with_filters() {
        let filter = HubmapSearchFilter {
            query: Some("spleen".into()),
            entity_type: Some("Dataset".into()),
            organ: Some("SP".into()),
            status: Some("Published".into()),
            size: 20,
            ..Default::default()
        };
        let request = search_request(&filter);
        assert_eq!(request["size"], 20);
        assert_eq!(
            request["query"]["bool"]["filter"]
                .as_array()
                .expect("filters")
                .len(),
            3
        );
    }

    #[test]
    fn validates_identifiers() {
        assert!(validate_identifier("0188cbfed79e0714b0e6ea6466ab8977").is_ok());
        assert!(validate_identifier("HBM537.HTKP.258").is_ok());
        assert!(validate_identifier("not-an-id").is_err());
    }
}
