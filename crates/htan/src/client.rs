use crate::error::{HtanError, Result};
use crate::types::{
    Atlas, DatasetFile, DatasetSummary, FileFilter, file_where_clause, limit_clause,
};

pub const DEFAULT_HOST: &str = "https://dl96orhu96.us-east-1.aws.clickhouse.cloud:8443";
pub const DEFAULT_DATABASE: &str = "htan_2026_932";
pub const DEFAULT_USER: &str = "htanwebuser";
// HTAN Portal documents this as an intentionally public, read-only credential.
pub const DEFAULT_PASSWORD: &str = "HT4N_P0RT4L_isDaBest";

#[derive(Debug, Clone)]
pub struct HtanClient {
    http: reqwest::Client,
    host: String,
    database: String,
    username: String,
    password: String,
}

impl Default for HtanClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HtanClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("autonomics-htan-sdk/0.1 (+https://humantumoratlas.org)")
                .build()
                .expect("failed to build HTAN HTTP client"),
            host: std::env::var("HTAN_CLICKHOUSE_HOST")
                .unwrap_or_else(|_| DEFAULT_HOST.into())
                .trim_end_matches('/')
                .to_owned(),
            database: std::env::var("HTAN_CLICKHOUSE_DATABASE")
                .unwrap_or_else(|_| DEFAULT_DATABASE.into()),
            username: std::env::var("HTAN_CLICKHOUSE_USER").unwrap_or_else(|_| DEFAULT_USER.into()),
            password: std::env::var("HTAN_CLICKHOUSE_PASSWORD")
                .unwrap_or_else(|_| DEFAULT_PASSWORD.into()),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    pub async fn atlases(&self) -> Result<Vec<Atlas>> {
        let sql = r#"
            SELECT
                a.htan_id AS htan_id,
                a.htan_name AS htan_name,
                JSONExtractString(a.AtlasMeta, 'title', 'rendered') AS title,
                JSONExtractString(a.AtlasMeta, 'lead_institutions') AS lead_institutions,
                JSONExtractString(a.AtlasMeta, 'short_description') AS short_description,
                toUInt64OrZero(a.num_biospecimens) AS num_biospecimens,
                toUInt64OrZero(a.num_cases) AS num_cases,
                count(DISTINCT f.synapseId, f.DataFileID) AS file_rows,
                countDistinct(f.DataFileID) AS data_files,
                a.AtlasMeta AS atlas_meta
            FROM atlases AS a
            LEFT JOIN (
                SELECT atlasid, synapseId, DataFileID
                FROM files
            ) AS f ON f.atlasid = a.htan_id
            GROUP BY a.htan_id, a.htan_name, a.AtlasMeta,
                     a.num_biospecimens, a.num_cases
            ORDER BY a.htan_id
        "#;
        self.query(sql).await
    }

    pub async fn dataset_summaries(&self, filter: &FileFilter) -> Result<Vec<DatasetSummary>> {
        let sql = format!(
            r#"
            SELECT
                atlasid,
                atlas_name,
                level,
                assayName AS assay_name,
                Component AS component,
                organType AS organ_type,
                releaseVersion AS release_version,
                downloadSource AS download_source,
                count() AS file_rows,
                countDistinct(DataFileID) AS data_files
            FROM files{}
            GROUP BY atlasid, atlas_name, level, assayName, Component,
                     organType, releaseVersion, downloadSource
            ORDER BY atlasid, level, assayName
            {}
            "#,
            file_where_clause(filter),
            limit_clause(filter.max_results)
        );
        self.query(&sql).await
    }

    pub async fn files(&self, filter: &FileFilter) -> Result<Vec<DatasetFile>> {
        let sql = format!(
            r#"
            SELECT
                synapseId AS synapse_id,
                atlasid,
                atlas_name,
                level,
                assayName AS assay_name,
                Filename AS filename,
                FileFormat AS file_format,
                DataFileID AS data_file_id,
                Component AS component,
                organType AS organ_type,
                releaseVersion AS release_version,
                downloadSource AS download_source,
                biospecimenIds AS biospecimen_ids,
                publicationIds AS publication_ids
            FROM files{}
            ORDER BY atlasid, filename, data_file_id
            {}
            "#,
            file_where_clause(filter),
            limit_clause(filter.max_results)
        );
        self.query(&sql).await
    }

    async fn query<T>(&self, sql: &str) -> Result<Vec<T>>
    where
        T: serde::de::DeserializeOwned,
    {
        let url = format!("{}/?database={}", self.host, self.database);
        let response = self
            .http
            .post(url)
            .basic_auth(&self.username, Some(&self.password))
            .header("X-ClickHouse-Format", "JSONEachRow")
            .body(format!("{sql}\nFORMAT JSONEachRow"))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(HtanError::Status {
                status: status.as_u16(),
                body,
            });
        }
        let mut rows = Vec::new();
        for line in body.lines().filter(|line| !line.trim().is_empty()) {
            rows.push(serde_json::from_str(line)?);
        }
        Ok(rows)
    }
}
