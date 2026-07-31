use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{
    client::GwasCatalogClient,
    format,
    rest::{EmbeddedSnps, RestPage, Snp, SnpQuery},
};

#[tool(
    name = "gwascatalog_snp",
    description = "Look up SNP metadata in the curated GWAS Catalog (REST API). \
                  Search by rsID, gene name, genomic range, disease trait, EFO trait, or PubMed ID. \
                  Returns rsID, functional class, chromosomal location(s), and nearby genes. \
                  \
                  `genomic_range` format: `chrom:start-end`, e.g. `1:1000000-2000000`."
)]
pub struct SnpInput {
    #[desc = "Variant rsID, e.g. `rs7329174`."]
    pub rs_id: Option<String>,
    #[desc = "Gene name to find associated SNPs."]
    pub gene: Option<String>,
    #[desc = "Genomic range: `chrom:start-end`, e.g. `1:1000000-2000000`."]
    pub genomic_range: Option<String>,
    #[desc = "Disease trait to find associated SNPs."]
    pub disease_trait: Option<String>,
    #[desc = "EFO trait label."]
    pub efo_trait: Option<String>,
    #[desc = "PubMed ID."]
    pub pubmed_id: Option<String>,
    #[desc = "Page number (0-indexed)."]
    pub page: Option<u32>,
    #[desc = "Page size (default 20)."]
    pub size: Option<u32>,
}

pub struct SnpTool {
    pub(crate) client: Arc<GwasCatalogClient>,
}

fn parse_genomic_range(s: &str) -> Result<(String, u64, u64), String> {
    let (chrom, range) = s
        .split_once(':')
        .ok_or("genomic_range must be `chrom:start-end`")?;
    let (start, end) = range
        .split_once('-')
        .ok_or("genomic_range must be `chrom:start-end`")?;
    Ok((
        chrom.to_string(),
        start.trim().parse().map_err(|_| "invalid start position")?,
        end.trim().parse().map_err(|_| "invalid end position")?,
    ))
}

#[async_trait]
impl ToolFunction for SnpTool {
    type Input = SnpInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        // Direct GET for single rsID (findByRsId returns a flat entity)
        if let Some(rs) = &input.rs_id {
            if input.gene.is_none()
                && input.genomic_range.is_none()
                && input.disease_trait.is_none()
                && input.efo_trait.is_none()
                && input.pubmed_id.is_none()
            {
                let snp: Snp = self
                    .client
                    .rest_get_snp(rs)
                    .await
                    .map_err(super::json_err)?;
                let md = format::format_rest_snps(&EmbeddedSnps {
                    single_nucleotide_polymorphisms: vec![snp],
                });
                return Ok(AgentToolResult::success(md));
            }
        }

        let query = if let Some(rs) = input.rs_id {
            SnpQuery::RsId(rs)
        } else if let Some(g) = input.gene {
            SnpQuery::Gene(g)
        } else if let Some(gr) = input.genomic_range {
            match parse_genomic_range(&gr) {
                Ok((chrom, start, end)) => SnpQuery::ChromBpRange {
                    chrom,
                    bp_start: start,
                    bp_end: end,
                },
                Err(e) => return Ok(AgentToolResult::error(e)),
            }
        } else if let Some(t) = input.disease_trait {
            SnpQuery::DiseaseTrait(t)
        } else if let Some(t) = input.efo_trait {
            SnpQuery::EfoTrait(t)
        } else if let Some(pm) = input.pubmed_id {
            SnpQuery::Pmid(pm)
        } else {
            return Ok(AgentToolResult::error(
                "Provide at least one of: rs_id, gene, genomic_range, disease_trait, efo_trait, pubmed_id.",
            ));
        };

        let resp: RestPage<EmbeddedSnps> = self
            .client
            .rest_find_snps(&query, input.page, input.size)
            .await
            .map_err(super::json_err)?;

        let embedded = resp._embedded.unwrap_or_default();
        let mut md = format::format_rest_snps(&embedded);
        md.push_str(&format!(
            "\n*Page {} of {} ({} total)*\n",
            resp.page.number, resp.page.total_pages, resp.page.total_elements
        ));
        Ok(AgentToolResult::success(md))
    }
}
