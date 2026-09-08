use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::UniProtClient;
use crate::format::format_entries;
use crate::query::Query;
use crate::types::SearchRequest;

#[tool(
    name = "uniprot_search",
    description = "Search UniProtKB (~600M protein entries: Swiss-Prot reviewed + TrEMBL \
                  unreviewed) and return matching entries with names, organism, function \
                  summaries and sequence lengths. \
                  \
                  TWO query modes (provide exactly one): \
                  \
                  1. RECOMMENDED — populate any subset of the typed filter fields \
                     (gene, protein_name, organism, organism_id, accessions, keyword, \
                     reviewed); they are AND-ed together. \
                     \
                     Example: { \"gene\": [\"INS\"], \"organism_id\": 9606, \"reviewed\": true } \
                  \
                  2. EXPERT — `query`: a raw UniProt query expression. \
                     \
                     Field prefixes: accession:, gene:, protein_name:, organism_name:, \
                     organism_id:, keyword:, reviewed:, length:. \
                     Boolean operators (uppercase): AND, OR, NOT. \
                     Phrase-quote with double quotes: organism_name:\"Homo sapiens\". \
                     \
                     Example: 'gene:BRCA1 AND organism_id:9606 AND reviewed:true'"
)]
pub struct UniprotSearchInput {
    #[desc = "Gene name(s), OR-ed within the clause. Example: [\"BRCA1\", \"BRCA2\"]."]
    pub gene: Option<Vec<String>>,

    #[desc = "Protein name fragment(s) to match, OR-ed. Example: [\"insulin\"]."]
    pub protein_name: Option<Vec<String>>,

    #[desc = "Organism name(s). Example: [\"Homo sapiens\"]."]
    pub organism: Option<Vec<String>>,

    #[desc = "NCBI taxon ID, e.g. 9606 (human), 10090 (mouse). Prefer this over `organism`."]
    pub organism_id: Option<u64>,

    #[desc = "UniProt accessions to fetch, e.g. [\"P01308\", \"P0DTC2\"]."]
    pub accessions: Option<Vec<String>>,

    #[desc = "Controlled-vocabulary keyword(s), e.g. [\"Glycoprotein\"]."]
    pub keyword: Option<Vec<String>>,

    #[desc = "true = reviewed Swiss-Prot only (high curation quality); \
             false = TrEMBL only. Omit for both."]
    pub reviewed: Option<bool>,

    #[desc = "Raw UniProt query expression (expert mode). Used only when no typed \
             filter field is set. Example: 'length:[500 TO 1000] AND organism_id:9606'."]
    pub query: Option<String>,

    #[desc = "Maximum number of entries to return per page (default 25, max 500). \
             Response includes a next-page cursor when more results exist."]
    pub size: Option<u32>,

    #[desc = "Cursor from a previous response's 'Next page cursor' to fetch the \
             next page."]
    pub cursor: Option<String>,
}

pub struct UniprotSearchTool {
    pub(crate) client: Arc<UniProtClient>,
}

#[async_trait]
impl ToolFunction for UniprotSearchTool {
    type Input = UniprotSearchInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut req = if let Some(q) = input.query {
            SearchRequest::new(q)
        } else {
            let mut q = Query::new();
            if let Some(v) = input.gene {
                q = q.gene(v);
            }
            if let Some(v) = input.protein_name {
                q = q.protein_name(v);
            }
            if let Some(v) = input.organism {
                q = q.organism(v);
            }
            if let Some(t) = input.organism_id {
                q = q.organism_id(t);
            }
            if let Some(v) = input.accessions {
                q = q.accessions(v);
            }
            if let Some(v) = input.keyword {
                q = q.keyword(v);
            }
            if let Some(r) = input.reviewed {
                q = q.reviewed(r);
            }
            SearchRequest::new(q.build().map_err(super::json_err)?)
        };

        if let Some(size) = input.size {
            req = req.size(size);
        }
        if let Some(cursor) = input.cursor {
            req = req.cursor(cursor);
        }

        let page = self.client.search(&req).await.map_err(super::json_err)?;
        Ok(AgentToolResult::success(format_entries(&page)))
    }
}
