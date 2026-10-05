// Tool Input structs marked `#[deprecated]` so the derived tool
// schema advertises `"deprecated": true` (survey T3 dual-track
// guidance); the module-local allow keeps the macro-generated
// impls in this file warning-free.
#![allow(deprecated)]

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::helpers::{json_err, query};
use crate::{ChEMBLClient, format::format_activities};

#[tool(
    name = "chembl_activities",
    description = "Pipeline/dataframe use: prefer the DAG node `source_chembl_activities` (typed table) — this tool stays for interactive lookup. Preview standardized bioactivity records for a ChEMBL molecule or target. \
                  Provide exactly one of molecule_chembl_id or target_chembl_id. Results include \
                  assay, standard activity type/value/units, relation, and pChEMBL."
)]
#[deprecated(note = "prefer the DAG node source_chembl_activities for pipeline use")]
pub struct ActivitiesInput {
    #[desc = "Optional molecule ID filter, e.g. 'CHEMBL25'."]
    pub molecule_chembl_id: Option<String>,

    #[desc = "Optional target ID filter, e.g. 'CHEMBL2094253'."]
    pub target_chembl_id: Option<String>,

    #[desc = "Number of records (default 20, max 100)."]
    pub limit: Option<u32>,

    #[desc = "Zero-based record offset for pagination."]
    pub offset: Option<u32>,
}

pub struct ActivitiesTool {
    pub(crate) client: Arc<ChEMBLClient>,
}

#[async_trait]
impl ToolFunction for ActivitiesTool {
    type Input = ActivitiesInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let page_query = query(input.limit, input.offset).only([
            "activity_id",
            "assay_chembl_id",
            "assay_description",
            "assay_type",
            "document_chembl_id",
            "molecule_chembl_id",
            "molecule_pref_name",
            "parent_molecule_chembl_id",
            "pchembl_value",
            "relation",
            "standard_flag",
            "standard_relation",
            "standard_type",
            "standard_units",
            "standard_value",
            "standard_upper_value",
            "target_chembl_id",
            "target_organism",
            "target_pref_name",
            "target_tax_id",
        ]);
        let page = match (input.molecule_chembl_id, input.target_chembl_id) {
            (Some(molecule), None) => self
                .client
                .activities_for_molecule(&molecule, &page_query)
                .await
                .map_err(json_err)?,
            (None, Some(target)) => self
                .client
                .activities_for_target(&target, &page_query)
                .await
                .map_err(json_err)?,
            (Some(_), Some(_)) => {
                return Err(ToolError::ValidationFailed {
                    message: "provide molecule_chembl_id or target_chembl_id, not both".into(),
                });
            }
            (None, None) => {
                return Err(ToolError::ValidationFailed {
                    message: "chembl_activities requires molecule_chembl_id or target_chembl_id"
                        .into(),
                });
            }
        };
        Ok(AgentToolResult::success(format_activities(&page)))
    }
}
