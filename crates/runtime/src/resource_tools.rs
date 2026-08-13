//! Agent-facing read-only tools for the centralized resource catalog.
//!
//! These tools let the AI agent discover what data sources, endpoints,
//! databases, and file paths are available — without hardcoding any
//! physical address. The agent can browse the catalog (`list_resources`)
//! and inspect a specific resource (`describe_resource`) to understand
//! what it is and how to use it.
//!
//! Both tools read from [`ResourceCatalog::global()`], which is set once
//! during `SharedInfra::open`. When the global is not set (e.g. in
//! standalone crate tests), the tools return an informational error.

use agentik_proc::tool;
use agentik_sdk::types::tools::ToolResult;
use agentik_types::tools::ToolResultContent;
use async_trait::async_trait;

use agentik_core::tools::{ToolError, ToolFunction};
use dag_core::resource_catalog::ResourceCatalog;

// ── list_resources ───────────────────────────────────────────────────────

#[tool(
    name = "list_resources",
    description = "List all registered data resources (Iceberg tables, file paths, \
                   API endpoints, databases, docs). Returns each resource's logical \
                   name, kind, and short description. Use this to discover what data \
                   sources and services are available before composing a DAG or \
                   calling an API."
)]
pub struct ListResourcesInput {
    #[desc = "Optional filter: only return resources whose kind matches \
              (e.g. 'iceberg_table', 'endpoint', 'file_path', 'database', 'doc')."]
    pub kind_filter: Option<String>,
}

/// The `list_resources` tool.
pub struct ListResourcesTool;

#[async_trait]
impl ToolFunction for ListResourcesTool {
    type Input = ListResourcesInput;

    async fn run(&self, input: ListResourcesInput) -> Result<ToolResult, ToolError> {
        let catalog = ResourceCatalog::global().ok_or_else(|| ToolError::RegistryError {
            message: "resource catalog is not initialized (running outside the runtime)".into(),
        })?;

        let mut entries = catalog.list();
        if entries.is_empty() {
            return Ok(ToolResult::success(
                "No resources registered. The catalog is empty.",
            ));
        }

        // Optional kind filter.
        if let Some(filter) = &input.kind_filter {
            entries.retain(|e| e.kind.as_str() == filter.as_str());
            if entries.is_empty() {
                return Ok(ToolResult::success(format!(
                    "No resources of kind '{filter}' registered."
                )));
            }
        }

        // Sort by kind then name for readability.
        entries.sort_by(|a, b| {
            a.kind
                .as_str()
                .cmp(b.kind.as_str())
                .then_with(|| a.name.cmp(&b.name))
        });

        let mut lines = Vec::with_capacity(entries.len() + 1);
        lines.push(format!("{} registered resources:\n", entries.len()));
        for e in &entries {
            lines.push(format!(
                "  [{:>13}] {} — {}",
                e.kind.as_str(),
                e.name,
                e.description
            ));
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

// ── describe_resource ────────────────────────────────────────────────────

#[tool(
    name = "describe_resource",
    description = "Show full details for a specific resource by its logical name \
                   (e.g. 'ldscore.1000g_eur', 'endpoint.eutils', 'db.agent'). \
                   Returns the kind, description, address, metadata, and tags. \
                   Use this after list_resources to understand a resource's \
                   physical location or configuration."
)]
pub struct DescribeResourceInput {
    #[desc = "The logical name of the resource (as shown by list_resources)."]
    pub name: String,
}

/// The `describe_resource` tool.
pub struct DescribeResourceTool;

#[async_trait]
impl ToolFunction for DescribeResourceTool {
    type Input = DescribeResourceInput;

    async fn run(&self, input: DescribeResourceInput) -> Result<ToolResult, ToolError> {
        let catalog = ResourceCatalog::global().ok_or_else(|| ToolError::RegistryError {
            message: "resource catalog is not initialized (running outside the runtime)".into(),
        })?;

        let entry = catalog
            .get(&input.name)
            .ok_or_else(|| ToolError::RegistryError {
                message: format!(
                    "unknown resource '{}'. Call list_resources to see available names.",
                    input.name
                ),
            })?;

        let mut lines = Vec::new();
        lines.push(format!("Name:        {}", entry.name));
        lines.push(format!("Kind:        {}", entry.kind.as_str()));
        lines.push(format!("Description: {}", entry.description));
        lines.push(format!("Address:     {:?}", entry.address));

        if !entry.metadata.is_empty() {
            lines.push("Metadata:".into());
            for (k, v) in &entry.metadata {
                lines.push(format!("  {k} = {v}"));
            }
        }

        if !entry.tags.is_empty() {
            lines.push(format!("Tags:        {}", entry.tags.join(", ")));
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

/// Return both resource tool registrations for inclusion in the default
/// tool set.
pub fn registrations() -> Vec<agentik_core::tools::ToolRegistration> {
    vec![ListResourcesTool.into(), DescribeResourceTool.into()]
}
