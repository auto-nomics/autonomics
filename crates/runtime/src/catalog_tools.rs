//! Agent-facing tools for the remote catalog and the local package cache.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_catalog::{LocalCatalog, RemoteCatalog};
use serde_json::json;

/// Remote catalog access plus the runtime's local package cache.
pub struct CatalogState {
    pub local: Arc<LocalCatalog>,
    pub remote: Arc<RemoteCatalog>,
}

#[tool(
    name = "catalog_search",
    description = "Search current entries in the remote catalog by free text and kind. Results include id, version, kind, digest, and generated VFS paths."
)]
pub struct CatalogSearchInput {
    #[desc = "Free-text terms. All terms must match id, version, kind, or digest."]
    pub query: Option<String>,
    #[desc = "Exact dataset kind, for example ldsc_ref_ld_chr."]
    pub kind: Option<String>,
    #[desc = "Maximum results. Default 50, maximum 500."]
    pub limit: Option<usize>,
}

pub struct CatalogSearchTool {
    state: Arc<CatalogState>,
}

#[tool(
    name = "catalog_install",
    description = "Download and verify one package from the remote catalog into the local cache. Restart the runtime before binding newly installed entries into a DAG."
)]
pub struct CatalogInstallInput {
    #[desc = "Catalog dataset id."]
    pub id: String,
    #[desc = "Optional exact version. Omit for the current version."]
    pub version: Option<String>,
    #[desc = "Optional immutable digest."]
    pub digest: Option<String>,
}

pub struct CatalogInstallTool {
    state: Arc<CatalogState>,
}

#[tool(
    name = "catalog_update",
    description = "Install remote current versions missing from the local cache. Omit id to update every installed dataset family. Restart the runtime to rebuild VFS mounts and bundle registry."
)]
pub struct CatalogUpdateInput {
    #[desc = "Optional dataset id. Omit to update all current entries."]
    pub id: Option<String>,
}

pub struct CatalogUpdateTool {
    state: Arc<CatalogState>,
}

#[tool(
    name = "catalog_describe",
    description = "Describe an installed catalog dataset from its validated local manifest, including metadata, payload documentation, files, and checksums."
)]
pub struct CatalogDescribeInput {
    #[desc = "Catalog dataset id."]
    pub id: String,
    #[desc = "Optional exact version. Omit for the current version."]
    pub version: Option<String>,
    #[desc = "Optional immutable digest."]
    pub digest: Option<String>,
}

pub struct CatalogDescribeTool {
    state: Arc<CatalogState>,
}

#[tool(
    name = "catalog_list_files",
    description = "List an installed catalog dataset's payload files with paths, byte sizes, and SHA-256 digests."
)]
pub struct CatalogListFilesInput {
    #[desc = "Catalog dataset id."]
    pub id: String,
    #[desc = "Optional exact version. Omit for the current version."]
    pub version: Option<String>,
    #[desc = "Optional immutable digest."]
    pub digest: Option<String>,
}

pub struct CatalogListFilesTool {
    state: Arc<CatalogState>,
}

#[tool(
    name = "catalog_list_versions",
    description = "List locally installed current and historical catalog versions for a dataset id, newest first."
)]
pub struct CatalogListVersionsInput {
    #[desc = "Catalog dataset id."]
    pub id: String,
}

pub struct CatalogListVersionsTool {
    state: Arc<CatalogState>,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct CatalogToolError(String);

fn execution_failed(error: String) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(CatalogToolError(error)),
    }
}

fn select_local_entry(
    state: &CatalogState,
    id: &str,
    version: Option<&str>,
    digest: Option<&str>,
) -> Result<data_catalog::CatalogEntry, ToolError> {
    state
        .local
        .index()
        .and_then(|index| index.select(id, version, digest))
        .map_err(|error| execution_failed(error.to_string()))
}

#[async_trait]
impl ToolFunction for CatalogSearchTool {
    type Input = CatalogSearchInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let index = self
            .state
            .remote
            .index()
            .await
            .map_err(|error| execution_failed(error.to_string()))?;
        let entries = index.search(
            input.query.as_deref().unwrap_or(""),
            input.kind.as_deref(),
            input.limit.unwrap_or(50).min(500),
        );
        Ok(ToolResult::success_json(json!({ "datasets": entries })))
    }
}

#[async_trait]
impl ToolFunction for CatalogInstallTool {
    type Input = CatalogInstallInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entry = self
            .state
            .local
            .install(
                &self.state.remote,
                &input.id,
                input.version.as_deref(),
                input.digest.as_deref(),
            )
            .await
            .map_err(|error| execution_failed(error.to_string()))?;
        Ok(ToolResult::success_json(json!({
            "entry": entry,
            "restart_required": true
        })))
    }
}

#[async_trait]
impl ToolFunction for CatalogUpdateTool {
    type Input = CatalogUpdateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let updated = self
            .state
            .local
            .update(&self.state.remote, input.id.as_deref())
            .await
            .map_err(|error| execution_failed(error.to_string()))?;
        let restart_required = !updated.is_empty();
        Ok(ToolResult::success_json(json!({
            "updated": updated,
            "restart_required": restart_required
        })))
    }
}

#[async_trait]
impl ToolFunction for CatalogDescribeTool {
    type Input = CatalogDescribeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entry = select_local_entry(
            &self.state,
            &input.id,
            input.version.as_deref(),
            input.digest.as_deref(),
        )?;
        let manifest = self
            .state
            .local
            .manifest(&entry)
            .map_err(|error| execution_failed(error.to_string()))?;
        Ok(ToolResult::success_json(json!({
            "entry": entry,
            "manifest": manifest
        })))
    }
}

#[async_trait]
impl ToolFunction for CatalogListFilesTool {
    type Input = CatalogListFilesInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entry = select_local_entry(
            &self.state,
            &input.id,
            input.version.as_deref(),
            input.digest.as_deref(),
        )?;
        let manifest = self
            .state
            .local
            .manifest(&entry)
            .map_err(|error| execution_failed(error.to_string()))?;
        Ok(ToolResult::success_json(json!({ "files": manifest.files })))
    }
}

#[async_trait]
impl ToolFunction for CatalogListVersionsTool {
    type Input = CatalogListVersionsInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let index = self
            .state
            .local
            .index()
            .map_err(|error| execution_failed(error.to_string()))?;
        let mut entries = index
            .entries
            .iter()
            .filter(|entry| entry.repo.as_str() == input.id)
            .cloned()
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return Err(execution_failed(format!(
                "catalog dataset `{}` is not installed",
                input.id
            )));
        }
        entries.sort_by(|left, right| {
            right
                .created_unix_seconds
                .cmp(&left.created_unix_seconds)
                .then_with(|| left.version.cmp(&right.version))
        });
        Ok(ToolResult::success_json(json!({ "versions": entries })))
    }
}

pub fn catalog_registrations(state: Arc<CatalogState>) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(CatalogSearchTool {
            state: Arc::clone(&state),
        }),
        ToolRegistration::from(CatalogInstallTool {
            state: Arc::clone(&state),
        }),
        ToolRegistration::from(CatalogUpdateTool {
            state: Arc::clone(&state),
        }),
        ToolRegistration::from(CatalogDescribeTool {
            state: Arc::clone(&state),
        }),
        ToolRegistration::from(CatalogListFilesTool {
            state: Arc::clone(&state),
        }),
        ToolRegistration::from(CatalogListVersionsTool { state }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> CatalogState {
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let _ = workspace;
        let local = LocalCatalog::open(cache.path()).unwrap();
        let remote = RemoteCatalog::hf("owner/catalog-index", None, None).unwrap();
        CatalogState {
            local: Arc::new(local),
            remote: Arc::new(remote),
        }
    }

    #[tokio::test]
    async fn registers_six_catalog_tools() {
        let registrations = catalog_registrations(Arc::new(test_state()));
        let names: Vec<&str> = registrations
            .iter()
            .map(|registration| registration.definition.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "catalog_search",
                "catalog_install",
                "catalog_update",
                "catalog_describe",
                "catalog_list_files",
                "catalog_list_versions"
            ]
        );
    }
}
