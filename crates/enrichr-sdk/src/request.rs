//! Request-side helpers: organism hosts and input validation.

use crate::error::{EnrichrError, Result};

/// Enrichr deploys one host per supported organism. Gene symbols for mouse
/// resolve through the human host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EnrichrHost {
    #[default]
    Human,
    Fly,
    Yeast,
    Worm,
    Fish,
}

impl EnrichrHost {
    /// Base URL of the host's Enrichr instance.
    pub fn endpoint(self) -> &'static str {
        match self {
            Self::Human => "https://maayanlab.cloud/Enrichr",
            Self::Fly => "https://maayanlab.cloud/FlyEnrichr",
            Self::Yeast => "https://maayanlab.cloud/YeastEnrichr",
            Self::Worm => "https://maayanlab.cloud/WormEnrichr",
            Self::Fish => "https://maayanlab.cloud/FishEnrichr",
        }
    }

    /// Base URL of the matching Speedrichr deployment.
    pub fn speedrichr_endpoint(self) -> &'static str {
        match self {
            Self::Human => "https://maayanlab.cloud/speedrichr/api",
            Self::Fly => "https://maayanlab.cloud/flyspeedrichr/api",
            Self::Yeast => "https://maayanlab.cloud/yeastspeedrichr/api",
            Self::Worm => "https://maayanlab.cloud/wormspeedrichr/api",
            Self::Fish => "https://maayanlab.cloud/fishspeedrichr/api",
        }
    }
}

/// Default description sent with `addList` submissions.
pub const DEFAULT_LIST_DESCRIPTION: &str = "enrichr-sdk gene list";

/// Validate and normalize a gene list: trim every symbol, reject empty
/// entries, and require at least one gene.
pub(crate) fn normalize_genes<S: AsRef<str>>(
    genes: &[S],
    field: &'static str,
) -> Result<Vec<String>> {
    let mut normalized = Vec::with_capacity(genes.len());
    for gene in genes {
        let gene = gene.as_ref().trim();
        if gene.is_empty() {
            return Err(EnrichrError::InvalidRequest(format!(
                "{field} contains an empty gene symbol"
            )));
        }
        normalized.push(gene.to_owned());
    }
    if normalized.is_empty() {
        return Err(EnrichrError::InvalidRequest(format!(
            "{field} requires at least one gene symbol"
        )));
    }
    Ok(normalized)
}

/// Validate a `backgroundType` library name.
pub(crate) fn validate_library_name(library: &str) -> Result<String> {
    let library = library.trim();
    if library.is_empty() {
        return Err(EnrichrError::InvalidRequest(
            "background_type cannot be empty; call `libraries` for valid names".into(),
        ));
    }
    Ok(library.to_owned())
}

/// Validate a `userListId` returned by `addList`.
pub(crate) fn validate_user_list_id(user_list_id: u64) -> Result<()> {
    if user_list_id == 0 {
        return Err(EnrichrError::InvalidRequest(
            "user_list_id must be a positive integer returned by addList".into(),
        ));
    }
    Ok(())
}

/// Validate a Speedrichr background token.
pub(crate) fn validate_background_id(background_id: &str) -> Result<String> {
    let background_id = background_id.trim();
    if background_id.is_empty() {
        return Err(EnrichrError::InvalidRequest(
            "background_id cannot be empty; it is returned by speedrichr_add_background".into(),
        ));
    }
    Ok(background_id.to_owned())
}

/// Validate a single gene symbol for `genemap`.
pub(crate) fn validate_gene_symbol(gene: &str) -> Result<String> {
    let gene = gene.trim();
    if gene.is_empty() {
        return Err(EnrichrError::InvalidRequest("gene cannot be empty".into()));
    }
    Ok(gene.to_owned())
}

/// Join genes with newlines — the wire format of every gene-list field.
pub(crate) fn joined_lines(genes: &[String]) -> String {
    genes.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_trims_and_rejects_empty() {
        let genes = normalize_genes(&[" TP53 ", "BRCA1"], "genes").unwrap();
        assert_eq!(genes, vec!["TP53".to_owned(), "BRCA1".to_owned()]);
        assert!(normalize_genes(&["", "TP53"], "genes").is_err());
        let empty: Vec<&str> = Vec::new();
        assert!(normalize_genes(&empty, "genes").is_err());
    }

    #[test]
    fn host_endpoints_are_distinct() {
        assert_ne!(EnrichrHost::Human.endpoint(), EnrichrHost::Fly.endpoint());
        assert!(
            EnrichrHost::Human
                .speedrichr_endpoint()
                .ends_with("/speedrichr/api")
        );
    }
}
