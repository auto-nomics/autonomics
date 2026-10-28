//! Export the frozen capture, without hydrating IDs from the current library.
use crate::search_snapshots::{
    SnapshotData, SnapshotError, SnapshotKind, StoredSnapshot, bounded_json, hash,
};
use bib_types::Article;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotExportFormat {
    Json,
    Ris,
    Bibtex,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotExport {
    pub file_name: String,
    pub content_type: String,
    pub content: String,
    pub articles: usize,
    pub result_basis: String,
}
type Result<T> = std::result::Result<T, SnapshotError>;
impl StoredSnapshot {
    pub fn export(
        &self,
        format: SnapshotExportFormat,
        max_output_bytes: usize,
    ) -> Result<SnapshotExport> {
        if !(1..=128 * 1024 * 1024).contains(&max_output_bytes) {
            return Err(SnapshotError::Invalid(
                "export budget outside 1..128MiB".into(),
            ));
        }
        if uuid::Uuid::parse_str(&self.summary.id).is_err() {
            return Err(SnapshotError::Corrupt(
                "invalid snapshot download identity".into(),
            ));
        }
        let payload = bounded_json(&self.capture, 64 * 1024 * 1024)?;
        let kind = match self.capture.data {
            SnapshotData::Library { .. } => SnapshotKind::Library,
            SnapshotData::Discovery { .. } => SnapshotKind::Discovery,
        };
        if hash(&payload) != self.summary.checksum
            || payload.len() != self.summary.payload_bytes
            || self.capture.coverage()? != self.summary.coverage
            || kind != self.summary.kind
        {
            return Err(SnapshotError::Corrupt(
                "export differs from frozen capture".into(),
            ));
        }
        let articles = self.capture.articles();
        let (extension, content_type, content) = match format {
            SnapshotExportFormat::Json => {
                let bytes = bounded_json(self, max_output_bytes)?;
                let text = String::from_utf8(bytes)
                    .map_err(|_| SnapshotError::Corrupt("non-UTF8 serialized capture".into()))?;
                ("json", "application/json; charset=utf-8", text)
            }
            SnapshotExportFormat::Ris => {
                let mut output = String::new();
                for article in &articles {
                    if !output.is_empty() {
                        append(&mut output, "\n\n", max_output_bytes)?;
                    }
                    append(
                        &mut output,
                        &crate::to_ris(&line_safe(article)?),
                        max_output_bytes,
                    )?;
                }
                ("ris", "text/plain; charset=utf-8", output)
            }
            SnapshotExportFormat::Bibtex => {
                let mut output = String::new();
                let mut counts = HashMap::<String, usize>::new();
                let mut used = HashSet::new();
                for article in &articles {
                    let base = crate::cite_key(article)
                        .chars()
                        .take(96)
                        .collect::<String>();
                    let counter = counts.entry(base.clone()).or_default();
                    let key = loop {
                        *counter += 1;
                        let candidate = if *counter == 1 {
                            base.clone()
                        } else {
                            format!("{base}_{counter}")
                        };
                        if used.insert(candidate.clone()) {
                            break candidate;
                        }
                    };
                    let rendered = crate::to_bibtex(&line_safe(article)?);
                    let (_, body) = rendered.split_once('\n').ok_or_else(|| {
                        SnapshotError::Corrupt("BibTeX renderer omitted its record header".into())
                    })?;
                    if !output.is_empty() {
                        append(&mut output, "\n\n", max_output_bytes)?;
                    }
                    append(
                        &mut output,
                        &format!("@article{{{key},\n{body}"),
                        max_output_bytes,
                    )?;
                }
                ("bib", "text/plain; charset=utf-8", output)
            }
        };
        Ok(SnapshotExport {
            file_name: format!("search-snapshot-{}.{extension}", self.summary.id),
            content_type: content_type.into(),
            content,
            articles: articles.len(),
            result_basis: self.summary.coverage.result_basis.clone(),
        })
    }
}
fn append(output: &mut String, value: &str, max: usize) -> Result<()> {
    if value.len() > max.saturating_sub(output.len()) {
        return Err(SnapshotError::Budget(
            "export output exceeds configured bytes".into(),
        ));
    }
    output.push_str(value);
    Ok(())
}
fn line_safe(article: &Article) -> Result<Article> {
    // Exchange-format fields are single lines. Preserve original newlines and
    // controls in the JSON capture, but prevent extra RIS records/field lines.
    fn clean(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(s) => {
                *s = s
                    .chars()
                    .map(|ch| {
                        if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') {
                            ' '
                        } else {
                            ch
                        }
                    })
                    .collect()
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    clean(value)
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values_mut() {
                    clean(value)
                }
            }
            _ => {}
        }
    }
    let mut value = serde_json::to_value(article)?;
    clean(&mut value);
    Ok(serde_json::from_value(value)?)
}
