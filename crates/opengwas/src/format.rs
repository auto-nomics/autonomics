//! Render raw OpenGWAS API responses into clean, LLM-friendly Markdown.
//!
//! Every formatter preserves **all** non-null fields from the source data.
//! Tabular results become Markdown tables; metadata becomes structured
//! key-value cards.
//!
//! Table-fetching endpoints (associations, phewas, tophits, variants, ld_clump,
//! gwasinfo) have been migrated to DAG source nodes. Their formatters are kept
//! here for potential reuse but are currently unused outside of tests.

#![allow(dead_code)]

use serde_json::Value;

use crate::types::GwasInfo;

// ---------------------------------------------------------------------------
// Generic helpers
// ---------------------------------------------------------------------------

/// Extract a string field from a JSON object, falling back to `default`.
pub fn str_or(value: &Value, key: &str, default: &str) -> String {
    value
        .get(key)
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => default.to_string(),
        })
        .unwrap_or_else(|| default.to_string())
}

/// Format a numeric value compactly.
///
/// - Floats in scientific notation for very small / very large magnitudes.
/// - Integers are printed as-is with comma separators.
pub fn format_number(value: &Value) -> String {
    match value {
        Value::Null => "-".to_string(),
        Value::Number(n) => {
            // Prefer integer formatting for whole numbers.
            if let Some(i) = n.as_i64() {
                format_comma(i)
            } else if let Some(f) = n.as_f64() {
                if f == 0.0 {
                    return "0".to_string();
                }
                let abs = f.abs();
                // Use scientific notation for very small or very large floats.
                if abs >= 1e6 || (abs < 0.001 && abs > 0.0) {
                    format!("{:.4e}", f)
                } else {
                    // Up to 6 decimal places, trim trailing zeros.
                    let s = format!("{:.6}", f);
                    let s = s.trim_end_matches('0').trim_end_matches('.');
                    s.to_string()
                }
            } else {
                n.to_string()
            }
        }
        other => other.to_string(),
    }
}

/// Format an integer with comma separators (e.g. 184305 → "184,305").
fn format_comma(n: i64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let digits: Vec<char> = s.chars().collect();
    for (i, c) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c);
    }
    out
}

/// Convert an array of JSON objects into a Markdown table.
///
/// All keys that appear in any row become columns. Cells are rendered
/// with [`format_number`] for numeric values and [`str_or`] otherwise.
/// Missing cells show "-".
pub fn value_to_table(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "No records returned.".to_string();
    }

    // Collect the union of all keys across rows.
    let mut columns: Vec<String> = Vec::new();
    let mut col_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in rows {
        if let Some(obj) = row.as_object() {
            for key in obj.keys() {
                if col_set.insert(key.clone()) {
                    columns.push(key.clone());
                }
            }
        }
    }

    // Heuristic column ordering: put common GWAS fields first.
    let priority = [
        "rsid",
        "variant",
        "chr",
        "chromosome",
        "position",
        "pos",
        "ea",
        "nea",
        "eaf",
        "beta",
        "se",
        "pval",
        "p",
        "nsnp",
        "trait",
        "study_id",
        "id",
        "samplesize",
        "sample_size",
        "ncase",
        "ncontrol",
        "unit",
        "population",
    ];
    columns.sort_by(|a, b| {
        let ai = priority.iter().position(|&p| p == a).unwrap_or(999);
        let bi = priority.iter().position(|&p| p == b).unwrap_or(999);
        ai.cmp(&bi)
    });

    let mut out = String::with_capacity(1024);

    // Header row
    out.push_str("| ");
    out.push_str(&columns.join(" | "));
    out.push_str(" |\n");

    // Separator
    out.push('|');
    for _ in &columns {
        out.push_str("------|");
    }
    out.push('\n');

    // Data rows
    for row in rows {
        out.push_str("| ");
        for col in &columns {
            let cell = row.get(col);
            let s = match cell {
                Some(Value::Null) => "-".to_string(),
                Some(v @ Value::Number(_)) => format_number(v),
                Some(Value::String(s)) => s.clone(),
                Some(Value::Bool(b)) => b.to_string(),
                Some(Value::Array(arr)) if arr.len() == 1 => {
                    arr[0].to_string().trim_matches('"').to_string()
                }
                Some(other) => other.to_string(),
                None => "-".to_string(),
            };
            out.push_str(&s);
            out.push_str(" | ");
        }
        out.push('\n');
    }

    out
}

/// Extract a flat array from a value that might be an object keyed by ID
/// (e.g. `{ "ieu-a-2": [{...}, ...], "ukb-b-1": [...] }`), or already an array.
fn extract_rows(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(arr) => arr.clone(),
        Value::Object(map) => {
            // Check if all values are arrays → treat as keyed groups.
            let all_arrays = map.values().all(|v| v.is_array());
            if all_arrays && !map.is_empty() {
                map.values()
                    .flat_map(|v| v.as_array().into_iter().flatten().cloned())
                    .collect()
            } else {
                // Single object → treat as one row.
                vec![value.clone()]
            }
        }
        _ => vec![value.clone()],
    }
}

/// Format a single JSON object as key-value lines.
pub fn obj_to_kv(value: &Value) -> String {
    match value.as_object() {
        Some(map) => {
            let mut out = String::with_capacity(512);
            for (k, v) in map {
                let display = match v {
                    Value::Null => "-".to_string(),
                    Value::String(s) => s.clone(),
                    v @ Value::Number(_) => format_number(v),
                    Value::Bool(b) => b.to_string(),
                    Value::Array(arr) => {
                        let items: Vec<String> = arr
                            .iter()
                            .map(|item| match item {
                                Value::String(s) => s.clone(),
                                v @ Value::Number(_) => format_number(v),
                                other => other.to_string(),
                            })
                            .collect();
                        items.join(", ")
                    }
                    Value::Object(_) => v.to_string(),
                };
                out.push_str(&format!("**{}:** {}\n", k, display));
            }
            out
        }
        None => value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// GwasInfo (typed) — used by gwasinfo_by_id, gwasinfo_search
// ---------------------------------------------------------------------------

/// Render a list of [`GwasInfo`] records as structured Markdown cards.
///
/// If `keyword` / `field` are provided (search context), they are included
/// in the header.
pub fn format_gwasinfo_table(
    datasets: &[GwasInfo],
    keyword: Option<&str>,
    field: Option<&str>,
) -> String {
    if datasets.is_empty() {
        let mut msg = "No matching datasets found.".to_string();
        if let Some(kw) = keyword {
            msg.push_str(&format!(" (keyword: \"{kw}\")"));
        }
        return msg;
    }

    let mut out = String::with_capacity(256 * datasets.len());

    out.push_str(&format!("Found **{}** datasets\n\n", datasets.len()));

    if let Some(kw) = keyword {
        let f = field.unwrap_or("trait");
        out.push_str(&format!("Searched **{f}** for \"{kw}\"\n\n"));
    }

    for ds in datasets {
        let id = ds.id.as_deref().unwrap_or("?");
        out.push_str(&format!("### {id}\n"));

        // Line 1: trait, population, sex
        let mut line1 = Vec::new();
        if let Some(t) = &ds.trait_ {
            line1.push(format!("**Trait:** {t}"));
        }
        if let Some(p) = &ds.population {
            line1.push(format!("**Population:** {p}"));
        }
        if let Some(s) = &ds.sex {
            line1.push(format!("**Sex:** {s}"));
        }
        if !line1.is_empty() {
            out.push_str(&line1.join(" | "));
            out.push('\n');
        }

        // Line 2: author, year, pmid
        let mut line2 = Vec::new();
        if let Some(a) = &ds.author {
            line2.push(format!("**Author:** {a}"));
        }
        if let Some(y) = ds.year {
            line2.push(format!("**Year:** {y}"));
        }
        if let Some(p) = ds.pmid {
            line2.push(format!("**PMID:** {p}"));
        }
        if !line2.is_empty() {
            out.push_str(&line2.join(" | "));
            out.push('\n');
        }

        // Line 3: nsnp, sample_size, ncase/ncontrol
        let mut line3 = Vec::new();
        if let Some(n) = ds.nsnp {
            line3.push(format!("**SNPs:** {}", format_comma(n)));
        }
        if let Some(n) = ds.sample_size {
            line3.push(format!("**Sample size:** {}", format_comma(n)));
        }
        if let Some(nc) = ds.ncase {
            let mut s = format!("**Cases:** {}", format_comma(nc));
            if let Some(nco) = ds.ncontrol {
                s.push_str(&format!(" | **Controls:** {}", format_comma(nco)));
            }
            line3.push(s);
        }
        if !line3.is_empty() {
            out.push_str(&line3.join(" | "));
            out.push('\n');
        }

        // Line 4: doi, build, category/subcategory
        let mut line4 = Vec::new();
        if let Some(d) = &ds.doi {
            line4.push(format!("**DOI:** {d}"));
        }
        if let Some(b) = &ds.build {
            line4.push(format!("**Build:** {b}"));
        }
        if let Some(c) = &ds.category {
            let mut s = format!("**Category:** {c}");
            if let Some(sc) = &ds.subcategory {
                s.push_str(&format!(" / {sc}"));
            }
            line4.push(s);
        }
        if let Some(g) = &ds.group_name {
            line4.push(format!("**Group:** {g}"));
        }
        if !line4.is_empty() {
            out.push_str(&line4.join(" | "));
            out.push('\n');
        }

        // Line 5: MR, SD, consortium, unit, ontology
        let mut line5 = Vec::new();
        if let Some(mr) = ds.mr {
            line5.push(format!("**MR:** {mr}"));
        }
        if let Some(sd) = ds.sd {
            line5.push(format!("**SD:** {sd}"));
        }
        if let Some(c) = &ds.consortium {
            line5.push(format!("**Consortium:** {c}"));
        }
        if let Some(u) = &ds.unit {
            line5.push(format!("**Unit:** {u}"));
        }
        if let Some(o) = &ds.ontology {
            line5.push(format!("**Ontology:** {o}"));
        }
        if !line5.is_empty() {
            out.push_str(&line5.join(" | "));
            out.push('\n');
        }

        // Line 6: remaining optional fields that might have data
        let mut line6 = Vec::new();
        if let Some(v) = &ds.study_design {
            line6.push(format!("**Study design:** {v}"));
        }
        if let Some(v) = &ds.covariates {
            line6.push(format!("**Covariates:** {v}"));
        }
        if let Some(v) = &ds.coverage {
            line6.push(format!("**Coverage:** {v}"));
        }
        if let Some(v) = &ds.imputation_panel {
            line6.push(format!("**Imputation:** {v}"));
        }
        if let Some(v) = &ds.beta_transformation {
            line6.push(format!("**Beta transform:** {v}"));
        }
        if let Some(v) = &ds.qc_prior_to_upload {
            line6.push(format!("**QC:** {v}"));
        }
        if !line6.is_empty() {
            out.push_str(&line6.join(" | "));
            out.push('\n');
        }

        // Note / priority (rare, only if present)
        let mut extras = Vec::new();
        if let Some(n) = &ds.note {
            extras.push(format!("**Note:** {n}"));
        }
        if let Some(p) = ds.priority {
            extras.push(format!("**Priority:** {p}"));
        }
        if let Some(nc) = ds.is_nc {
            extras.push(format!("**Non-coding:** {}", nc));
        }
        if !extras.is_empty() {
            out.push_str(&extras.join(" | "));
            out.push('\n');
        }

        out.push('\n');
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// gwasinfo_count
// ---------------------------------------------------------------------------

pub fn format_gwasinfo_count(count: i64) -> String {
    format!("Total cached datasets: **{}**", format_comma(count))
}

// ---------------------------------------------------------------------------
// Generic table formatters for pass-through endpoints
// ---------------------------------------------------------------------------

/// Format associations API response as a Markdown table.
///
/// The API may return `{ study_id: [rows...] }` or a flat array.
pub fn format_associations(value: &Value) -> String {
    // Check for keyed groups (object with all-array values) BEFORE flattening.
    if let Some(obj) = value.as_object() {
        let all_arrays = !obj.is_empty() && obj.values().all(|v| v.is_array());
        if all_arrays {
            return format_keyed_groups(value, "Associations");
        }
    }
    let rows = extract_rows(value);
    let mut out = format!("Found **{}** association records\n\n", rows.len());
    out.push_str(&value_to_table(&rows));
    out
}

/// Format tophits API response as a Markdown table.
pub fn format_tophits(value: &Value) -> String {
    let rows = extract_rows(value);
    let mut out = format!("Found **{}** top hits\n\n", rows.len());
    out.push_str(&value_to_table(&rows));
    out
}

/// Format phewas API response as a Markdown table.
pub fn format_phewas(value: &Value) -> String {
    let rows = extract_rows(value);
    let mut out = format!("Found **{}** PheWAS associations\n\n", rows.len());
    out.push_str(&value_to_table(&rows));
    out
}

/// Format variant info (by rsID or chr:pos) as a Markdown table.
pub fn format_variants(value: &Value) -> String {
    let rows = extract_rows(value);
    let mut out = format!("Found **{}** variants\n\n", rows.len());
    out.push_str(&value_to_table(&rows));
    out
}

/// Format LD clump results as a Markdown table.
pub fn format_ld_clump(value: &Value) -> String {
    let rows = extract_rows(value);
    let mut out = format!("Found **{}** clumped loci\n\n", rows.len());
    out.push_str(&value_to_table(&rows));
    out
}

/// Format LD matrix as an SNP×SNP correlation table.
///
/// The OpenGWAS `/ld/matrix` endpoint returns a flat array in row-major
/// order, paired with a SNP list whose entries carry allele suffixes
/// (`rs1205_A_G`). We reshape the flat array into an N×N table and
/// present it as a Markdown correlation matrix.
///
/// Two shapes are accepted (defensive):
///
/// 1. **Canonical** — `{ "snplist": ["rs1205_A_G", ...],
///                       "matrix":  [r00, r01, ..., r10, r11, ...] }`
/// 2. **Nested**   — `{ "rs1205": { "rs174546": 0.92, ... }, ... }`
///    (a more verbose, symmetrical per-row map — kept for compatibility)
pub fn format_ld_matrix(value: &Value) -> String {
    // -- Canonical {snplist, matrix} response --------------------------------
    if let Some(obj) = value.as_object() {
        if let (Some(snplist), Some(matrix)) = (obj.get("snplist"), obj.get("matrix")) {
            if let (Some(snps), Some(values)) = (snplist.as_array(), matrix.as_array()) {
                return format_ld_matrix_canonical(snps, values);
            }
        }
    }

    // -- Nested {rsid: {other_rsid: r}} response -----------------------------
    if let Some(map) = value.as_object() {
        if !map.is_empty() && map.values().all(|v| v.is_object()) {
            return format_ld_matrix_nested(map);
        }
    }

    // -- Anything else: dump as key-value so nothing is lost ------------------
    obj_to_kv(value)
}

/// Render the canonical `{snplist, matrix}` response as an N×N table.
fn format_ld_matrix_canonical(snps: &[Value], values: &[Value]) -> String {
    if snps.is_empty() {
        return "No LD matrix data returned.".to_string();
    }

    let n = snps.len();
    let expected = n * n;
    let have = values.len();

    // Strip trailing `_REF_ALT` allele suffixes so headers stay readable.
    let labels: Vec<String> = snps
        .iter()
        .map(|v| {
            let raw = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            strip_allele_suffix(&raw)
        })
        .collect();

    // Coerce every entry to f64; nulls / non-numerics become `None`.
    let flat: Vec<Option<f64>> = values
        .iter()
        .map(|v| match v {
            Value::Number(num) => num.as_f64(),
            Value::String(s) => s.parse::<f64>().ok(),
            _ => None,
        })
        .collect();

    // Reshape: row-major. If the count doesn't match an N×N square we still
    // emit what we can and surface the size mismatch to the model.
    let mut rows: Vec<Vec<Option<f64>>> = Vec::with_capacity(n);
    if have == expected {
        for i in 0..n {
            let start = i * n;
            rows.push(flat[start..start + n].to_vec());
        }
    } else {
        // Fall back to chunking by `n` so the model still sees row order.
        for chunk in flat.chunks(n.max(1)) {
            let mut row = chunk.to_vec();
            // Pad short final row so columns line up.
            while row.len() < n {
                row.push(None);
            }
            rows.push(row);
        }
        if rows.len() != n {
            // Hard mismatch — render as a flat list rather than a fake matrix.
            return format!(
                "LD matrix shape mismatch: snplist has {n} entries but matrix has \
                 {have} values (expected {expected}). Raw values:\n\n{}",
                obj_to_kv(&Value::Array(values.to_vec()))
            );
        }
    }

    let mut out = format!("LD matrix ({} SNPs, R values)\n\n", n);
    if have != expected {
        out.push_str(&format!(
            "_Warning: snplist has {n} entries but matrix has {have} values \
             (expected {expected}). Showing partial data._\n\n"
        ));
    }

    // Header: | | rs1 | rs2 | ... |
    out.push_str("| | ");
    out.push_str(&labels.join(" | "));
    out.push_str(" |\n");

    out.push('|');
    out.push_str("------|");
    for _ in &labels {
        out.push_str("--------|");
    }
    out.push('\n');

    // Rows
    for (i, row) in rows.iter().enumerate() {
        out.push_str(&format!("| **{}** | ", labels[i]));
        for (j, cell) in row.iter().enumerate() {
            if i == j {
                out.push_str("1.000 | ");
            } else {
                match cell {
                    Some(f) => out.push_str(&format!("{f:.3} | ")),
                    None => out.push_str("- | "),
                }
            }
        }
        out.push('\n');
    }

    out
}

/// Render the nested `{rsid: {other_rsid: r}}` response as an N×N table.
fn format_ld_matrix_nested(map: &serde_json::Map<String, Value>) -> String {
    let mut snps: Vec<String> = map.keys().cloned().collect();
    snps.sort();

    let mut out = format!("LD matrix ({} SNPs, R values)\n\n", snps.len());

    out.push_str("| | ");
    out.push_str(&snps.join(" | "));
    out.push_str(" |\n");

    out.push('|');
    out.push_str("------|");
    for _ in &snps {
        out.push_str("--------|");
    }
    out.push('\n');

    for snp in &snps {
        out.push_str(&format!("| **{}** | ", snp));
        let row_data = map.get(snp).and_then(|v| v.as_object());
        for target in &snps {
            if snp == target {
                out.push_str("1.000 | ");
            } else if let Some(val) = row_data.and_then(|r| r.get(target)) {
                let f = val.as_f64().unwrap_or(0.0);
                out.push_str(&format!("{f:.3} | "));
            } else {
                out.push_str("- | ");
            }
        }
        out.push('\n');
    }

    out
}

/// Strip a trailing `_REF_ALT` allele suffix from an `rsID_REF_ALT` label.
///
/// `rs1205_A_G` → `rs1205`
/// `rs174546`    → `rs174546` (no suffix, returned as-is)
///
/// Splits at the **first** underscore because rsIDs themselves never
/// contain underscores — the suffix is always at the tail.
fn strip_allele_suffix(label: &str) -> String {
    if let Some(idx) = label.find('_') {
        let (rsid, tail) = label.split_at(idx);
        let tail = &tail[1..]; // drop the leading '_'
        let parts: Vec<&str> = tail.split('_').collect();
        if parts.len() == 2 && parts.iter().all(|p| p.len() == 1 && is_allele_char(p)) {
            return rsid.to_string();
        }
    }
    label.to_string()
}

fn is_allele_char(c: &str) -> bool {
    matches!(c, "A" | "C" | "G" | "T" | "N" | "a" | "c" | "g" | "t" | "n")
}

/// Format download results (already structured as `{ count, files }`).
pub fn format_download(value: &Value) -> String {
    let count = value.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    let files = value.get("files").and_then(|v| v.as_array());

    let mut out = format!("Downloaded **{}** file(s)\n\n", count);

    if let Some(files) = files {
        if !files.is_empty() {
            out.push_str("| Study ID | Filename | Path | Size |\n");
            out.push_str("|----------|----------|------|------|\n");
            for f in files {
                let study = str_or(f, "study_id", "-");
                let filename = str_or(f, "filename", "-");
                let path = str_or(f, "path", "-");
                let size = f.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                let size_str = format_bytes(size);
                out.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    study, filename, path, size_str
                ));
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Format a response that is keyed by study ID with array values.
///
/// Shape: `{ "ieu-a-2": [{ ... }, ...], "ukb-b-1": [...] }`
fn format_keyed_groups(value: &Value, label: &str) -> String {
    let map = match value.as_object() {
        Some(m) => m,
        None => return format!("{label}:\n{}", obj_to_kv(value)),
    };

    let mut total = 0usize;
    let mut out = String::with_capacity(2048);

    for (key, val) in map {
        let rows: Vec<Value> = val.as_array().map(|a| a.to_vec()).unwrap_or_default();
        total += rows.len();

        out.push_str(&format!("### {key} ({} records)\n\n", rows.len()));
        out.push_str(&value_to_table(&rows));
        out.push_str("\n\n");
    }

    // Prepend summary.
    let summary = format!(
        "Found **{}** {label} across **{}** studies\n\n",
        total,
        map.len()
    );
    format!("{}{}", summary, out)
}

/// Format bytes into a human-readable string.
fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- helpers --

    #[test]
    fn test_str_or_string() {
        let v = json!({"name": "test"});
        assert_eq!(str_or(&v, "name", "-"), "test");
        assert_eq!(str_or(&v, "missing", "-"), "-");
    }

    #[test]
    fn test_str_or_number() {
        let v = json!({"count": 42});
        assert_eq!(str_or(&v, "count", "-"), "42");
    }

    #[test]
    fn test_format_number_scientific() {
        let v = json!(3.2e-14);
        assert_eq!(format_number(&v), "3.2000e-14");

        let v = json!(0.00005);
        assert_eq!(format_number(&v), "5.0000e-5");

        let v = json!(1.5e9);
        // serde_json stores this as f64 (not i64), so it gets scientific notation.
        assert_eq!(format_number(&v), "1.5000e9");

        let v = json!(0.0);
        assert_eq!(format_number(&v), "0");
    }

    #[test]
    fn test_format_number_normal() {
        let v = json!(0.042);
        assert_eq!(format_number(&v), "0.042");

        let v = json!(123.456);
        assert_eq!(format_number(&v), "123.456");
    }

    #[test]
    fn test_format_comma() {
        assert_eq!(format_comma(184305), "184,305");
        assert_eq!(format_comma(1000), "1,000");
        assert_eq!(format_comma(1000000), "1,000,000");
        assert_eq!(format_comma(42), "42");
    }

    // -- value_to_table --

    #[test]
    fn test_value_to_table_empty() {
        assert_eq!(value_to_table(&[]), "No records returned.");
    }

    #[test]
    fn test_value_to_table_basic() {
        let rows = vec![
            json!({"rsid": "rs1205", "chr": "7", "position": 105561135, "pval": 3.2e-14}),
            json!({"rsid": "rs174546", "chr": "1", "pval": 0.001}),
        ];
        let table = value_to_table(&rows);
        assert!(table.contains("| rsid | chr | position | pval |"));
        assert!(table.contains("| rs1205 | 7 | 105,561,135 | 3.2000e-14 |"));
        assert!(table.contains("| rs174546 | 1 | - | 0.001 |"));
    }

    #[test]
    fn test_value_to_table_column_order() {
        let rows = vec![json!({"z": 1, "a": 2, "pval": 3e-8, "rsid": "rs123"})];
        let table = value_to_table(&rows);
        // "rsid" should come before "z" due to priority ordering.
        let rsid_pos = table.find("rsid").unwrap();
        let z_pos = table.find("| z |").unwrap();
        assert!(rsid_pos < z_pos);
    }

    // -- extract_rows --

    #[test]
    fn test_extract_rows_array() {
        let v = json!([{"a": 1}, {"a": 2}]);
        let rows = extract_rows(&v);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_extract_rows_keyed_groups() {
        let v = json!({
            "ieu-a-2": [{"pval": 1e-8}],
            "ukb-b-1": [{"pval": 1e-5}]
        });
        let rows = extract_rows(&v);
        assert_eq!(rows.len(), 2);
    }

    // -- format_gwasinfo --

    #[test]
    fn test_format_gwasinfo_empty() {
        let result = format_gwasinfo_table(&[], None, None);
        assert_eq!(result, "No matching datasets found.");
    }

    #[test]
    fn test_format_gwasinfo_with_data() {
        let datasets = vec![GwasInfo {
            id: Some("ieu-a-2".into()),
            trait_: Some("Coronary artery disease".into()),
            population: Some("European".into()),
            sex: Some("Males and Females".into()),
            author: Some("Nikpay et al.".into()),
            year: Some(2015),
            nsnp: Some(1_000_000),
            sample_size: Some(184305),
            doi: Some("10.1186/s12916-015-0447-7".into()),
            build: Some("GRCh37".into()),
            category: Some("Binary".into()),
            subcategory: Some("Cardiovascular".into()),
            pmid: Some(26343387),
            mr: Some(1),
            sd: Some(1.0),
            ..Default::default()
        }];
        let result = format_gwasinfo_table(&datasets, None, None);
        assert!(result.contains("### ieu-a-2"));
        assert!(result.contains("**Trait:** Coronary artery disease"));
        assert!(result.contains("**Population:** European"));
        assert!(result.contains("**SNPs:** 1,000,000"));
        assert!(result.contains("**Sample size:** 184,305"));
        assert!(result.contains("**PMID:** 26343387"));
    }

    // -- format_gwasinfo_count --

    #[test]
    fn test_format_gwasinfo_count() {
        assert_eq!(
            format_gwasinfo_count(184305),
            "Total cached datasets: **184,305**"
        );
    }

    // -- format_associations --

    #[test]
    fn test_format_associations_flat() {
        let v = json!([
            {"rsid": "rs1205", "pval": 1e-10, "beta": 0.05},
            {"rsid": "rs174546", "pval": 1e-5, "beta": 0.02},
        ]);
        let result = format_associations(&v);
        assert!(result.contains("**2** association records"));
        assert!(result.contains("| rs1205 |"));
    }

    #[test]
    fn test_format_associations_keyed() {
        let v = json!({
            "ieu-a-2": [{"rsid": "rs1205", "pval": 1e-10}],
            "ukb-b-1": [{"rsid": "rs174546", "pval": 1e-5}],
        });
        let result = format_associations(&v);
        assert!(result.contains("**2** Associations across **2** studies"));
        assert!(result.contains("### ieu-a-2"));
    }

    // -- format_ld_matrix --

    #[test]
    fn test_format_ld_matrix_canonical() {
        // The actual OpenGWAS /ld/matrix response: flat matrix + snplist
        // with `_REF_ALT` allele suffixes, row-major.
        let v = json!({
            "snplist": ["rs1205_A_G", "rs174546_C_T"],
            "matrix": [1.0, 0.92, 0.92, 1.0]
        });
        let result = format_ld_matrix(&v);
        assert!(result.contains("LD matrix (2 SNPs"));
        assert!(result.contains("rs1205"));
        assert!(result.contains("rs174546"));
        // Allele suffixes must be stripped from the headers.
        assert!(!result.contains("rs1205_A_G"));
        // Diagonal is 1.000; off-diagonal is 0.92.
        assert!(result.contains("1.000"));
        assert!(result.contains("0.920"));
    }

    #[test]
    fn test_format_ld_matrix_nested_fallback() {
        // Defensive: if the API ever returns the nested shape, still render.
        let v = json!({
            "rs1205": {"rs174546": 0.92},
            "rs174546": {"rs1205": 0.92}
        });
        let result = format_ld_matrix(&v);
        assert!(result.contains("LD matrix (2 SNPs"));
        assert!(result.contains("1.000"));
        assert!(result.contains("0.920"));
    }

    #[test]
    fn test_format_ld_matrix_size_mismatch() {
        // snplist has 3 entries but matrix has 5 values — should warn.
        let v = json!({
            "snplist": ["rs1_A_G", "rs2_C_T", "rs3_G_A"],
            "matrix":   [1.0, 0.5, 0.3, 0.5, 1.0]
        });
        let result = format_ld_matrix(&v);
        assert!(
            result.contains("shape mismatch") || result.contains("Warning"),
            "expected mismatch warning, got: {result}"
        );
    }

    #[test]
    fn test_format_ld_matrix_empty() {
        let v = json!({"snplist": [], "matrix": []});
        assert_eq!(format_ld_matrix(&v), "No LD matrix data returned.");
    }

    #[test]
    fn test_format_ld_matrix_unknown_shape() {
        // Falls back to key-value dump.
        let v = json!("not an object");
        let result = format_ld_matrix(&v);
        assert!(result.contains("not an object"));
    }

    #[test]
    fn test_strip_allele_suffix() {
        assert_eq!(strip_allele_suffix("rs1205_A_G"), "rs1205");
        assert_eq!(strip_allele_suffix("rs174546_C_T"), "rs174546");
        assert_eq!(strip_allele_suffix("rs1234"), "rs1234");
        // Insufficient tokens — leave untouched.
        assert_eq!(strip_allele_suffix("rs1234_A"), "rs1234_A");
        // Non-allele trailing tokens — leave untouched.
        assert_eq!(strip_allele_suffix("rs1234_foo_bar"), "rs1234_foo_bar");
    }

    // -- format_download --

    #[test]
    fn test_format_download() {
        let v = json!({
            "count": 2,
            "files": [
                {"study_id": "ieu-a-2", "filename": "ieu-a-2.vcf.gz", "path": "/ieu-a-2/ieu-a-2.vcf.gz", "size": 1048576},
                {"study_id": "ieu-a-2", "filename": "ieu-a-2.vcf.gz.tbi", "path": "/ieu-a-2/ieu-a-2.vcf.gz.tbi", "size": 5120},
            ]
        });
        let result = format_download(&v);
        assert!(result.contains("**2** file(s)"));
        assert!(result.contains("1.0 MB"));
        assert!(result.contains("5.0 KB"));
    }

    // -- format_bytes --

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1048576), "1.0 MB");
        assert_eq!(format_bytes(1073741824), "1.0 GB");
    }
}
