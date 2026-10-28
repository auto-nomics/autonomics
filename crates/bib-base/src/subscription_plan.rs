//! Frozen source windows, separate from publication-date search filters.
use crate::subscriptions::*;
use bib_types::discovery::{DiscoveryRequest, SourceNotice, SourcePlan};
use sha2::{Digest, Sha256};
pub const DAY_MS: i64 = 86_400_000;

pub fn plan(
    request: &DiscoveryRequest,
    source: &str,
    window: Option<&SubscriptionWindow>,
) -> Result<SourcePlan> {
    let mut plan = crate::discovery_plan::plan(source, request)
        .map_err(|e| SubscriptionError::Invalid(e.message))?;
    let Some(window) = window else {
        return Ok(plan);
    };
    if window.from_ms < 0 || window.through_ms < window.from_ms {
        return Err(invalid("invalid closed source window"));
    }
    let from = chrono::DateTime::from_timestamp_millis(window.from_ms)
        .ok_or_else(|| invalid("window date outside supported range"))?;
    let through = chrono::DateTime::from_timestamp_millis(window.through_ms)
        .ok_or_else(|| invalid("window date outside supported range"))?;
    match (source, window.kind) {
        ("pubmed", SubscriptionWindowKind::PubmedCreatedOrRevised) => {
            if window.from_ms % DAY_MS != 0 || window.through_ms % DAY_MS != DAY_MS - 1 {
                return Err(invalid("PubMed windows must include complete UTC days"));
            }
            let original = plan
                .parameters
                .get("term")
                .ok_or_else(|| invalid("PubMed plan has no term"))?;
            let range = format!("{}:{}", from.format("%Y/%m/%d"), through.format("%Y/%m/%d"));
            plan.parameters.insert(
                "term".into(),
                format!("({original}) AND (({range}[crdt]) OR ({range}[lr]))"),
            );
            plan.notices.push(SourceNotice{code:"subscription_window".into(),message:"Closed UTC-day creation/revision window. PubMed LR covers completed citations; it is not a complete feed of every internal indexing change.".into()});
        }
        ("crossref", SubscriptionWindowKind::CrossrefIndexed) => {
            if window.from_ms % 1000 != 0 || window.through_ms % 1000 != 999 {
                return Err(invalid(
                    "Crossref windows must include complete UTC seconds",
                ));
            }
            if plan.parameters.contains_key("doi") {
                return Err(invalid(
                    "singleton DOI checks require bounded replay, not an update watermark",
                ));
            }
            let old = plan.parameters.get("filter").cloned().unwrap_or_default();
            if old.split(',').any(|f| {
                matches!(
                    f.split_once(':').map(|(key, _)| key),
                    Some("from-index-date" | "until-index-date")
                )
            }) {
                return Err(invalid(
                    "native index-date bounds require bounded replay; overlapping filter constraints cannot be silently widened",
                ));
            }
            let dates = format!(
                "from-index-date:{},until-index-date:{}",
                from.format("%Y-%m-%dT%H:%M:%S"),
                through.format("%Y-%m-%dT%H:%M:%S")
            );
            plan.parameters.insert(
                "filter".into(),
                if old.is_empty() {
                    dates
                } else {
                    format!("{old},{dates}")
                },
            );
            plan.parameters.insert("sort".into(), "indexed".into());
            plan.parameters.insert("order".into(), "asc".into());
            plan.notices.retain(|n| {
                !matches!(
                    n.code.as_str(),
                    "cursor_semantics"
                        | "approximate_text"
                        | "no_text_relevance"
                        | "publication_offset_window"
                )
            });
            plan.notices
                .extend(crate::discovery_crossref::notices(&plan.parameters));
            plan.notices.push(SourceNotice{code:"subscription_scan_order".into(),message:"Subscription traversal uses index-date ascending within this closed update window. The saved publication/relevance sort remains in the rule. Crossref cursor results can change; overlap and local seen identities remain necessary.".into()});
        }
        _ => return Err(invalid("source does not support this update window")),
    }
    let mut h = Sha256::new();
    h.update(source);
    h.update(serde_json::to_vec(&plan.parameters)?);
    h.update(plan.page_size.to_le_bytes());
    plan.fingerprint = format!("{:x}", h.finalize());
    Ok(plan)
}
