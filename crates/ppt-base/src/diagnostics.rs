//! Aesthetics diagnostics: rule-based deck quality checks that run
//! automatically after every write.
//!
//! Two tiers, one report:
//!
//! - **Static** (Rust, always): rules over the parsed markdown that need
//!   no layout — empty slides, coarse text density.
//! - **Render** (headless Chromium, when a backend is available): the
//!   print page measures real DOM geometry per slide through
//!   `window.__deckDiagnose()` (see `apps/gui … render/diagnostics.ts` —
//!   the single render path also powers the diagnostics), reporting text
//!   overflowing the canvas, clipped content, tiny fonts, low contrast,
//!   … Render findings win over static ones with the same
//!   `(slide, rule)` key.
//!
//! Writes funnel through `save_markdown` / `save_theme_css`, which fire a
//! non-blocking notify into the diagnostics worker
//! ([`spawn_worker`]). The worker serializes runs and collapses bursts of
//! same-deck writes into one run against the latest state. Reports land
//! in `deck_diagnostics` (latest per deck) keyed by markdown revision +
//! theme revision, so readers can tell fresh from stale.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::shared::DeckShared;
use crate::store::DiagnosticsRow;

/// Wall-clock budget for one render-tier pass (browser + all slides).
const RENDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Static tier: flag a slide whose markdown exceeds this many chars
/// (render tier refines this with visible-text measurement).
const STATIC_DENSITY_CHARS: usize = 1600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "error" => Severity::Error,
            "info" => Severity::Info,
            _ => Severity::Warning,
        }
    }
}

/// One rule violation on one slide.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// 1-based slide number (raw index, same numbering as deck_slide_preview).
    pub slide: u32,
    /// Kebab rule id, e.g. `text-overflow-canvas`.
    pub rule: String,
    pub severity: Severity,
    /// Human/agent-readable description (Chinese, actionable).
    pub message: String,
    /// Optional machine-readable context from the rule (selector, px, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl Finding {
    fn summary(&self) -> String {
        format!("[{}] slide {} · {} · {}", self.severity.as_str(), self.slide, self.rule, self.message)
    }
}

/// A finished report.
pub type Report = DiagnosticsRow;

impl Report {
    pub fn counts(&self) -> (u64, u64, u64) {
        let errors = self.findings.iter().filter(|f| f.severity == Severity::Error).count() as u64;
        let warnings = self.findings.iter().filter(|f| f.severity == Severity::Warning).count() as u64;
        let infos = self.findings.iter().filter(|f| f.severity == Severity::Info).count() as u64;
        (errors, warnings, infos)
    }

    /// Whether the report describes exactly this markdown + theme state.
    pub fn is_fresh(&self, revision: &str, theme_revision: &str) -> bool {
        self.revision == revision && self.theme_revision == theme_revision
    }
}

// ---------------------------------------------------------------------------
// Static tier
// ---------------------------------------------------------------------------

/// Markdown-level rules; run on every report regardless of backend.
pub fn static_findings(deck: &crate::deck::Deck) -> Vec<Finding> {
    let mut out = Vec::new();
    for no in 1..=deck.len() {
        let Some(slide) = deck.slide(no) else { continue };
        let content = slide.content.trim();
        let has_image = slide.frontmatter.image.is_some() || content.contains("![");
        if content.is_empty() && !has_image {
            out.push(Finding {
                slide: no as u32,
                rule: "empty-slide".into(),
                severity: Severity::Info,
                message: format!("Slide {no} has no content."),
                details: None,
            });
        } else if content.chars().count() > STATIC_DENSITY_CHARS {
            out.push(Finding {
                slide: no as u32,
                rule: "text-density".into(),
                severity: Severity::Warning,
                message: format!(
                    "Slide {no} markdown is about {} characters long — too dense; split it across slides.",
                    content.chars().count()
                ),
                details: None,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Render tier
// ---------------------------------------------------------------------------

/// One `window.__deckDiagnose()` item as returned by the print page.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenderFinding {
    rule: String,
    severity: String,
    message: String,
    #[serde(default)]
    details: Option<Value>,
}

fn merge_render_findings(
    findings: &mut Vec<Finding>,
    rendered: &Value,
    targets: &[(u32, u32)],
) {
    let mut from_render = Vec::new();
    for (no, _) in targets {
        let Some(items) = rendered.get(no.to_string()).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let Ok(parsed) = serde_json::from_value::<RenderFinding>(item.clone()) else {
                continue;
            };
            from_render.push(Finding {
                slide: *no,
                rule: parsed.rule,
                severity: Severity::parse(&parsed.severity),
                message: parsed.message,
                details: parsed.details,
            });
        }
    }
    // Render-tier measurements supersede the coarser static duplicates.
    let keys: std::collections::HashSet<(u32, String)> = from_render
        .iter()
        .map(|f| (f.slide, f.rule.clone()))
        .collect();
    findings.retain(|f| !keys.contains(&(f.slide, f.rule.clone())));
    findings.extend(from_render);
}

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

/// Run both tiers for the deck's CURRENT state and store the report.
///
/// Heavy (may launch a browser); callers are the diagnostics worker or an
/// explicit agent/HTTP request — never the write actor's hot path.
pub async fn run_diagnostics(shared: &DeckShared, deck_id: &str) -> Result<Report> {
    let deck = shared.load_deck(deck_id).await?;
    let theme_css = shared.theme_css(deck_id).await?;
    let theme_revision = crate::shared::css_revision(&theme_css);
    let canvas = crate::theme::canvas_size(
        &theme_css,
        crate::theme::CanvasSize {
            width: deck.meta.canvas(),
            height: deck.meta.canvas_height(),
        },
    )?;

    let mut findings = static_findings(&deck);
    let mut rendered = false;

    // Render tier: every slide, fully revealed (layout after ALL clicks).
    let targets: Vec<(u32, u32)> = (1..=deck.len())
        .map(|no| {
            let total = deck
                .slide(no)
                .and_then(|s| s.click_plan.as_ref())
                .map(|p| p.total)
                .unwrap_or(0);
            (no as u32, total)
        })
        .collect();

    if !targets.is_empty() {
        match render_findings(shared, deck_id, &targets, canvas.width, canvas.height).await {
            Ok(value) => {
                merge_render_findings(&mut findings, &value, &targets);
                rendered = true;
            }
            Err(e) => {
                // Backend unavailable (NullExporter / no Chromium): the
                // report degrades to the static tier, flagged rendered=false.
                tracing::debug!(deck = %deck_id, error = %e, "render-tier diagnostics skipped");
            }
        }
    }

    findings.sort_by_key(|f| (f.slide, std::cmp::Reverse(f.severity as u8), f.rule.clone()));
    let report = DiagnosticsRow {
        deck_id: deck_id.to_string(),
        revision: deck.meta.revision.clone(),
        theme_revision,
        rendered,
        findings,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    shared.store.save_diagnostics(&report).await?;
    Ok(report)
}

/// Mint an ephemeral print token, drive the exporter's diagnose pass,
/// then delete the token row (the preview_slide_png pattern).
async fn render_findings(
    shared: &DeckShared,
    deck_id: &str,
    targets: &[(u32, u32)],
    canvas_width: u32,
    canvas_height: u32,
) -> Result<Value> {
    let raw_token = uuid::Uuid::new_v4().simple().to_string();
    let token_hash = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(raw_token.as_bytes());
        format!("{:x}", h.finalize())
    };
    let export_id = format!("diagnostics-{}", uuid::Uuid::new_v4().simple());
    let now = chrono::Utc::now().to_rfc3339();
    let record = crate::store::ExportRecord {
        id: export_id.clone(),
        deck_id: deck_id.to_string(),
        format: crate::store::ExportFormat::Png,
        status: crate::store::ExportStatus::Queued,
        params: serde_json::json!({
            "diagnostics": true,
            "slides": targets.len(),
            "canvasWidth": canvas_width,
            "canvasHeight": canvas_height,
        })
        .to_string(),
        print_token_hash: Some(token_hash),
        token_expires_at: Some(
            (chrono::Utc::now() + chrono::Duration::seconds(5 * 60)).to_rfc3339(),
        ),
        artifact_key: None,
        artifact_mime: None,
        artifact_size: None,
        error: None,
        created_at: now.clone(),
        updated_at: now,
    };
    shared.store.create_export(&record).await?;

    let job = crate::export::ExportJob {
        deck_id: deck_id.to_string(),
        export_id: export_id.clone(),
        format: crate::store::ExportFormat::Png,
        dpi: 96, // CSS-px measurements must match canvas coordinates
        pages: targets.iter().map(|(no, _)| *no as usize).collect(),
        canvas_width,
        canvas_height,
        notes: Vec::new(),
        print_token: raw_token,
    };
    let result = tokio::time::timeout(
        RENDER_TIMEOUT,
        shared.exporter.diagnose_slides(&job, targets),
    )
    .await
    .unwrap_or_else(|_| {
        Err(Error::Export("diagnostics render pass timed out".into()))
    });
    // The ephemeral token dies with the run either way.
    let _ = shared.store.delete_export(&export_id).await;
    result
}

/// Text summary for agent-facing results.
pub fn render_report_text(report: &Report) -> String {
    let (errors, warnings, infos) = report.counts();
    if report.findings.is_empty() {
        return format!(
            "diagnostics: no issues found ({})",
            if report.rendered { "including render-tier checks" } else { "static checks only" }
        );
    }
    let mut text = format!(
        "diagnostics: {} error / {} warning / {} info ({}):\n",
        errors,
        warnings,
        infos,
        if report.rendered { "including render-tier checks" } else { "static checks only" }
    );
    for finding in &report.findings {
        text.push_str("  ");
        text.push_str(&finding.summary());
        text.push('\n');
    }
    text
}

/// Context-injection payload attached to write-tool results (option A):
/// static-tier findings synchronously (microseconds, always accurate for
/// the just-saved state) + the render tier's scheduling status. The
/// render report itself lands in `deck_diagnostics` moments later — the
/// agent reads it via `deck_diagnose`.
pub fn write_hook(shared: &DeckShared, deck: &crate::deck::Deck) -> serde_json::Value {
    let findings = static_findings(deck);
    let render_tier = if shared.diagnostics_tx.get().is_some() {
        "scheduled"
    } else {
        "unavailable"
    };
    let (errors, warnings, infos) = (
        findings.iter().filter(|f| f.severity == Severity::Error).count(),
        findings.iter().filter(|f| f.severity == Severity::Warning).count(),
        findings.iter().filter(|f| f.severity == Severity::Info).count(),
    );
    serde_json::json!({
        "renderTier": render_tier,
        "staticErrors": errors,
        "staticWarnings": warnings,
        "staticInfos": infos,
        "staticFindings": findings,
        "note": match render_tier {
            "scheduled" => "layout diagnostics are running in the background; call deck_diagnose for the full report (text overflow, contrast, font size, …)",
            _ => "layout diagnostics unavailable in this environment; only static markdown checks ran",
        },
    })
}

// ---------------------------------------------------------------------------
// Post-write worker
// ---------------------------------------------------------------------------

/// Spawn the post-write diagnostics worker: serializes runs, collapses
/// bursts of same-deck writes into a single run against the latest
/// state. Returns the notify sender stored on [`DeckShared`].
pub fn spawn_worker(shared: Arc<DeckShared>) -> tokio::sync::mpsc::Sender<String> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
    tokio::spawn(async move {
        let mut backlog: Vec<String> = Vec::new();
        loop {
            // Wait for a deck when idle; resume the backlog otherwise.
            let deck_id = match backlog.pop() {
                Some(id) => id,
                None => match rx.recv().await {
                    Some(id) => id,
                    None => break,
                },
            };
            // Coalesce: absorb everything currently queued, dropping
            // duplicates of the deck we are about to run and keeping one
            // entry per other deck.
            while let Ok(next) = rx.try_recv() {
                if next != deck_id && !backlog.contains(&next) {
                    backlog.push(next);
                }
            }
            if let Err(e) = run_diagnostics(&shared, &deck_id).await {
                tracing::warn!(deck = %deck_id, error = %e, "deck diagnostics run failed");
            }
        }
        tracing::debug!("deck diagnostics worker stopped (channel closed)");
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck_with(markdown_slides: &[&str]) -> crate::deck::Deck {
        let markdown = markdown_slides.join("\n---\n\n");
        crate::deck::assemble(&markdown, "deck-test", "", "")
    }

    #[test]
    fn static_rules_flag_empty_and_dense_slides() {
        let dense = "很大的一段文字".repeat(400); // 2400 chars
        let deck = deck_with(&[
            "# 标题\n\n- 要点",           // healthy
            "",                            // empty
            &format!("# 密\n\n{dense}"),   // too long
        ]);
        let findings = static_findings(&deck);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(findings.iter().any(|f| f.slide == 2 && f.rule == "empty-slide" && f.severity == Severity::Info));
        assert!(findings.iter().any(|f| f.slide == 3 && f.rule == "text-density" && f.severity == Severity::Warning));
    }

    #[test]
    fn image_only_slides_are_not_empty() {
        let deck = deck_with(&["\n![figure](assets://abc)"]);
        assert!(static_findings(&deck).is_empty());
    }

    #[test]
    fn render_findings_merge_and_supersede_static_duplicates() {
        let mut findings = vec![Finding {
            slide: 2,
            rule: "text-density".into(),
            severity: Severity::Warning,
            message: "static".into(),
            details: None,
        }];
        let rendered = serde_json::json!({
            "2": [
                { "rule": "text-overflow-canvas", "severity": "error",
                  "message": "文字超出画布", "details": { "overflow": "right +24px" } },
                { "rule": "text-density", "severity": "warning", "message": "太密" }
            ],
            "3": [
                { "rule": "low-contrast", "severity": "warning", "message": "对比度 2.1" }
            ]
        });
        merge_render_findings(&mut findings, &rendered, &[(1, 0), (2, 0), (3, 0)]);
        // static text-density on slide 2 replaced by the render one
        assert_eq!(findings.iter().filter(|f| f.slide == 2 && f.rule == "text-density").count(), 1);
        assert!(findings.iter().any(|f| f.rule == "text-overflow-canvas" && f.severity == Severity::Error));
        assert!(findings.iter().any(|f| f.slide == 3 && f.rule == "low-contrast"));
        assert!(!findings.iter().any(|f| f.message == "static"));
    }
}
