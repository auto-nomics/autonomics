//! Prompt formatting helpers for conversation compaction.
//!
//! The actual compaction prompt is built by [`build_compaction_prompt`] in
//! `session.rs`. This module provides post-processing helpers to clean up
//! the LLM's raw summary output.

/// Extract and format the summary from the LLM's compaction response.
///
/// Strips `<analysis>` blocks, extracts `<summary>` content (or falls back
/// to the raw text), and collapses excessive blank lines.
pub fn format_compact_summary(summary: &str) -> String {
    let mut formatted = summary.to_string();

    // Remove <analysis> blocks — they're thinking scaffolding, not part of
    // the handoff summary.
    let re = regex::Regex::new(r"<analysis>[\s\S]*?</analysis>").unwrap();
    formatted = re.replace(&formatted, "").to_string();

    // Extract <summary> content if present.
    if let Some(caps) = regex::Regex::new(r"<summary>([\s\S]*?)</summary>")
        .unwrap()
        .captures(&formatted)
    {
        let content = caps.get(1).map_or("", |m| m.as_str()).trim();
        formatted = regex::Regex::new(r"<summary>[\s\S]*?</summary>")
            .unwrap()
            .replace(&formatted, &format!("Summary:\n{content}"))
            .to_string();
    }

    // Collapse runs of blank lines.
    let re = regex::Regex::new(r"\n\n+").unwrap();
    formatted = re.replace_all(&formatted, "\n\n").to_string();
    formatted.trim().to_string()
}
