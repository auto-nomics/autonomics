//! `source_gwascatalog_summary_associations` — per-variant summary
//! statistics as a dataframe, served from the EBI FTP (HTTPS mirror).
//!
//! The upstream Summary Statistics REST API was retired (every endpoint
//! answers `410 Gone`); EBI names the FTP mirror as the sanctioned access
//! path. This node therefore resolves the study's published TSV files on
//! the mirror, streams them, and applies the row filters locally:
//!
//! - `reveal: harmonised` (default) parses every `harmonised/*.h.tsv.gz`
//!   file of the study (multi-trait studies publish one per EFO term);
//! - `reveal: raw` parses the study's `{accession}_build{assembly}.tsv`
//!   (assembly preference: explicit `assembly`, else GRCh38 over GRCh37);
//! - `p_lower` / `p_upper` and `max_results` filter and cap the output
//!   table after parsing (previously server-side query parameters).
//!
//! `variant_id`-keyed queries (one variant across all studies) have no FTP
//! equivalent and are rejected with guidance.

use std::io::BufRead;
use std::sync::Arc;

use arrow_array::{Float64Array, RecordBatch, UInt32Array, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, DagNode, NodePorts, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::nodes::util;

/// Spec for [`SummaryAssociationsNode`]. Exactly one target is required:
/// `study_accession` (supported) or `variant_id` (rejected since the
/// Summary Statistics API retirement — see module docs).
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct SummaryAssociationsSpec {
    /// Study accession whose summary statistics to load, e.g.
    /// `GCST90000061`.
    #[serde(default)]
    pub study_accession: Option<String>,
    /// Rejected: the retired Summary Statistics API was the only
    /// variant-keyed access path. Use `study_accession` instead.
    #[serde(default)]
    pub variant_id: Option<String>,
    /// Which data representation to return: `harmonised` (default, parses
    /// `harmonised/*.h.tsv.gz`) or `raw` (per-build `.tsv`). `all` is
    /// accepted and behaves as `harmonised` (the API distinction no longer
    /// exists on the FTP path).
    #[serde(default)]
    pub reveal: Option<String>,
    /// Genome assembly for `reveal: raw` files: `GRCh37` or `GRCh38`. If
    /// omitted, prefers GRCh38 and falls back to GRCh37. Ignored for
    /// harmonised files (always GRCh38 coordinates).
    #[serde(default)]
    pub assembly: Option<String>,
    /// Lower p-value bound (inclusive), applied after parsing.
    #[serde(default)]
    pub p_lower: Option<f64>,
    /// Upper p-value bound (inclusive), applied after parsing.
    #[serde(default)]
    pub p_upper: Option<f64>,
    /// Ignored — retained for spec compatibility with the retired API,
    /// which paginated server-side.
    #[serde(default)]
    pub page_size: Option<u32>,
    /// Maximum total rows emitted (default 250, matching the retired
    /// API's one-page default).
    #[serde(default)]
    pub max_results: Option<u32>,
}

/// Which published file set to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reveal {
    Harmonised,
    Raw,
}

fn reveal_mode(spec: &SummaryAssociationsSpec) -> Result<Reveal, DagError> {
    match spec.reveal.as_deref().map(str::trim) {
        None | Some("") | Some("harmonised") | Some("all") => Ok(Reveal::Harmonised),
        Some("raw") => Ok(Reveal::Raw),
        Some(other) => Err(DagError::Schedule(format!(
            "source_gwascatalog_summary_associations: `reveal` must be `harmonised`, \
             `all`, or `raw`; got {other:?}"
        ))),
    }
}

/// Source node emitting summary-statistics rows from the FTP mirror.
#[derive(Clone)]
pub struct SummaryAssociationsNode {
    meta: NodePorts,
    spec: SummaryAssociationsSpec,
}

pub struct SummaryAssociationsNodeFactory;

impl NodeFactory for SummaryAssociationsNodeFactory {
    fn kind(&self) -> &'static str {
        "source_gwascatalog_summary_associations"
    }

    fn desc(&self) -> &'static str {
        "Fetch harmonised GWAS summary statistics (beta, SE, p) as a table."
    }

    fn doc(&self) -> &'static str {
        "A source node over the EBI GWAS Catalog FTP (HTTPS mirror) — the \
         per-variant effect sizes behind published GWAS studies. The \
         upstream Summary Statistics REST API was retired (410 Gone on \
         every endpoint), so this node parses the study's published TSV \
         files directly.\n\n\
         Requires `study_accession` (e.g. `GCST90000061`). `variant_id` \
         queries are no longer possible — the retired API was the only \
         variant-keyed access path.\n\n\
         `reveal` picks the file set: `harmonised` (default; parses every \
         `harmonised/*.h.tsv.gz`, using GRCh38 coordinates) or `raw` \
         (per-build `.tsv`, `assembly` picks GRCh37/GRCh38, default \
         GRCh38).\n\n\
         `p_lower`/`p_upper` narrow to a p-value window (the classic \
         `p_upper: 5e-8` genome-wide significance pull) and \
         `max_results` caps output rows (default 250). Both are applied \
         after parsing, so the full file is downloaded regardless.\n\n\
         Output schema: `variant_id, chromosome, base_pair_location, \
         study_accession, trait, hm_code, effect_allele, other_allele, \
         effect_allele_frequency, odds_ratio, ci_lower, ci_upper, beta, \
         standard_error, p_value`.\n\n\
         For full per-study files on disk (hundreds of MiB, consumed \
         downstream by `file_to_dataframe`) use `source_gwascatalog_download`."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(SummaryAssociationsSpec)
    }

    fn ports(&self) -> NodePorts {
        util::df_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let spec: SummaryAssociationsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(SummaryAssociationsNode {
            meta: util::df_port(),
            spec,
        }))
    }
}

// ── Row model ───────────────────────────────────────────────────────────────

/// One parsed summary-statistics row. Every field is optional: the FTP
/// files carry `NA` for missing values and raw submissions omit whole
/// columns.
#[derive(Debug, Clone, Default, PartialEq)]
struct FtpRow {
    variant_id: Option<String>,
    chromosome: Option<u32>,
    base_pair_location: Option<u64>,
    study_accession: Option<String>,
    trait_: Option<String>,
    hm_code: Option<u32>,
    effect_allele: Option<String>,
    other_allele: Option<String>,
    effect_allele_frequency: Option<f64>,
    odds_ratio: Option<f64>,
    ci_lower: Option<f64>,
    ci_upper: Option<f64>,
    beta: Option<f64>,
    standard_error: Option<f64>,
    p_value: Option<f64>,
}

/// Column indices resolved from a TSV header. Harmonised headers carry
/// `hm_*` columns (preferred); raw submissions only carry the fallback
/// names, and may omit even those.
#[derive(Debug, Default)]
struct ColMap {
    rsid: Option<usize>,
    variant_id: Option<usize>,
    chrom: Option<usize>,
    pos: Option<usize>,
    effect_allele: Option<usize>,
    other_allele: Option<usize>,
    eaf: Option<usize>,
    beta: Option<usize>,
    odds_ratio: Option<usize>,
    ci_lower: Option<usize>,
    ci_upper: Option<usize>,
    hm_code: Option<usize>,
    standard_error: Option<usize>,
    p_value: Option<usize>,
}

impl ColMap {
    /// Resolve column indices, preferring `hm_*` names over raw ones.
    /// Returns `None` when the header matches neither a variant id nor a
    /// p-value column (i.e. the file is not a summary-statistics TSV).
    fn build(header: &[&str]) -> Option<ColMap> {
        fn find(header: &[&str], names: &[&str]) -> Option<usize> {
            names
                .iter()
                .find_map(|n| header.iter().position(|h| h == n))
        }
        let map = ColMap {
            rsid: find(header, &["hm_rsid"]),
            variant_id: find(header, &["hm_variant_id", "variant_id"]),
            chrom: find(header, &["hm_chrom", "chromosome"]),
            pos: find(header, &["hm_pos", "base_pair_location"]),
            effect_allele: find(header, &["hm_effect_allele", "effect_allele"]),
            other_allele: find(header, &["hm_other_allele", "other_allele"]),
            eaf: find(header, &["hm_effect_allele_frequency", "effect_allele_frequency"]),
            beta: find(header, &["hm_beta", "beta"]),
            odds_ratio: find(header, &["hm_odds_ratio", "odds_ratio"]),
            ci_lower: find(header, &["hm_ci_lower", "ci_lower"]),
            ci_upper: find(header, &["hm_ci_upper", "ci_upper"]),
            hm_code: find(header, &["hm_code"]),
            standard_error: find(header, &["standard_error", "se"]),
            p_value: find(header, &["p_value"]),
        };
        if map.rsid.is_none() && map.variant_id.is_none() && map.p_value.is_none() {
            return None;
        }
        Some(map)
    }
}

/// A TSV cell, with `NA` and empty strings normalised to `None`.
fn cell<'a>(fields: &[&'a str], idx: Option<usize>) -> Option<&'a str> {
    let idx = idx?;
    let v = fields.get(idx)?.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("na") {
        None
    } else {
        Some(v)
    }
}

fn opt_f64(fields: &[&str], idx: Option<usize>) -> Option<f64> {
    cell(fields, idx)?.parse::<f64>().ok()
}

fn opt_u32(fields: &[&str], idx: Option<usize>) -> Option<u32> {
    cell(fields, idx)?.parse::<u32>().ok()
}

fn opt_u64(fields: &[&str], idx: Option<usize>) -> Option<u64> {
    cell(fields, idx)?.parse::<u64>().ok()
}

/// Parse summary-statistics rows out of a TSV reader.
///
/// * `trait_label` — the study/trait label to attach to every row (from
///   the file's `-meta.yaml` sidecar; the TSV itself has no trait column).
/// * `limit` — stop once `rows` reaches this length (0 = no limit), so a
///   `max_results` cap bounds parsing work.
///
/// Leading `#` comment lines (GWAS-SSF preambles) are skipped. Returns the
/// number of data lines consumed.
fn parse_rows<R: BufRead>(
    reader: R,
    study_accession: &str,
    trait_label: Option<&str>,
    p_lower: Option<f64>,
    p_upper: Option<f64>,
    limit: usize,
    rows: &mut Vec<FtpRow>,
) -> std::io::Result<usize> {
    let mut colmap: Option<ColMap> = None;
    let mut seen = 0usize;
    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            // Comment preamble (GWAS-SSF metadata) — skip until the real
            // header line arrives.
            continue;
        }
        let map: &ColMap = match colmap.as_ref() {
            Some(map) => map,
            None => {
                let header: Vec<&str> = line.split('\t').collect();
                let map = ColMap::build(&header).unwrap_or_default();
                colmap = Some(map);
                // The line just consumed was the header, not data.
                continue;
            }
        };
        if map.rsid.is_none() && map.variant_id.is_none() && map.p_value.is_none() {
            return Err(std::io::Error::other(format!(
                "unrecognised summary-statistics header: {}",
                line.chars().take(120).collect::<String>()
            )));
        }
        seen += 1;
        let fields: Vec<&str> = line.split('\t').collect();
        let row = FtpRow {
            variant_id: cell(&fields, map.rsid)
                .or_else(|| cell(&fields, map.variant_id))
                .map(str::to_string),
            chromosome: opt_u32(&fields, map.chrom),
            base_pair_location: opt_u64(&fields, map.pos),
            study_accession: Some(study_accession.to_string()),
            trait_: trait_label.map(str::to_string),
            hm_code: opt_u32(&fields, map.hm_code),
            effect_allele: cell(&fields, map.effect_allele).map(str::to_string),
            other_allele: cell(&fields, map.other_allele).map(str::to_string),
            effect_allele_frequency: opt_f64(&fields, map.eaf),
            odds_ratio: opt_f64(&fields, map.odds_ratio),
            ci_lower: opt_f64(&fields, map.ci_lower),
            ci_upper: opt_f64(&fields, map.ci_upper),
            beta: opt_f64(&fields, map.beta),
            standard_error: opt_f64(&fields, map.standard_error),
            p_value: opt_f64(&fields, map.p_value),
        };
        // p-window filter: rows whose p cannot be verified against a set
        // bound are dropped (matches the retired API's server-side filter).
        if let Some(lo) = p_lower {
            if row.p_value.is_none_or(|p| p < lo) {
                continue;
            }
        }
        if let Some(hi) = p_upper {
            if row.p_value.is_none_or(|p| p > hi) {
                continue;
            }
        }
        rows.push(row);
        if limit > 0 && rows.len() >= limit {
            break;
        }
    }
    Ok(seen)
}

/// Extract the `trait_description` list from a harmonised `-meta.yaml`
/// sidecar (e.g. `trait_description:\n  - Pulse pressure`). Missing or
/// unparsable input yields `None`.
fn trait_from_meta_yaml(text: &str) -> Option<String> {
    let mut in_block = false;
    let mut items: Vec<&str> = Vec::new();
    for line in text.lines() {
        if in_block {
            let trimmed = line.trim_start();
            if let Some(item) = trimmed.strip_prefix("- ") {
                items.push(item.trim());
                continue;
            }
            // First non-list line ends the block.
            if !line.trim().is_empty() {
                break;
            }
        } else if line.trim_end() == "trait_description:" {
            in_block = true;
        }
    }
    crate::nodes::joined(&items.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

/// A resolved FTP file to parse, with its optional `-meta.yaml` sidecar.
struct FileTarget {
    name: String,
    url: String,
    gzipped: bool,
    meta_url: Option<String>,
}

/// Resolve the files to parse for a study on the FTP mirror.
async fn resolve_targets(
    client: &crate::GwasCatalogClient,
    accession: &str,
    reveal: Reveal,
    assembly: Option<&str>,
) -> Result<Vec<FileTarget>, DagError> {
    let block_url = client.ftp_block_url(accession).map_err(|e| {
        DagError::Schedule(format!("source_gwascatalog_summary_associations: {e}"))
    })?;
    let study_url = format!("{block_url}/{accession}");

    match reveal {
        Reveal::Harmonised => {
            let harm_url = format!("{study_url}/harmonised");
            let entries = client.list_ftp_directory(&harm_url).await.map_err(|e| {
                DagError::Schedule(format!(
                    "source_gwascatalog_summary_associations: harmonised listing failed: {e}"
                ))
            })?;
            let mut targets = Vec::new();
            for name in &entries {
                if name.ends_with(".h.tsv.gz") {
                    let meta_name = format!("{name}-meta.yaml");
                    let meta_url = entries
                        .contains(&meta_name)
                        .then(|| format!("{harm_url}/{meta_name}"));
                    targets.push(FileTarget {
                        name: name.clone(),
                        url: format!("{harm_url}/{name}"),
                        gzipped: true,
                        meta_url,
                    });
                }
            }
            Ok(targets)
        }
        Reveal::Raw => {
            let entries = client.list_ftp_directory(&study_url).await.map_err(|e| {
                DagError::Schedule(format!(
                    "source_gwascatalog_summary_associations: FTP listing failed: {e}"
                ))
            })?;
            let mut candidates: Vec<&String> = entries
                .iter()
                .filter(|e| {
                    e.starts_with(&format!("{accession}_build"))
                        && (e.ends_with(".tsv") || e.ends_with(".tsv.gz"))
                })
                .collect();
            match assembly {
                Some(want) => candidates.sort_by_key(|e| !e.to_ascii_uppercase().contains(want)),
                None => candidates.sort_by_key(|e| !e.to_ascii_uppercase().contains("GRCH38")),
            }
            // One raw file: the preferred assembly only.
            candidates.first().map(|name| {
                let meta_name = format!("{name}-meta.yaml");
                let meta_url = entries
                    .contains(&meta_name)
                    .then(|| format!("{study_url}/{meta_name}"));
                vec![FileTarget {
                    name: (*name).clone(),
                    url: format!("{study_url}/{name}"),
                    gzipped: name.ends_with(".gz"),
                    meta_url,
                }]
            }).ok_or_else(|| {
                DagError::Schedule(format!(
                    "source_gwascatalog_summary_associations: no raw summary-statistics file \
                     found for {accession} (assembly={assembly:?})"
                ))
            })
        }
    }
}

#[async_trait]
impl DagNode for SummaryAssociationsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }

    fn kind(&self) -> &'static str {
        "source_gwascatalog_summary_associations"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let accession = self
            .spec
            .study_accession
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        let variant = self
            .spec
            .variant_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        match (&accession, &variant) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(DagError::Schedule(
                    "source_gwascatalog_summary_associations requires exactly one target — \
                     `study_accession` (not both, not neither)"
                        .into(),
                ));
            }
            (None, Some(rs)) => {
                return Err(DagError::Schedule(format!(
                    "source_gwascatalog_summary_associations: `variant_id` ({rs}) queries are \
                     no longer supported — the GWAS Catalog Summary Statistics API was retired \
                     upstream (410 Gone) and the FTP mirror is study-keyed only. Query by \
                     `study_accession`, or download full files with source_gwascatalog_download \
                     and filter locally."
                )));
            }
            (Some(_), None) => {}
        }
        let accession = accession.expect("validated above");
        let reveal = reveal_mode(&self.spec)?;
        let max_results = self
            .spec
            .max_results
            .unwrap_or(250)
            .max(1)
            .min(1_000_000) as usize;

        let client = crate::GwasCatalogClient::new();
        let targets = resolve_targets(
            &client,
            &accession,
            reveal,
            self.spec.assembly.as_deref().map(str::trim),
        )
        .await?;
        if targets.is_empty() {
            return Err(DagError::Schedule(format!(
                "source_gwascatalog_summary_associations: no harmonised summary-statistics \
                 files found for {accession} on the FTP mirror"
            )));
        }
        reporter.info(format!(
            "parsing {} file(s) for {accession} (reveal={:?})",
            targets.len(),
            self.spec.reveal
        ));

        let mut rows: Vec<FtpRow> = Vec::new();
        for target in &targets {
            if max_results > 0 && rows.len() >= max_results {
                break;
            }
            let data = client.http_get_bytes(&target.url).await.map_err(|e| {
                DagError::Schedule(format!(
                    "source_gwascatalog_summary_associations: download of {} failed: {e}",
                    target.name
                ))
            })?;
            let trait_label = match &target.meta_url {
                Some(url) => client
                    .http_get_text_if_available(url)
                    .await
                    .ok()
                    .flatten()
                    .as_deref()
                    .and_then(trait_from_meta_yaml),
                None => None,
            };
            let before = rows.len();
            let read_result = if target.gzipped {
                parse_rows(
                    std::io::BufReader::new(flate2::read::MultiGzDecoder::new(&data[..])),
                    &accession,
                    trait_label.as_deref(),
                    self.spec.p_lower,
                    self.spec.p_upper,
                    max_results,
                    &mut rows,
                )
            } else {
                parse_rows(
                    std::io::BufReader::new(&data[..]),
                    &accession,
                    trait_label.as_deref(),
                    self.spec.p_lower,
                    self.spec.p_upper,
                    max_results,
                    &mut rows,
                )
            };
            match read_result {
                Ok(seen) => reporter.info(format!(
                    "{}: {} data line(s) → {} row(s) (total {})",
                    target.name,
                    seen,
                    rows.len() - before,
                    rows.len()
                )),
                Err(e) => {
                    return Err(DagError::Schedule(format!(
                        "source_gwascatalog_summary_associations: parsing {} failed: {e}",
                        target.name
                    )))
                }
            }
        }
        if rows.is_empty() {
            reporter.warn(format!(
                "no rows matched the filters for {accession} (p_lower={:?}, p_upper={:?})",
                self.spec.p_lower, self.spec.p_upper
            ));
        }

        let batch = build_summary_batch(&rows)?;
        let df = ctx.session().read_batch(batch).map_err(|e| {
            DagError::Schedule(format!("failed to read summary-associations batch: {e}"))
        })?;
        let mut outputs = PortOutputs::new();
        outputs.insert(0, df);
        Ok(outputs)
    }
}

fn build_summary_batch(rows: &[FtpRow]) -> Result<RecordBatch, DagError> {
    let schema = Arc::new(Schema::new(vec![
        crate::nodes::utf8("variant_id"),
        Field::new("chromosome", DataType::UInt32, true),
        Field::new("base_pair_location", DataType::UInt64, true),
        crate::nodes::utf8("study_accession"),
        crate::nodes::utf8("trait"),
        Field::new("hm_code", DataType::UInt32, true),
        crate::nodes::utf8("effect_allele"),
        crate::nodes::utf8("other_allele"),
        Field::new("effect_allele_frequency", DataType::Float64, true),
        Field::new("odds_ratio", DataType::Float64, true),
        Field::new("ci_lower", DataType::Float64, true),
        Field::new("ci_upper", DataType::Float64, true),
        Field::new("beta", DataType::Float64, true),
        Field::new("standard_error", DataType::Float64, true),
        Field::new("p_value", DataType::Float64, true),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            crate::nodes::str_array(
                rows.iter().map(|r| r.variant_id.clone()).collect(),
            ),
            Arc::new(UInt32Array::from(
                rows.iter().map(|r| r.chromosome).collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(
                rows.iter().map(|r| r.base_pair_location).collect::<Vec<_>>(),
            )),
            crate::nodes::str_array(
                rows.iter().map(|r| r.study_accession.clone()).collect(),
            ),
            crate::nodes::str_array(rows.iter().map(|r| r.trait_.clone()).collect()),
            Arc::new(UInt32Array::from(
                rows.iter().map(|r| r.hm_code).collect::<Vec<_>>(),
            )),
            crate::nodes::str_array(
                rows.iter().map(|r| r.effect_allele.clone()).collect(),
            ),
            crate::nodes::str_array(
                rows.iter().map(|r| r.other_allele.clone()).collect(),
            ),
            Arc::new(Float64Array::from(
                rows.iter()
                    .map(|r| r.effect_allele_frequency)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.odds_ratio).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.ci_lower).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.ci_upper).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.beta).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.standard_error).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                rows.iter().map(|r| r.p_value).collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(|e| DagError::Schedule(format!("failed to build summary-associations batch: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::Array;

    const HARMONISED_HEADER: &str = "hm_variant_id\thm_rsid\thm_chrom\thm_pos\thm_other_allele\t\
        hm_effect_allele\thm_beta\thm_odds_ratio\thm_ci_lower\thm_ci_upper\t\
        hm_effect_allele_frequency\thm_code\tvariant_id\teffect_allele\tother_allele\t\
        effect_allele_frequency\tbeta\tstandard_error\tp_value\tn\tchromosome\t\
        base_pair_location\todds_ratio\tci_lower\tci_upper";

    fn harmonised_tsv() -> String {
        format!(
            "{HARMONISED_HEADER}\n\
             1_826940_C_T\trs75333668\t1\t826940\tC\tT\t0.041008318\tNA\tNA\tNA\t0.0011\t10\t\
             rs75333668\tt\tc\t0.0011\t0.041008318\t0.028969821\t0.1569\t739929\t1\t826940\tNA\tNA\tNA\n\
             1_925969_C_T\trs200686669\t1\t925969\tC\tT\t0.030007559\tNA\tNA\tNA\t0.0021\t10\t\
             rs200686669\tt\tc\t0.0021\t0.030007559\t0.051092581\t0.557\t624982\t1\t925969\tNA\tNA\tNA\n"
        )
    }

    #[test]
    fn parses_harmonised_rows_with_hm_preference() {
        let mut rows = Vec::new();
        let seen = parse_rows(
            std::io::Cursor::new(harmonised_tsv()),
            "GCST90000061",
            Some("Pulse pressure"),
            None,
            None,
            0,
            &mut rows,
        )
        .unwrap();
        assert_eq!(seen, 2);
        assert_eq!(rows.len(), 2);

        let r = &rows[0];
        assert_eq!(r.variant_id.as_deref(), Some("rs75333668")); // hm_rsid preferred
        assert_eq!(r.chromosome, Some(1));
        assert_eq!(r.base_pair_location, Some(826940));
        assert_eq!(r.study_accession.as_deref(), Some("GCST90000061"));
        assert_eq!(r.trait_.as_deref(), Some("Pulse pressure"));
        assert_eq!(r.hm_code, Some(10));
        assert_eq!(r.effect_allele.as_deref(), Some("T")); // hm_effect_allele, not the raw `t`
        assert_eq!(r.other_allele.as_deref(), Some("C"));
        assert!((r.effect_allele_frequency.unwrap() - 0.0011).abs() < 1e-12);
        assert!((r.beta.unwrap() - 0.041008318).abs() < 1e-12);
        assert!((r.standard_error.unwrap() - 0.028969821).abs() < 1e-12);
        assert!((r.p_value.unwrap() - 0.1569).abs() < 1e-12);
        assert_eq!(r.odds_ratio, None); // NA → null
        assert_eq!(r.ci_lower, None);
        assert_eq!(r.ci_upper, None);
    }

    #[test]
    fn parses_raw_submission_header_via_fallbacks() {
        // Column set of the real GCST90000061 raw file — no hm_* columns,
        // extra submission-specific columns present.
        let tsv = "variant_id\tMarkerName\teffect_allele\tother_allele\t\
             effect_allele_frequency\tFreqSE\tbeta\tstandard_error\tp_value\t\
             chromosome\tbase_pair_location\n\
             rs75333668\t1:762320\tt\tc\t0.0011\t0.0003\t0.041008318\t0.028969821\t0.1569\t1\t762320\n";
        let mut rows = Vec::new();
        parse_rows(
            std::io::Cursor::new(tsv),
            "GCST90000061",
            None,
            None,
            None,
            0,
            &mut rows,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.variant_id.as_deref(), Some("rs75333668"));
        assert_eq!(r.chromosome, Some(1));
        assert_eq!(r.base_pair_location, Some(762320));
        assert_eq!(r.effect_allele.as_deref(), Some("t"));
        assert_eq!(r.trait_, None);
        assert_eq!(r.hm_code, None);
        assert_eq!(r.odds_ratio, None);
    }

    #[test]
    fn skips_ssf_comment_preamble() {
        let tsv = format!(
            "# GWAS-SSF\n# genome_assembly: GRCh38\n{HARMONISED_HEADER}\n\
             1_826940_C_T\trs75333668\t1\t826940\tC\tT\t0.041\tNA\tNA\tNA\t0.0011\t10\t\
             rs75333668\tt\tc\t0.0011\t0.041\t0.028\t0.1569\t739929\t1\t826940\tNA\tNA\tNA\n"
        );
        let mut rows = Vec::new();
        parse_rows(
            std::io::Cursor::new(tsv),
            "GCST90000061",
            None,
            None,
            None,
            0,
            &mut rows,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn p_window_filters_rows() {
        let mut rows = Vec::new();
        parse_rows(
            std::io::Cursor::new(harmonised_tsv()),
            "GCST90000061",
            None,
            Some(0.2), // lower bound drops p=0.1569
            None,
            0,
            &mut rows,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert!((rows[0].p_value.unwrap() - 0.557).abs() < 1e-12);

        let mut rows = Vec::new();
        parse_rows(
            std::io::Cursor::new(harmonised_tsv()),
            "GCST90000061",
            None,
            None,
            Some(5e-8),
            0,
            &mut rows,
        )
        .unwrap();
        assert!(rows.is_empty(), "no genome-wide significant rows");
    }

    #[test]
    fn limit_caps_rows() {
        let mut rows = Vec::new();
        parse_rows(
            std::io::Cursor::new(harmonised_tsv()),
            "GCST90000061",
            None,
            None,
            None,
            1,
            &mut rows,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn unrecognised_header_is_an_error() {
        let mut rows = Vec::new();
        let err = parse_rows(
            std::io::Cursor::new("foo\tbar\n1\t2\n"),
            "GCST90000061",
            None,
            None,
            None,
            0,
            &mut rows,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unrecognised"), "{err}");
    }

    #[test]
    fn gzipped_input_round_trips() {
        let mut encoder =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, harmonised_tsv().as_bytes()).unwrap();
        let gz = encoder.finish().unwrap();
        let mut rows = Vec::new();
        parse_rows(
            std::io::BufReader::new(flate2::read::MultiGzDecoder::new(&gz[..])),
            "GCST90000061",
            None,
            None,
            None,
            0,
            &mut rows,
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn meta_yaml_trait_extraction() {
        let yaml = "# Study meta-data\ngwas_id: GCST90000061\n\
                    \ntrait_description:\n  - Pulse pressure\nontology_mapping:\n  - EFO_0005763\n";
        assert_eq!(
            trait_from_meta_yaml(yaml).as_deref(),
            Some("Pulse pressure")
        );
        // Multi-trait studies list several descriptions.
        let multi = "trait_description:\n  - Waist circumference\n  - Hip circumference\nx: 1\n";
        assert_eq!(
            trait_from_meta_yaml(multi).as_deref(),
            Some("Waist circumference; Hip circumference")
        );
        assert_eq!(trait_from_meta_yaml("unrelated: 1"), None);
    }

    #[test]
    fn batch_carries_effect_sizes_and_p() {
        let mut rows = Vec::new();
        parse_rows(
            std::io::Cursor::new(harmonised_tsv()),
            "GCST90000061",
            Some("Pulse pressure"),
            None,
            None,
            0,
            &mut rows,
        )
        .unwrap();
        let batch = build_summary_batch(&rows).unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.num_columns(), 15);
        let beta = batch
            .column(12)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((beta.value(0) - 0.041008318).abs() < 1e-12);
        let p = batch
            .column(14)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((p.value(0) - 0.1569).abs() < 1e-12);
        let trait_col = batch
            .column(4)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .unwrap();
        assert_eq!(trait_col.value(0), "Pulse pressure");
        let odds = batch
            .column(9)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(odds.is_null(0));
    }

    #[test]
    fn exactly_one_target_is_required() {
        let both = SummaryAssociationsSpec {
            study_accession: Some("GCST90002395".into()),
            variant_id: Some("rs7329174".into()),
            reveal: None,
            assembly: None,
            p_lower: None,
            p_upper: None,
            page_size: None,
            max_results: None,
        };
        assert!(both.study_accession.is_some() == both.variant_id.is_some());
        let only_study = SummaryAssociationsSpec {
            variant_id: None,
            ..both
        };
        assert!(only_study.study_accession.is_some() != only_study.variant_id.is_some());
    }

    #[test]
    fn reveal_parsing_rejects_unknown() {
        let spec = SummaryAssociationsSpec {
            study_accession: Some("GCST1".into()),
            variant_id: None,
            reveal: Some("everything".into()),
            assembly: None,
            p_lower: None,
            p_upper: None,
            page_size: None,
            max_results: None,
        };
        let err = reveal_mode(&spec).unwrap_err().to_string();
        assert!(err.contains("harmonised"), "{err}");
    }

    #[test]
    fn reveal_all_maps_to_harmonised() {
        let spec = SummaryAssociationsSpec {
            study_accession: Some("GCST1".into()),
            variant_id: None,
            reveal: Some("all".into()),
            assembly: None,
            p_lower: None,
            p_upper: None,
            page_size: None,
            max_results: None,
        };
        assert_eq!(reveal_mode(&spec).unwrap(), Reveal::Harmonised);
        let spec = SummaryAssociationsSpec {
            reveal: Some("raw".into()),
            ..spec
        };
        assert_eq!(reveal_mode(&spec).unwrap(), Reveal::Raw);
    }
}
