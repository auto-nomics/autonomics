use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{ChemblError, Result};
use crate::query::ResourceQuery;
use crate::types::{
    Activity, Assay, Document, DrugIndication, Mechanism, Molecule, Page, Status, Target,
};

pub const DEFAULT_ENDPOINT: &str = "https://www.ebi.ac.uk/chembl/api/data";
const MAX_PAGE_SIZE: u32 = 1_000;
const MAX_AUTO_PAGE_SIZE: u32 = 100_000;

pub fn endpoint() -> String {
    std::env::var("ENDPOINT_CHEMBL_URL").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string())
}

#[derive(Debug, Clone)]
pub struct ChEMBLClient {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for ChEMBLClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ChEMBLClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent("chembl-rs-sdk/0.1 (+https://www.ebi.ac.uk/chembl)")
            .build()
            .expect("reqwest client builder");
        Self::with_parts(http, endpoint())
    }

    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent("chembl-rs-sdk/0.1 (+https://www.ebi.ac.uk/chembl)")
            .build()
            .expect("reqwest client builder");
        Self::with_parts(http, endpoint)
    }

    pub fn with_http(http: reqwest::Client) -> Self {
        Self::with_parts(http, endpoint())
    }

    pub fn with_parts(http: reqwest::Client, endpoint: impl Into<String>) -> Self {
        Self {
            http,
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
        }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    async fn get_raw(&self, path: &str, query: Option<&ResourceQuery>) -> Result<Value> {
        let url = format!("{}/{path}", self.endpoint);
        let mut request = self.http.get(&url);
        if let Some(query) = query {
            request = request.query(&query.query_pairs());
        }
        let response = request.send().await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        if !(200..300).contains(&status) {
            return Err(ChemblError::Status { status, body });
        }
        Ok(serde_json::from_str(&body)?)
    }

    async fn get_optional_raw(
        &self,
        path: &str,
        query: Option<&ResourceQuery>,
    ) -> Result<Option<Value>> {
        match self.get_raw(path, query).await {
            Ok(value) => Ok(Some(value)),
            Err(ChemblError::Status { status: 404, .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub async fn status(&self) -> Result<Status> {
        let value = self.get_raw("status.json", None).await?;
        Ok(serde_json::from_value(value)?)
    }

    pub async fn get_json(&self, path: &str, query: Option<&ResourceQuery>) -> Result<Value> {
        self.get_raw(path, query).await
    }

    pub async fn list<T: DeserializeOwned>(
        &self,
        resource: &str,
        query: &ResourceQuery,
    ) -> Result<Page<T>> {
        validate_component(resource)?;
        let value = self
            .get_raw(&format!("{resource}.json"), Some(query))
            .await?;
        parse_page(resource, value)
    }

    pub async fn get<T: DeserializeOwned>(&self, resource: &str, id: &str) -> Result<Option<T>> {
        validate_component(resource)?;
        validate_component(id)?;
        let value = self
            .get_optional_raw(&format!("{resource}/{id}.json"), None)
            .await?;
        Ok(value.map(serde_json::from_value).transpose()?)
    }

    pub async fn search<T: DeserializeOwned>(
        &self,
        resource: &str,
        query_text: &str,
        query: &ResourceQuery,
    ) -> Result<Page<T>> {
        validate_component(resource)?;
        let search_query = ResourceQuery {
            filters: vec![("q".into(), query_text.to_string())],
            ..query.clone()
        };
        let value = self
            .get_raw(&format!("{resource}/search.json"), Some(&search_query))
            .await?;
        parse_page(resource, value)
    }

    pub async fn list_all<T: DeserializeOwned>(
        &self,
        resource: &str,
        query: &ResourceQuery,
    ) -> Result<Vec<T>> {
        let mut query = query.clone();
        if query.limit.is_none() {
            query.limit = Some(MAX_PAGE_SIZE);
        }
        let mut offset = query.offset.unwrap_or(0);
        let mut records = Vec::new();

        loop {
            query.offset = Some(offset);
            let page: Page<T> = self.list(resource, &query).await?;
            let returned = page.records.len();
            records.extend(page.records);
            if returned == 0
                || page.page_meta.next.is_none()
                || records.len() >= MAX_AUTO_PAGE_SIZE as usize
            {
                break;
            }
            offset += returned as u32;
        }

        Ok(records)
    }

    pub async fn molecule(&self, chembl_id: &str) -> Result<Option<Molecule>> {
        self.get("molecule", chembl_id).await
    }

    pub async fn target(&self, chembl_id: &str) -> Result<Option<Target>> {
        self.get("target", chembl_id).await
    }

    pub async fn assay(&self, chembl_id: &str) -> Result<Option<Assay>> {
        self.get("assay", chembl_id).await
    }

    pub async fn document(&self, chembl_id: &str) -> Result<Option<Document>> {
        self.get("document", chembl_id).await
    }

    pub async fn activity(&self, activity_id: u64) -> Result<Option<Activity>> {
        self.get("activity", &activity_id.to_string()).await
    }

    pub async fn molecules_by_name(
        &self,
        name: &str,
        query: &ResourceQuery,
    ) -> Result<Page<Molecule>> {
        self.search("molecule", name, query).await
    }

    pub async fn targets_by_name(&self, name: &str, query: &ResourceQuery) -> Result<Page<Target>> {
        self.search("target", name, query).await
    }

    pub async fn activities_for_molecule(
        &self,
        chembl_id: &str,
        query: &ResourceQuery,
    ) -> Result<Page<Activity>> {
        self.list(
            "activity",
            &query.clone().filter("molecule_chembl_id", chembl_id),
        )
        .await
    }

    pub async fn activities_for_target(
        &self,
        chembl_id: &str,
        query: &ResourceQuery,
    ) -> Result<Page<Activity>> {
        self.list(
            "activity",
            &query.clone().filter("target_chembl_id", chembl_id),
        )
        .await
    }

    pub async fn assays_for_target(
        &self,
        chembl_id: &str,
        query: &ResourceQuery,
    ) -> Result<Page<Assay>> {
        self.list(
            "assay",
            &query.clone().filter("target_chembl_id", chembl_id),
        )
        .await
    }

    pub async fn mechanisms_for_molecule(
        &self,
        chembl_id: &str,
        query: &ResourceQuery,
    ) -> Result<Page<Mechanism>> {
        self.list(
            "mechanism",
            &query.clone().filter("molecule_chembl_id", chembl_id),
        )
        .await
    }

    pub async fn drug_indications_for_molecule(
        &self,
        chembl_id: &str,
        query: &ResourceQuery,
    ) -> Result<Page<DrugIndication>> {
        self.list(
            "drug_indication",
            &query.clone().filter("molecule_chembl_id", chembl_id),
        )
        .await
    }
}

fn validate_component(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(ChemblError::InvalidPath(value.to_string()));
    }
    Ok(())
}

fn explicit_record_key(resource: &str) -> Option<String> {
    Some(match resource {
        "activity" => "activities".into(),
        "cell_line" => "cell_lines".into(),
        "mechanism" => "mechanisms".into(),
        "molecule" => "molecules".into(),
        "source" => "sources".into(),
        "target" => "targets".into(),
        "tissue" => "tissues".into(),
        _ => return None,
    })
}

fn parse_page<T: DeserializeOwned>(resource: &str, value: Value) -> Result<Page<T>> {
    if let Some(error) = value.get("error_message").and_then(Value::as_str) {
        return Err(ChemblError::Api(error.to_string()));
    }

    let records = match explicit_record_key(resource)
        .as_deref()
        .and_then(|key| value.get(key))
        .or_else(|| {
            value
                .as_object()
                .and_then(|object| {
                    object
                        .iter()
                        .find(|(key, item)| *key != "page_meta" && item.is_array())
                })
                .map(|(_, item)| item)
        }) {
        Some(records) => serde_json::from_value(records.clone())?,
        None => return Err(ChemblError::UnexpectedResponse(value)),
    };

    let page_meta = value
        .get("page_meta")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()?;

    Ok(Page {
        records,
        page_meta: page_meta.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_path_components_are_rejected() {
        assert!(validate_component("molecule").is_ok());
        assert!(validate_component("drug_indication").is_ok());
        assert!(validate_component("CHEMBL25").is_ok());
        assert!(validate_component("").is_err());
        assert!(validate_component("../status").is_err());
        assert!(validate_component("CHEMBL/25").is_err());
    }

    #[test]
    fn parses_known_resource_array() {
        let value = serde_json::json!({
            "activities": [{"activity_id": 1, "molecule_chembl_id": "CHEMBL25", "assay_chembl_id": "CHEMBL1", "target_chembl_id": "CHEMBL2"}],
            "page_meta": {"limit": 1, "offset": 0, "total_count": 10, "next": "/next"}
        });
        let page: Page<Activity> = parse_page("activity", value).unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.page_meta.total_count, 10);
    }

    #[test]
    fn falls_back_to_first_unnamed_resource_array() {
        let value = serde_json::json!({
            "new_resources": [],
            "page_meta": {"limit": 100, "offset": 0, "total_count": 0}
        });
        let page: Page<Activity> = parse_page("new_resource", value).unwrap();
        assert!(page.records.is_empty());
    }

    #[test]
    fn api_error_message_becomes_api_error() {
        let value = serde_json::json!({"error_message": "No search query provided"});
        let error = parse_page::<Activity>("molecule", value).unwrap_err();
        assert!(
            matches!(error, ChemblError::Api(message) if message == "No search query provided")
        );
    }
}
