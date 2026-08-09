use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::CrossrefClient;
use crate::format::{format_funders, format_journals, format_members};

/// Browse Crossref resource registries: members (publishers), journals,
/// funders, or work types.
///
/// This is for exploring the Crossref ecosystem. For finding individual
/// publications, use `crossref_search`.
#[tool(
    name = "crossref_types",
    description = "Browse Crossref resource registries: members (publishers), journals, \
                  funders, or work types. Returns lists with summary information. \
                  \
                  Set `resource` to one of: 'members', 'journals', 'funders', 'types'. \
                  Use `query` to filter by name. \
                  \
                  Example: resource='members', query='Elsevier' returns the Elsevier \
                  member record with DOI counts."
)]
pub struct CrossrefTypesInput {
    #[desc = "Which registry to browse: 'members' (publishers/organisations), \
             'journals', 'funders' (Open Funder Registry), or 'types' (work types). \
             Default: 'members'."]
    pub resource: Option<String>,

    #[desc = "Name filter (e.g. 'Elsevier' for members, 'Nature' for journals)."]
    pub query: Option<String>,

    #[desc = "Maximum number of results (default 25, max 1000)."]
    pub rows: Option<u32>,
}

pub struct CrossrefTypesTool {
    pub client: Arc<CrossrefClient>,
}

#[async_trait]
impl ToolFunction for CrossrefTypesTool {
    type Input = CrossrefTypesInput;

    fn timeout_seconds(&self) -> u64 {
        60
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let resource = input.resource.as_deref().unwrap_or("members");
        let mut list = crate::client::ListQuery::new();
        if let Some(q) = input.query {
            list = list.with_query(q);
        }
        if let Some(r) = input.rows {
            list = list.with_rows(r.min(1000));
        }

        let markdown = match resource {
            "members" => {
                let resp = self.client.members(&list).await.map_err(super::json_err)?;
                format_members(&resp)
            }
            "journals" => {
                let resp = self.client.journals(&list).await.map_err(super::json_err)?;
                format_journals(&resp)
            }
            "funders" => {
                let resp = self.client.funders(&list).await.map_err(super::json_err)?;
                format_funders(&resp)
            }
            "types" => {
                let resp = self.client.types().await.map_err(super::json_err)?;
                let msg = &resp.message;
                let mut out = String::new();
                for (i, t) in msg.items.iter().enumerate() {
                    out.push_str(&format!("{}. **{}** — {}\n", i + 1, t.id, t.label));
                }
                out
            }
            other => {
                return Err(ToolError::ValidationFailed {
                    message: format!(
                        "unknown resource '{other}' — use 'members', 'journals', \
                         'funders', or 'types'"
                    ),
                });
            }
        };

        Ok(AgentToolResult::success(markdown))
    }
}
