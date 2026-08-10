use iceberg_catalog_rest::{REST_CATALOG_PROP_URI, REST_CATALOG_PROP_WAREHOUSE};
use std::collections::HashMap;
use std::env;

pub struct IcebergConfig {
    pub catalog_uri: String,
    pub warehouse: String,
    pub s3_endpoint: String,
    pub s3_region: String,
    pub s3_access_key_id: String,
    pub s3_secret_access_key: String,
}

impl IcebergConfig {
    pub fn to_properties(&self) -> HashMap<String, String> {
        HashMap::from([
            (REST_CATALOG_PROP_URI.to_string(), self.catalog_uri.clone()),
            (
                REST_CATALOG_PROP_WAREHOUSE.to_string(),
                self.warehouse.clone(),
            ),
            ("s3.endpoint".to_string(), self.s3_endpoint.clone()),
            ("s3.region".to_string(), self.s3_region.clone()),
            ("s3.path-style-access".to_string(), "true".to_string()),
            (
                "s3.access-key-id".to_string(),
                self.s3_access_key_id.clone(),
            ),
            (
                "s3.secret-access-key".to_string(),
                self.s3_secret_access_key.clone(),
            ),
        ])
    }
}

impl Default for IcebergConfig {
    fn default() -> Self {
        let catalog_uri = env::var("ICEBERG_REST_URI").expect("ICEBERG_REST_URI not set");
        Self {
            catalog_uri,
            // Read S3 config from env to match Python (infra/datalake/src/datalake/catalog.py).
            // Previously these were hardcoded, causing Rust/Python divergence in production.
            warehouse: env::var("ICEBERG_S3_BUCKET")
                .unwrap_or_else(|_| "datalake".to_string()),
            s3_endpoint: env::var("ICEBERG_S3_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:3900".to_string()),
            s3_region: env::var("ICEBERG_S3_REGION")
                .unwrap_or_else(|_| "garage".to_string()),
            s3_access_key_id: env::var("ICEBERG_S3_ACCESS_KEY_ID")
                .expect("ICEBERG_S3_ACCESS_KEY_ID not set"),
            s3_secret_access_key: env::var("ICEBERG_S3_SECRET_ACCESS_KEY")
                .expect("ICEBERG_S3_SECRET_ACCESS_KEY not set"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config() {
        let val = IcebergConfig::default();

        println!("{:#?}", val.to_properties());
    }
}
