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
use crate::{ChEMBLClient, format::format_mechanisms};

#[tool(
    name = "chembl_mechanisms",
    description = "Pipeline/dataframe use: prefer the DAG node `source_chembl_mechanisms` (typed table) — this tool stays for interactive lookup. Summarize mechanisms of action recorded in ChEMBL for a molecule, including \
                  target, action type, development phase, and direct-interaction flag."
)]
#[deprecated(note = "prefer the DAG node source_chembl_mechanisms for pipeline use")]
pub struct MechanismsInput {
    #[desc = "ChEMBL molecule ID, e.g. 'CHEMBL25'."]
    pub chembl_id: String,

    #[desc = "Number of records (default 20, max 100)."]
    pub limit: Option<u32>,

    #[desc = "Zero-based record offset for pagination."]
    pub offset: Option<u32>,
}

pub struct MechanismsTool {
    pub(crate) client: Arc<ChEMBLClient>,
}

#[async_trait]
impl ToolFunction for MechanismsTool {
    type Input = MechanismsInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let page = self
            .client
            .mechanisms_for_molecule(
                &input.chembl_id,
                &query(input.limit, input.offset).only([
                    "mec_id",
                    "molecule_chembl_id",
                    "target_chembl_id",
                    "mechanism_of_action",
                    "action_type",
                    "direct_interaction",
                    "molecular_mechanism",
                    "disease_efficacy",
                    "max_phase",
                ]),
            )
            .await
            .map_err(json_err)?;
        Ok(AgentToolResult::success(format_mechanisms(&page)))
    }
}
