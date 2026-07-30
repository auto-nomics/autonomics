//! Async client for the [Open Targets Platform GraphQL API](
//! https://platform.opentargets.org/api).
//!
//! The API is public — no authentication or API key is required.
//!
//! # Example
//!
//! ```no_run
//! # use opentargets::OpenTargetsClient;
//! # #[tokio::main] async fn main() -> opentargets::Result<()> {
//! let client = OpenTargetsClient::new();
//! let brca1 = client.target("ENSG00000012048").await?;
//! assert_eq!(brca1.approved_symbol, "BRCA1");
//! # Ok(()) }
//! ```

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::associations::{
    AssociationPage, AssociatedDisease, AssociatedTarget, Pagination,
};
use crate::error::{OpenTargetsError, Result};
use crate::search::SearchResults;
use crate::types::{Disease, Drug, Meta, Study, Target, Variant};

/// Default GraphQL endpoint (API v4).
pub const DEFAULT_ENDPOINT: &str =
    "https://api.platform.opentargets.org/api/v4/graphql";

// Maximum page size accepted by the API (server enforces ≤ 3000).
const MAX_PAGE_SIZE: u32 = 3000;

// ---------------------------------------------------------------------------
// GraphQL selection sets (kept here so query strings stay close to types).
// ---------------------------------------------------------------------------

const TARGET_FRAGMENT: &str = r#"
id approvedSymbol approvedName biotype
genomicLocation { chromosome start end strand }
targetClass { label id level }
dbXrefs { id source }
synonyms { label source }
proteinIds { id source }
transcriptIds
functionDescriptions
isEssential
"#;

const DISEASE_FRAGMENT: &str = r#"
id name description
isTherapeuticArea
therapeuticAreas { id name }
parents { id name }
children { id name }
ancestors
dbXRefs
synonyms { relation terms }
"#;

const DRUG_FRAGMENT: &str = r#"
id name drugType maximumClinicalStage description
tradeNames { label source }
synonyms { label source }
crossReferences { ids source }
"#;

const STUDY_FRAGMENT: &str = r#"
id projectId traitFromSource studyType
pubmedId publicationTitle publicationJournal publicationFirstAuthor publicationDate
condition
nSamples nCases nControls hasSumstats summarystatsLocation initialSampleSize
cohorts qualityControls analysisFlags
discoverySamples { sampleSize ancestry }
replicationSamples { sampleSize ancestry }
"#;

const VARIANT_FRAGMENT: &str = r#"
id chromosome position referenceAllele alternateAllele
variantDescription hgvsId rsIds
alleleFrequencies
transcriptConsequences
mostSevereConsequence
"#;

const ASSOC_DISEASE_FRAGMENT: &str = r#"
score novelty
disease { id name }
datasourceScores { id score }
datatypeScores { id score }
"#;

const ASSOC_TARGET_FRAGMENT: &str = r#"
score novelty
target { id approvedSymbol approvedName biotype }
datasourceScores { id score }
datatypeScores { id score }
"#;

const SEARCH_FRAGMENT: &str = r#"
id name entity score description category highlights
"#;

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An async client for the Open Targets Platform GraphQL API.
#[derive(Debug, Clone)]
pub struct OpenTargetsClient {
    http: reqwest::Client,
    endpoint: String,
}

impl Default for OpenTargetsClient {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenTargetsClient {
    /// Create a client pointing at the default production endpoint.
    pub fn new() -> Self {
        Self::with_endpoint(DEFAULT_ENDPOINT)
    }

    /// Create a client pointing at a custom GraphQL endpoint (useful for
    /// staging / test mirrors).
    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            endpoint: endpoint.into(),
        }
    }

    /// Borrow the inner HTTP client (e.g. to set a timeout via a builder).
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    // ----- low-level -----------------------------------------------------

    /// Send an arbitrary GraphQL `query` (with `$`-variables) and deserialize
    /// the top-level `data` object into `T`.
    ///
    /// This is the escape-hatch for endpoints or fields not covered by the
    /// typed helpers below.
    pub async fn query<T: DeserializeOwned>(
        &self,
        request: &str,
        variables: Value,
    ) -> Result<T> {
        let body = json!({ "query": request, "variables": variables });
        let resp = self.http.post(&self.endpoint).json(&body).send().await?;
        let status = resp.status().as_u16();
        let text = resp.text().await?;
        if !(200..300).contains(&status) {
            return Err(OpenTargetsError::Status { status, body: text });
        }

        let value: Value = serde_json::from_str(&text)?;
        if let Some(errs) = value.get("errors") {
            if !errs.is_null() {
                return Err(OpenTargetsError::GraphQl(errs.to_string()));
            }
        }
        let data = value
            .get("data")
            .ok_or_else(|| OpenTargetsError::EmptyData(value.clone()))?;
        Ok(serde_json::from_value(data.clone())?)
    }

    /// Run a *named-operation* query and extract a single top-level field by
    /// `field_name`. Handles the common `{ data: { <field>: <T> } }` shape.
    async fn field<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: Value,
        field_name: &str,
    ) -> Result<T> {
        // Reuse `query` to get the `data` object, then pick the field.
        let data: Value = self.query(query, variables).await?;
        let v = data
            .get(field_name)
            .cloned()
            .unwrap_or(Value::Null);
        Ok(serde_json::from_value(v)?)
    }

    // ----- meta ----------------------------------------------------------

    /// API / data version metadata.
    pub async fn meta(&self) -> Result<Meta> {
        let q = "query { meta { name product dataPrefix downloads enableDataReleasePrefix
                apiVersion { x y z suffix }
                dataVersion { year month iteration } } }";
        self.field(q, json!({}), "meta").await
    }

    // ----- target --------------------------------------------------------

    /// Fetch a single target (gene) by Ensembl ID.
    pub async fn target(&self, ensembl_id: &str) -> Result<Option<Target>> {
        let q = format!(
            "query($id: String!) {{ target(ensemblId: $id) {{ {TARGET_FRAGMENT} }} }}"
        );
        self.field(&q, json!({ "id": ensembl_id }), "target").await
    }

    /// Fetch several targets at once.
    pub async fn targets(&self, ensembl_ids: &[&str]) -> Result<Vec<Target>> {
        let q = format!(
            "query($ids: [String!]!) {{ targets(ensemblIds: $ids) {{ {TARGET_FRAGMENT} }} }}"
        );
        self.field(&q, json!({ "ids": ensembl_ids }), "targets").await
    }

    // ----- disease -------------------------------------------------------

    /// Fetch a single disease by EFO / MONDO / HP / Orphanet ID.
    pub async fn disease(&self, efo_id: &str) -> Result<Option<Disease>> {
        let q = format!(
            "query($id: String!) {{ disease(efoId: $id) {{ {DISEASE_FRAGMENT} }} }}"
        );
        self.field(&q, json!({ "id": efo_id }), "disease").await
    }

    /// Fetch several diseases at once.
    pub async fn diseases(&self, efo_ids: &[&str]) -> Result<Vec<Disease>> {
        let q = format!(
            "query($ids: [String!]!) {{ diseases(efoIds: $ids) {{ {DISEASE_FRAGMENT} }} }}"
        );
        self.field(&q, json!({ "ids": efo_ids }), "diseases").await
    }

    // ----- drug ----------------------------------------------------------

    /// Fetch a single drug by ChEMBL ID.
    pub async fn drug(&self, chembl_id: &str) -> Result<Option<Drug>> {
        let q = format!(
            "query($id: String!) {{ drug(chemblId: $id) {{ {DRUG_FRAGMENT} }} }}"
        );
        self.field(&q, json!({ "id": chembl_id }), "drug").await
    }

    /// Fetch several drugs at once.
    pub async fn drugs(&self, chembl_ids: &[&str]) -> Result<Vec<Drug>> {
        let q = format!(
            "query($ids: [String!]!) {{ drugs(chemblIds: $ids) {{ {DRUG_FRAGMENT} }} }}"
        );
        self.field(&q, json!({ "ids": chembl_ids }), "drugs").await
    }

    // ----- study ---------------------------------------------------------

    /// Fetch a single GWAS study by study ID (e.g. GCST…).
    pub async fn study(&self, study_id: &str) -> Result<Option<Study>> {
        let q = format!(
            "query($id: String!) {{ study(studyId: $id) {{ {STUDY_FRAGMENT} }} }}"
        );
        self.field(&q, json!({ "id": study_id }), "study").await
    }

    // ----- variant -------------------------------------------------------

    /// Fetch a single variant by `chr_pos_ref_alt` (GRCh38) ID.
    pub async fn variant(&self, variant_id: &str) -> Result<Option<Variant>> {
        let q = format!(
            "query($id: String!) {{ variant(variantId: $id) {{ {VARIANT_FRAGMENT} }} }}"
        );
        self.field(&q, json!({ "id": variant_id }), "variant").await
    }

    // ----- search --------------------------------------------------------

    /// Full-text search across entities.
    ///
    /// * `query_str` – the search string.
    /// * `entity_names` – optional filter, e.g. `Some(&["target", "disease"])`.
    /// * `page` – optional pagination (defaults to index 0, size 10).
    pub async fn search(
        &self,
        query_str: &str,
        entity_names: Option<&[&str]>,
        page: Option<Pagination>,
    ) -> Result<SearchResults> {
        let page = page.unwrap_or(Pagination::new(0, 10));
        let q = format!(
            "query($q: String!, $entities: [String!], $page: Pagination!) {{
                search(queryString: $q, entityNames: $entities, page: $page) {{
                    total hits {{ {SEARCH_FRAGMENT} }}
                }} }}"
        );
        let res: SearchResults = self
            .field(
                &q,
                json!({ "q": query_str, "entities": entity_names, "page": page }),
                "search",
            )
            .await?;
        Ok(res)
    }

    // ----- associations --------------------------------------------------

    /// One page of disease associations for a target.
    ///
    /// Results are ordered by descending overall score server-side.
    pub async fn associated_diseases(
        &self,
        ensembl_id: &str,
        page: Pagination,
        enable_indirect: bool,
        b_filter: Option<&str>,
    ) -> Result<AssociationPage<AssociatedDisease>> {
        let q = format!(
            "query($id: String!, $page: Pagination!, $indirect: Boolean!, $bf: String) {{
                target(ensemblId: $id) {{
                    associatedDiseases(page: $page, enableIndirect: $indirect, BFilter: $bf) {{
                        count rows {{ {ASSOC_DISEASE_FRAGMENT} }}
                    }} }} }}"
        );
        let data: Value = self
            .query(
                &q,
                json!({ "id": ensembl_id, "page": page, "indirect": enable_indirect, "bf": b_filter }),
            )
            .await?;
        let page = &data["target"]["associatedDiseases"];
        Ok(serde_json::from_value(page.clone())?)
    }

    /// One page of target associations for a disease.
    pub async fn associated_targets(
        &self,
        efo_id: &str,
        page: Pagination,
        enable_indirect: bool,
        b_filter: Option<&str>,
    ) -> Result<AssociationPage<AssociatedTarget>> {
        let q = format!(
            "query($id: String!, $page: Pagination!, $indirect: Boolean!, $bf: String) {{
                disease(efoId: $id) {{
                    associatedTargets(page: $page, enableIndirect: $indirect, BFilter: $bf) {{
                        count rows {{ {ASSOC_TARGET_FRAGMENT} }}
                    }} }} }}"
        );
        let data: Value = self
            .query(
                &q,
                json!({ "id": efo_id, "page": page, "indirect": enable_indirect, "bf": b_filter }),
            )
            .await?;
        let page = &data["disease"]["associatedTargets"];
        Ok(serde_json::from_value(page.clone())?)
    }

    /// Auto-paginate **all** disease associations for a target, fetching in
    /// `MAX_PAGE_SIZE`-sized pages until `count` is reached.
    ///
    /// This is the common case for downstream target-prioritisation pipelines.
    pub async fn associated_diseases_all(
        &self,
        ensembl_id: &str,
    ) -> Result<Vec<AssociatedDisease>> {
        self.associated_diseases_all_filtered(ensembl_id, false, None)
            .await
    }

    /// Same as [`associated_diseases_all`] but with `enableIndirect` / `BFilter`.
    pub async fn associated_diseases_all_filtered(
        &self,
        ensembl_id: &str,
        enable_indirect: bool,
        b_filter: Option<&str>,
    ) -> Result<Vec<AssociatedDisease>> {
        let mut out = Vec::new();
        let mut index = 0u32;
        let size = MAX_PAGE_SIZE;
        loop {
            let page = self
                .associated_diseases(ensembl_id, Pagination::new(index, size), enable_indirect, b_filter)
                .await?;
            let count = page.count;
            let got = page.rows.len();
            out.extend(page.rows);
            if out.len() as i64 >= count || got == 0 {
                break;
            }
            index += 1;
        }
        Ok(out)
    }

    /// Auto-paginate **all** target associations for a disease.
    pub async fn associated_targets_all(
        &self,
        efo_id: &str,
    ) -> Result<Vec<AssociatedTarget>> {
        self.associated_targets_all_filtered(efo_id, false, None)
            .await
    }

    /// Same as [`associated_targets_all`] but with `enableIndirect` / `BFilter`.
    pub async fn associated_targets_all_filtered(
        &self,
        efo_id: &str,
        enable_indirect: bool,
        b_filter: Option<&str>,
    ) -> Result<Vec<AssociatedTarget>> {
        let mut out = Vec::new();
        let mut index = 0u32;
        let size = MAX_PAGE_SIZE;
        loop {
            let page = self
                .associated_targets(efo_id, Pagination::new(index, size), enable_indirect, b_filter)
                .await?;
            let count = page.count;
            let got = page.rows.len();
            out.extend(page.rows);
            if out.len() as i64 >= count || got == 0 {
                break;
            }
            index += 1;
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// camelCase → snake_case rename helper for Meta
// ---------------------------------------------------------------------------

// (no longer needed — structs use `#[serde(rename_all = "camelCase")]`)
