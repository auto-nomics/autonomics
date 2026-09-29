//! Deterministic distillation: observations → skill proposals.
//!
//! Where EvoScientist spends an auxiliary LLM per turn to distill
//! observations and another to synthesize skills, this loop is pure
//! code end to end:
//!
//! 1. **Cluster** observations by their structural anchor —
//!    `(node_kind, error signature)` — not by text similarity. Only
//!    anchored observations participate; free-form notes stay
//!    searchable but never auto-propose.
//! 2. **Threshold**: a cluster needs `MIN_CLUSTER` anchored
//!    observations before it is worth a skill — the same
//!    "repeated pattern" bar EvoScientist applies, evaluated
//!    deterministically.
//! 3. **Consume exactly once**: clusters already covered by a
//!    proposal or a processed marker are skipped — the loop is
//!    idempotent by construction (stable `cluster_hash`).
//! 4. **Synthesize a skeleton**: a draft SKILL.md assembled from the
//!    cluster's own summaries and bodies. It is conservative —
//!    "here are the recorded fixes" — never invented advice. A later
//!    LLM pass may rewrite it; the proposal format and the approval
//!    gate do not care who wrote the draft.
//!
//! The generated name derives from the anchor, so the same pattern
//! yields the same skill name across runs — stable, greppable, and
//! collision-checkable against the installed library.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::error::SkillError;
use crate::format;
use crate::observation::{Observation, ObservationKind};
use crate::proposals::{Proposals, cluster_hash};

/// Anchored observations required before a cluster proposes.
pub const MIN_CLUSTER: usize = 3;

/// One candidate cluster surfaced by [`distill`].
#[derive(Debug, Clone)]
pub struct Candidate {
    pub node_kind: String,
    pub error_signature: String,
    pub observations: Vec<Observation>,
}

impl Candidate {
    /// The stable idempotency key over the member observation ids.
    pub fn hash(&self) -> String {
        let ids: BTreeSet<String> = self.observations.iter().map(|o| o.id.clone()).collect();
        cluster_hash(&ids)
    }

    /// The skill name this cluster distills to.
    pub fn skill_name(&self) -> Option<String> {
        let error_slug = slug(&self.error_signature);
        let kind_slug = slug(&self.node_kind);
        if kind_slug.is_empty() {
            return None;
        }
        let base = if error_slug.is_empty() {
            format!("{kind_slug}-fixes")
        } else {
            format!("{kind_slug}-{error_slug}")
        };
        // Leave room for nothing: names cap at 64 chars by contract.
        format::sanitize_name(&base[..base.len().min(60)])
    }
}

/// Reduce a slug: lowercase, alphanumerics and dashes, collapsed.
fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in text.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(lower);
        } else if !out.is_empty() {
            pending_dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Outcome of one distillation pass.
#[derive(Debug, Default)]
pub struct DistillReport {
    pub candidates_considered: usize,
    pub proposals_written: Vec<String>,
    /// (cluster hash, reason) — skipped clusters.
    pub skipped: Vec<(String, String)>,
}

impl DistillReport {
    pub fn wrote_anything(&self) -> bool {
        !self.proposals_written.is_empty()
    }
}

/// Group anchored observations into candidate clusters meeting the
/// threshold, in a deterministic order.
pub fn candidates(observations: &[Observation]) -> Vec<Candidate> {
    let mut groups: BTreeMap<(String, String), Vec<Observation>> = BTreeMap::new();
    for observation in observations {
        // Only failure-kind observations carry a reliable anchor; a
        // recipe without a node kind has nothing to cluster on.
        let Some(kind) = observation.node_kind.as_deref().map(str::to_string) else {
            continue;
        };
        if observation.kind != ObservationKind::Failure {
            continue;
        }
        let error = observation
            .error
            .as_deref()
            .map(signature)
            .unwrap_or_default();
        groups
            .entry((kind, error))
            .or_default()
            .push(observation.clone());
    }
    groups
        .into_iter()
        .filter(|(_, members)| members.len() >= MIN_CLUSTER)
        .map(|((node_kind, error_signature), observations)| Candidate {
            node_kind,
            error_signature,
            observations,
        })
        .collect()
}

/// Normalize an error text into a stable signature: first line,
/// whitespace-collapsed, numbers generalized so "column 3" and
/// "column 7" cluster together.
fn signature(error: &str) -> String {
    let first_line = error.lines().next().unwrap_or("");
    let collapsed: String = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(collapsed.len());
    let mut in_number = false;
    for ch in collapsed.chars() {
        if ch.is_ascii_digit() {
            if !in_number {
                out.push('N');
                in_number = true;
            }
        } else {
            in_number = false;
            out.push(ch);
        }
    }
    out.trim().to_string()
}

/// Render the skeleton SKILL.md for a candidate.
///
/// Conservative by design: it presents what was observed, not advice
/// the loop has no basis for. The approval gate (a human, for now)
/// decides whether the pattern deserves promotion.
pub fn render_skill_md(candidate: &Candidate) -> String {
    let mut out = String::new();
    let error_line = if candidate.error_signature.is_empty() {
        String::new()
    } else {
        format!(" failing with `{}`", candidate.error_signature)
    };
    out.push_str(&format!(
        "---\nname: {}\ndescription: Fixes recorded for the `{}` node kind{}. \
         Consult before debugging a similar failure.\ntags: [auto, {}]\n---\n\n",
        candidate.skill_name().unwrap_or_default(),
        candidate.node_kind,
        error_line,
        slug(&candidate.node_kind),
    ));
    out.push_str(&format!(
        "# Auto-distilled: `{}`{}\n\n",
        candidate.node_kind, error_line
    ));
    out.push_str(&format!(
        "Distilled from {} recorded observation(s). Each entry below was \
         learned during real work; none of it is invented.\n\n",
        candidate.observations.len()
    ));
    out.push_str("## Recorded fixes\n\n");
    for observation in &candidate.observations {
        out.push_str(&format!("### {}\n\n", observation.summary));
        let body = observation.body.trim();
        out.push_str(body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(
        "## Caveats\n\n- This skill was generated deterministically from \
         observation records. Verify each fix still applies before use, and \
         edit or uninstall it if the underlying node behavior has changed.\n",
    );
    out
}

/// One full distillation pass: cluster → filter consumed → write
/// proposals. Pure with respect to the stores — callers pass the
/// observation list and the proposal area.
pub fn distill(
    observations: &[Observation],
    proposals: &Proposals,
) -> Result<DistillReport, SkillError> {
    let mut report = DistillReport::default();
    let processed = proposals.processed_clusters();
    for candidate in candidates(observations) {
        report.candidates_considered += 1;
        let hash = candidate.hash();
        if processed.contains(&hash) {
            report
                .skipped
                .push((hash, "cluster already consumed".into()));
            continue;
        }
        let Some(name) = candidate.skill_name() else {
            proposals.mark_processed(&hash)?;
            report
                .skipped
                .push((hash, "no valid name derivable".into()));
            continue;
        };
        // A proposal whose target name collides with an installed
        // skill or an existing proposal is skipped and consumed —
        // forcing it would shadow or double a human decision.
        if proposals.meta(&name).is_some() {
            proposals.mark_processed(&hash)?;
            report
                .skipped
                .push((hash, format!("name {name:?} already proposed")));
            continue;
        }
        let dir = proposals.root().join(&name);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("SKILL.md"), render_skill_md(&candidate))?;
        let rationale = format!(
            "{} anchored observation(s) on `{}`{}",
            candidate.observations.len(),
            candidate.node_kind,
            if candidate.error_signature.is_empty() {
                String::new()
            } else {
                format!(" / `{}`", candidate.error_signature)
            }
        );
        proposals.submit(
            &name,
            &hash,
            &rationale,
            candidate
                .observations
                .iter()
                .map(|o| o.id.clone())
                .collect(),
        )?;
        report.proposals_written.push(name);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{
        ObservationInput, ObservationKind, ObservationSource, ObservationStore,
    };

    fn observe(store: &ObservationStore, error: &str, body: &str) {
        store
            .record(ObservationInput {
                kind: ObservationKind::Failure,
                source: ObservationSource::Agent,
                summary: format!("fix for {error}"),
                body: body.into(),
                node_kind: Some("file_to_dataframe".into()),
                error: Some(error.into()),
            })
            .unwrap();
    }

    #[test]
    fn clusters_by_anchor_with_numeric_generalization() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(tmp.path());
        observe(&store, "column 3 has bad type", "cast to f64");
        observe(&store, "column 7 has bad type", "cast to f64");
        observe(&store, "column 9 has bad type", "cast to f64 or i64");
        observe(&store, "unrelated error text", "other fix");

        let candidates = candidates(&store.list());
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.node_kind, "file_to_dataframe");
        assert_eq!(candidate.observations.len(), 3);
        let name = candidate.skill_name().unwrap();
        assert_eq!(name, "file-to-dataframe-column-n-has-bad-type");
    }

    #[test]
    fn below_threshold_never_proposes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(tmp.path());
        observe(&store, "e1", "f1");
        observe(&store, "e1", "f2");
        assert!(candidates(&store.list()).is_empty());
    }

    #[test]
    fn unanchored_observations_do_not_participate() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(tmp.path());
        for i in 0..5 {
            store
                .record(ObservationInput {
                    kind: ObservationKind::Recipe,
                    source: ObservationSource::Agent,
                    summary: format!("note {i}"),
                    body: "no anchor".into(),
                    node_kind: None,
                    error: None,
                })
                .unwrap();
        }
        assert!(candidates(&store.list()).is_empty());
    }

    #[test]
    fn distill_writes_then_consumes_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(tmp.path());
        observe(&store, "boom at step N", "restart with clean state");
        observe(&store, "boom at step N", "restart and bump timeout");
        observe(&store, "boom at step N", "restart, bump timeout, verify");
        let proposals = Proposals::open(tmp.path());

        let report = distill(&store.list(), &proposals).unwrap();
        assert_eq!(report.proposals_written.len(), 1);
        let name = &report.proposals_written[0];
        assert!(proposals.root().join(name).join("SKILL.md").is_file());

        // The written proposal passes the strict contract.
        assert!(proposals.validate_dir(name).is_empty());

        // Second pass: nothing new — the pending proposal blocks its
        // own cluster (listed as a proposal with that name).
        let again = distill(&store.list(), &proposals).unwrap();
        assert!(again.proposals_written.is_empty());
        assert_eq!(again.skipped.len(), 1);
    }
}
