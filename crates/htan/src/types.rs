use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Atlas {
    pub htan_id: String,
    pub htan_name: String,
    pub title: String,
    pub lead_institutions: String,
    pub short_description: String,
    #[serde(default, deserialize_with = "optional_u64")]
    pub num_biospecimens: Option<u64>,
    #[serde(default, deserialize_with = "optional_u64")]
    pub num_cases: Option<u64>,
    #[serde(default, deserialize_with = "optional_u64")]
    pub file_rows: Option<u64>,
    #[serde(default, deserialize_with = "optional_u64")]
    pub data_files: Option<u64>,
    pub atlas_meta: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DatasetSummary {
    pub atlasid: String,
    pub atlas_name: String,
    pub level: String,
    pub assay_name: String,
    pub component: String,
    pub organ_type: Vec<String>,
    pub release_version: String,
    pub download_source: String,
    #[serde(default, deserialize_with = "optional_u64")]
    pub file_rows: Option<u64>,
    #[serde(default, deserialize_with = "optional_u64")]
    pub data_files: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DatasetFile {
    pub synapse_id: String,
    pub atlasid: String,
    pub atlas_name: String,
    pub level: String,
    pub assay_name: String,
    pub filename: String,
    pub file_format: String,
    pub data_file_id: String,
    pub component: String,
    pub organ_type: Vec<String>,
    pub release_version: String,
    pub download_source: String,
    pub biospecimen_ids: Vec<String>,
    pub publication_ids: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FileFilter {
    pub query: Option<String>,
    pub atlas: Option<String>,
    pub level: Option<String>,
    pub assay: Option<String>,
    pub organ: Option<String>,
    pub component: Option<String>,
    pub file_format: Option<String>,
    pub release: Option<String>,
    pub max_results: Option<u64>,
}

pub(crate) fn file_where_clause(filter: &FileFilter) -> String {
    let mut clauses = Vec::new();
    if let Some(value) = clean(&filter.atlas) {
        clauses.push(format!("lower(atlasid) = lower({})", quote(value)));
    }
    if let Some(value) = clean(&filter.level) {
        clauses.push(format!("lower(level) = lower({})", quote(value)));
    }
    if let Some(value) = clean(&filter.assay) {
        clauses.push(format!("lower(assayName) = lower({})", quote(value)));
    }
    if let Some(value) = clean(&filter.organ) {
        clauses.push(format!("has(organType, {})", quote(value)));
    }
    if let Some(value) = clean(&filter.component) {
        clauses.push(format!("lower(Component) = lower({})", quote(value)));
    }
    if let Some(value) = clean(&filter.file_format) {
        clauses.push(format!("lower(FileFormat) = lower({})", quote(value)));
    }
    if let Some(value) = clean(&filter.release) {
        clauses.push(format!("lower(releaseVersion) = lower({})", quote(value)));
    }
    if let Some(value) = clean(&filter.query) {
        clauses.push(format!(
            "positionCaseInsensitive(concat_ws(' ', Filename, atlas_name, assayName, Component, arrayStringConcat(organType, ' ')), {}) > 0",
            quote(value)
        ));
    }
    if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    }
}

pub(crate) fn limit_clause(max_results: Option<u64>) -> String {
    format!(" LIMIT {}", max_results.unwrap_or(100).clamp(1, 100_000))
}

pub(crate) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn optional_u64<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Null => Ok(None),
        Value::Number(number) => number
            .as_u64()
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("expected unsigned integer")),
        Value::String(text) if text.trim().is_empty() => Ok(None),
        Value::String(text) => {
            text.trim().parse::<u64>().map(Some).map_err(|_| {
                serde::de::Error::custom(format!("invalid unsigned integer: '{text}'"))
            })
        }
        other => Err(serde::de::Error::custom(format!(
            "invalid unsigned integer: {other}"
        ))),
    }
}

fn clean(value: &Option<String>) -> Option<&str> {
    let value = value.as_deref()?.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_where_clause_and_escapes_values() {
        let filter = FileFilter {
            atlas: Some("HTA9".into()),
            organ: Some("Breast".into()),
            query: Some("o'brien".into()),
            ..Default::default()
        };
        let sql = file_where_clause(&filter);
        assert!(sql.contains("lower(atlasid) = lower('HTA9')"));
        assert!(sql.contains("has(organType, 'Breast')"));
        assert!(sql.contains("o\\'brien"));
    }
}
