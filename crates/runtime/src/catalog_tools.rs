//! Agent-facing search and inspection tools for the object-storage catalog.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_catalog::{CatalogSearchQuery, CatalogServiceTrait, S3CatalogService};
use serde_json::json;

#[tool(
    name = "catalog_search",
    description = "Search discoverable data-catalog datasets by free text, kind, and tags. Results include stable and immutable VFS paths, descriptions, metadata, file counts, and digests."
)]
pub struct CatalogSearchInput {
    #[desc = "Free-text terms. All terms must match id, version, kind, metadata, payload, tags, or digest."]
    pub query: Option<String>,
    #[desc = "Exact dataset kind, for example ldsc_ref_ld_chr."]
    pub kind: Option<String>,
    #[desc = "Required tags; a result must contain every tag."]
    pub tags: Option<Vec<String>>,
    #[desc = "Maximum results. Default 50, maximum 500."]
    pub limit: Option<usize>,
    #[desc = "Reload index.json and current manifests before searching. Default false."]
    pub refresh: Option<bool>,
}

pub struct CatalogSearchTool {
    service: Arc<dyn CatalogServiceTrait>,
}

#[tool(
    name = "catalog_describe",
    description = "Describe a catalog dataset from its validated manifest, including metadata, payload documentation, files, checksums, stable VFS path, and immutable digest path."
)]
pub struct CatalogDescribeInput {
    #[desc = "Catalog dataset id, for example ldsc.ref_ld.1000g_eur.basic."]
    pub id: String,
    #[desc = "Optional exact version. Omit for the current version."]
    pub version: Option<String>,
    #[desc = "Optional immutable digest. Takes precedence when combined with version."]
    pub digest: Option<String>,
    #[desc = "Reload the catalog before resolving the dataset. Default false."]
    pub refresh: Option<bool>,
}

pub struct CatalogDescribeTool {
    service: Arc<dyn CatalogServiceTrait>,
}

#[tool(
    name = "catalog_list_files",
    description = "List a catalog dataset's payload files with paths, byte sizes, and SHA-256 digests."
)]
pub struct CatalogListFilesInput {
    #[desc = "Catalog dataset id."]
    pub id: String,
    #[desc = "Optional exact version. Omit for the current version."]
    pub version: Option<String>,
    #[desc = "Optional immutable digest."]
    pub digest: Option<String>,
    #[desc = "Reload the catalog first. Default false."]
    pub refresh: Option<bool>,
}

pub struct CatalogListFilesTool {
    service: Arc<dyn CatalogServiceTrait>,
}

#[tool(
    name = "catalog_list_versions",
    description = "List current and historical catalog versions for a dataset id, with digests and VFS paths."
)]
pub struct CatalogListVersionsInput {
    #[desc = "Catalog dataset id."]
    pub id: String,
    #[desc = "Reload the catalog first. Default false."]
    pub refresh: Option<bool>,
}

pub struct CatalogListVersionsTool {
    service: Arc<dyn CatalogServiceTrait>,
}

#[tool(
    name = "catalog_refresh",
    description = "Reload catalog index.json and current dataset manifests for discovery tools. This does not rebuild startup VFS mounts or the DataBundle registry; restart the runtime before binding newly published entries to a DAG node."
)]
pub struct CatalogRefreshInput {}

pub struct CatalogRefreshTool {
    service: Arc<dyn CatalogServiceTrait>,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct CatalogToolError(String);

fn execution_failed(error: String) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(CatalogToolError(error)),
    }
}

async fn refresh_if_requested(
    service: &dyn CatalogServiceTrait,
    refresh: bool,
) -> Result<(), ToolError> {
    if refresh {
        service
            .refresh()
            .await
            .map(|_| ())
            .map_err(execution_failed)?;
    }
    Ok(())
}

#[async_trait]
impl ToolFunction for CatalogSearchTool {
    type Input = CatalogSearchInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        refresh_if_requested(&*self.service, input.refresh.unwrap_or(false)).await?;
        let records = self
            .service
            .search(CatalogSearchQuery {
                query: input.query,
                kind: input.kind,
                tags: input.tags.unwrap_or_default(),
                limit: input.limit,
            })
            .await
            .map_err(execution_failed)?;
        Ok(ToolResult::success_json(json!({ "datasets": records })))
    }
}

#[async_trait]
impl ToolFunction for CatalogDescribeTool {
    type Input = CatalogDescribeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        refresh_if_requested(&*self.service, input.refresh.unwrap_or(false)).await?;
        let dataset = self
            .service
            .describe(&input.id, input.version.as_deref(), input.digest.as_deref())
            .await
            .map_err(execution_failed)?;
        Ok(ToolResult::success_json(json!({ "dataset": dataset })))
    }
}

#[async_trait]
impl ToolFunction for CatalogListFilesTool {
    type Input = CatalogListFilesInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        refresh_if_requested(&*self.service, input.refresh.unwrap_or(false)).await?;
        let files = self
            .service
            .list_files(&input.id, input.version.as_deref(), input.digest.as_deref())
            .await
            .map_err(execution_failed)?;
        Ok(ToolResult::success_json(json!({ "files": files })))
    }
}

#[async_trait]
impl ToolFunction for CatalogListVersionsTool {
    type Input = CatalogListVersionsInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        refresh_if_requested(&*self.service, input.refresh.unwrap_or(false)).await?;
        let versions = self
            .service
            .list_versions(&input.id)
            .await
            .map_err(execution_failed)?;
        Ok(ToolResult::success_json(json!({ "versions": versions })))
    }
}

#[async_trait]
impl ToolFunction for CatalogRefreshTool {
    type Input = CatalogRefreshInput;

    async fn run(&self, _input: Self::Input) -> Result<ToolResult, ToolError> {
        let snapshot = self.service.refresh().await.map_err(execution_failed)?;
        Ok(ToolResult::success_json(json!({
            "generation": snapshot.index.generation,
            "current_datasets": snapshot.records.len(),
            "vfs_mounts_refreshed": false,
            "data_bundle_registry_refreshed": false
        })))
    }
}

pub fn catalog_registrations(service: Arc<dyn CatalogServiceTrait>) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(CatalogSearchTool {
            service: Arc::clone(&service),
        }),
        ToolRegistration::from(CatalogDescribeTool {
            service: Arc::clone(&service),
        }),
        ToolRegistration::from(CatalogListFilesTool {
            service: Arc::clone(&service),
        }),
        ToolRegistration::from(CatalogListVersionsTool {
            service: Arc::clone(&service),
        }),
        ToolRegistration::from(CatalogRefreshTool { service }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use data_catalog::{
        build_package, package::BuildOptions, publish_package, storage::operator_for_backend,
    };
    use vfs::{BackendConfig, BackendDefinition, MountDefinition, VfsManifest};

    async fn test_service() -> S3CatalogService {
        let workspace = tempfile::tempdir().unwrap();
        let warehouse = tempfile::tempdir().unwrap();
        let input = workspace.path().join("input");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("data.txt"), b"catalog-tools").unwrap();
        let package = build_package(
            &input,
            workspace.path().join("package"),
            BuildOptions {
                id: Some("catalog_tools.test".into()),
                version: Some("v1".into()),
                kind: Some("table".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let manifest = VfsManifest {
            backend: vec![BackendDefinition {
                id: "warehouse".into(),
                config: BackendConfig::local(warehouse.path().to_string_lossy().into_owned()),
            }],
            mount: vec![MountDefinition {
                path: "/".into(),
                backend: "warehouse".into(),
                source: "/".into(),
                read_only: true,
            }],
        };
        let config = data_catalog::CatalogConfig {
            backend: "warehouse".into(),
            source: "/catalog".into(),
            ..Default::default()
        };
        let operator = operator_for_backend(&manifest, &config.backend).unwrap();
        publish_package(package.path, &config, &operator)
            .await
            .unwrap();
        S3CatalogService::new(&manifest, &config).await.unwrap()
    }

    #[tokio::test]
    async fn registers_five_catalog_tools() {
        let registrations = catalog_registrations(Arc::new(test_service().await));
        let names: Vec<&str> = registrations
            .iter()
            .map(|registration| registration.definition.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "catalog_search",
                "catalog_describe",
                "catalog_list_files",
                "catalog_list_versions",
                "catalog_refresh"
            ]
        );
    }
}
