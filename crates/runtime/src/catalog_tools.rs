//! Agent-facing tools for the package catalog and its local installation cache.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_catalog::{LocalCatalog, RemoteCatalog};
use serde_json::json;

/// Remote package registry plus the runtime's local installation cache.
pub struct CatalogState {
    pub local: Arc<LocalCatalog>,
    pub remote: Arc<RemoteCatalog>,
}

#[tool(
    name = "catalog_search",
    description = "Search current entries installed in the local catalog cache by free text and kind. Results include repository, version, kind, and digest."
)]
pub struct CatalogSearchInput {
    #[desc = "Free-text terms. All terms must match repository, version, kind, or digest."]
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
        let entries = self
            .state
            .local
            .search(
                input.query.as_deref().unwrap_or(""),
                input.kind.as_deref(),
                input.limit.unwrap_or(50).min(500),
            )
            .map_err(|error| execution_failed(error.to_string()))?;
        Ok(ToolResult::success_json(json!({ "datasets": entries })))
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
    use agentik_sdk::types::ToolResultContent;
    use data_catalog::remote::test_utils::MapSource;

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

    fn local_state_with_installed_entry() -> (CatalogState, tempfile::TempDir, std::path::PathBuf) {
        let cache = tempfile::tempdir().unwrap();
        let entry = data_catalog::CatalogEntry {
            repo: data_catalog::HfRepoId::new("owner/installed-panel").unwrap(),
            version: "v1".into(),
            kind: "panel".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            current: true,
            created_unix_seconds: 1,
        };
        let mut index = data_catalog::CatalogIndex::default();
        index.upsert_current(entry.clone());
        std::fs::write(
            cache.path().join("index.json"),
            serde_json::to_vec(&index).unwrap(),
        )
        .unwrap();

        let entry_dir = cache.path().join(entry.cache_dir_name());
        std::fs::create_dir_all(&entry_dir).unwrap();
        std::fs::write(entry_dir.join("manifest.json"), "").unwrap();
        let marker = entry_dir.join(data_catalog::local::PANEL_CACHE_COMPLETE_MARKER);
        std::fs::write(&marker, format!("digest={}\n", entry.digest)).unwrap();

        let remote = RemoteCatalog::from_source(
            data_catalog::CatalogConfig {
                repository: Some("owner/catalog-index".into()),
                ..Default::default()
            },
            Box::new(MapSource::default()),
        );
        (
            CatalogState {
                local: Arc::new(LocalCatalog::open(cache.path()).unwrap()),
                remote: Arc::new(remote),
            },
            cache,
            marker,
        )
    }

    #[tokio::test]
    async fn registers_five_catalog_tools() {
        let registrations = catalog_registrations(Arc::new(test_state()));
        let names: Vec<&str> = registrations
            .iter()
            .map(|registration| registration.definition.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "catalog_search",
                "catalog_update",
                "catalog_describe",
                "catalog_list_files",
                "catalog_list_versions"
            ]
        );
    }

    #[tokio::test]
    async fn catalog_search_uses_installed_local_entries() {
        let (state, _cache, marker) = local_state_with_installed_entry();
        let state = Arc::new(state);
        let result = CatalogSearchTool {
            state: Arc::clone(&state),
        }
        .run(CatalogSearchInput {
            query: Some("installed-panel".into()),
            kind: Some("panel".into()),
            limit: Some(10),
        })
        .await
        .unwrap();

        let ToolResultContent::Json(value) = result.content else {
            panic!("catalog_search must return JSON");
        };
        let datasets = value["datasets"].as_array().unwrap();
        assert_eq!(datasets.len(), 1);
        assert_eq!(datasets[0]["repo"].as_str(), Some("owner/installed-panel"));

        std::fs::remove_file(marker).unwrap();
        let result = CatalogSearchTool { state }
            .run(CatalogSearchInput {
                query: Some("installed-panel".into()),
                kind: Some("panel".into()),
                limit: Some(10),
            })
            .await
            .unwrap();
        let ToolResultContent::Json(value) = result.content else {
            panic!("catalog_search must return JSON");
        };
        assert!(value["datasets"].as_array().unwrap().is_empty());
    }
}
