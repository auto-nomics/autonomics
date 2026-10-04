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
use crate::{ChEMBLClient, format::format_drug_indications};

#[tool(
    name = "chembl_indications",
    description = "Pipeline/dataframe use: prefer the DAG node `source_chembl_indications` (typed table) — this tool stays for interactive lookup. Summarize drug indications recorded in ChEMBL for a molecule, including EFO, \
                  MeSH heading, and maximum development phase for the indication."
)]
#[deprecated(note = "prefer the DAG node source_chembl_indications for pipeline use")]
pub struct IndicationsInput {
    #[desc = "ChEMBL molecule ID, e.g. 'CHEMBL25'."]
    pub chembl_id: String,

    #[desc = "Number of records (default 20, max 100)."]
    pub limit: Option<u32>,

    #[desc = "Zero-based record offset for pagination."]
    pub offset: Option<u32>,
}

pub struct IndicationsTool {
    pub(crate) client: Arc<ChEMBLClient>,
}

#[async_trait]
impl ToolFunction for IndicationsTool {
    type Input = IndicationsInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let page = self
            .client
            .drug_indications_for_molecule(
                &input.chembl_id,
                &query(input.limit, input.offset).only([
                    "drugind_id",
                    "molecule_chembl_id",
                    "efo_id",
                    "efo_term",
                    "mesh_id",
                    "mesh_heading",
                    "max_phase_for_ind",
                ]),
            )
            .await
            .map_err(json_err)?;
        Ok(AgentToolResult::success(format_drug_indications(&page)))
    }
}
