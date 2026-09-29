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
    /// Proposals that revise an already-installed auto skill.
    pub updated_existing: Vec<String>,
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

/// Render an update proposal: the current SKILL.md (frontmatter and
/// human edits preserved verbatim) plus one appended section holding
/// only the NEW observations.
pub fn render_update_md(current: &str, fresh: &[&Observation]) -> String {
    let mut out = String::from(current.trim_end());
    out.push_str(&format!(
        "\n\n## Update: {} new observation(s)\n\n",
        fresh.len()
    ));
    for observation in fresh {
        out.push_str(&format!(
            "- **{}** — {}\n",
            observation.summary,
            observation.body.trim()
        ));
    }
    out.push_str(
        "\nArchived revision of the previous version sits in this skill's \
         `.history/`.\n",
    );
    out
}

/// One full distillation pass: cluster → filter consumed → write
/// proposals (create or update). Pure with respect to the stores —
/// callers pass the observation list, the proposal area, and a
/// registry view for existing-skill resolution.
pub fn distill(
    observations: &[Observation],
    proposals: &Proposals,
    registry: &crate::registry::SkillRegistry,
) -> Result<DistillReport, SkillError> {
    let mut report = DistillReport::default();
    let processed = proposals.processed_clusters();
    let installed = registry.list();
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
        // Deterministic create-vs-update resolution — no LLM judgment
        // anywhere. The derived name either matches an installed skill
        // or it does not.
        let existing = installed.iter().find(|e| e.meta.name == name);
        if let Some(entry) = existing
            && let Some(dir) = entry.dir()
        {
            if !entry.meta.tags.iter().any(|t| t == "auto") {
                // Human-owned name: the loop never overwrites a human
                // decision. Consume the cluster — re-surfacing it every
                // pass would only add noise.
                proposals.mark_processed(&hash)?;
                report.skipped.push((
                    hash,
                    format!(
                        "name {name:?} belongs to a human-installed skill \
                         (no auto tag); update it manually"
                    ),
                ));
                continue;
            }
            // Update path: only NEW evidence (not covered by the last
            // manifest for this name) justifies a revision, and the
            // same MIN_CLUSTER bar applies to the delta as to a fresh
            // skill.
            let covered = proposals.covered_observation_ids(&name);
            let fresh: Vec<&Observation> = candidate
                .observations
                .iter()
                .filter(|o| !covered.contains(&o.id))
                .collect();
            if fresh.len() < MIN_CLUSTER {
                report.skipped.push((
                    hash,
                    format!(
                        "only {} new observation(s) for {name:?}; need {MIN_CLUSTER}",
                        fresh.len()
                    ),
                ));
                continue;
            }
            let current = std::fs::read_to_string(dir.join("SKILL.md")).map_err(|e| {
                SkillError::Unreadable {
                    path: dir.join("SKILL.md"),
                    reason: e.to_string(),
                }
            })?;
            let dir_proposal = proposals.root().join(&name);
            std::fs::create_dir_all(&dir_proposal)?;
            std::fs::write(
                dir_proposal.join("SKILL.md"),
                render_update_md(&current, &fresh),
            )?;
            let rationale = format!(
                "update: {} new observation(s) on `{}`{}",
                fresh.len(),
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
                // The manifest records the FULL cluster membership so
                // the next delta is measured against this update.
                candidate
                    .observations
                    .iter()
                    .map(|o| o.id.clone())
                    .collect(),
                true,
            )?;
            report.proposals_written.push(name.clone());
            report.updated_existing.push(name);
            continue;
        }
        // Create path: a proposal whose target name already has a
        // pending proposal is skipped and consumed — forcing it would
        // double a review already in flight.
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
            false,
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

        let registry = crate::registry::SkillRegistry::empty().with_root(
            crate::registry::SkillTier::Global,
            tmp.path().join("skills"),
        );
        let report = distill(&store.list(), &proposals, &registry).unwrap();
        assert_eq!(report.proposals_written.len(), 1);
        let name = &report.proposals_written[0];
        assert!(proposals.root().join(name).join("SKILL.md").is_file());

        // The written proposal passes the strict contract.
        assert!(proposals.validate_dir(name).is_empty());

        // Second pass: nothing new — the pending proposal blocks its
        // own cluster (listed as a proposal with that name).
        let again = distill(&store.list(), &proposals, &registry).unwrap();
        assert!(again.proposals_written.is_empty());
        assert_eq!(again.skipped.len(), 1);
    }
}

#[cfg(test)]
mod update_tests {
    use super::*;
    use crate::observation::{
        ObservationInput, ObservationKind, ObservationSource, ObservationStore,
    };
    use crate::registry::{SkillRegistry, SkillTier};

    fn observe(store: &ObservationStore, error: &str, body: &str) {
        store
            .record(ObservationInput {
                kind: ObservationKind::Failure,
                source: ObservationSource::Agent,
                summary: format!("fix for {error}"),
                body: body.into(),
                node_kind: Some("sql".into()),
                error: Some(error.into()),
            })
            .unwrap();
    }

    /// The full evolution arc of one pattern: three observations
    /// create the skill, three more revise it, and the revision is
    /// an overlay with the previous version archived.
    #[test]
    fn create_then_update_full_arc() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(tmp.path());
        let proposals = Proposals::open(tmp.path());
        let registry =
            SkillRegistry::empty().with_root(SkillTier::Global, tmp.path().join("skills"));
        let global = tmp.path().join("skills");

        // Round 1: three observations → create proposal → approve.
        observe(&store, "syntax error near N", "fix one");
        observe(&store, "syntax error near N", "fix two");
        observe(&store, "syntax error near N", "fix three");
        let report = distill(&store.list(), &proposals, &registry).unwrap();
        assert_eq!(report.proposals_written.len(), 1);
        let name = report.proposals_written[0].clone();

        proposals.approve(&name, &global).unwrap();
        let skill_md = global.join(&name).join("SKILL.md");
        let content = std::fs::read_to_string(&skill_md).unwrap();
        assert!(content.contains("fix one"));
        assert!(content.contains("fix three"));

        // Round 2: three NEW observations on the same anchor → an
        // update proposal whose body appends only the new evidence.
        observe(&store, "syntax error near N", "fix four");
        observe(&store, "syntax error near N", "fix five");
        observe(&store, "syntax error near N", "fix six");
        let updated = distill(&store.list(), &proposals, &registry).unwrap();
        assert_eq!(updated.updated_existing, vec![name.clone()]);
        assert!(updated.proposals_written.contains(&name));

        let proposal = proposals.find(&name).unwrap();
        assert!(proposal.update);
        assert_eq!(proposal.status, crate::ProposalStatus::Pending);

        // Approve the update: overlay lands, previous version archived.
        proposals.approve(&name, &global).unwrap();
        let content = std::fs::read_to_string(&skill_md).unwrap();
        assert!(content.contains("fix one"));
        assert!(content.contains("fix six"));
        assert!(content.contains("## Update: 3 new observation(s)"));
        let history = global.join(&name).join(".history");
        assert!(history.read_dir().expect("history dir").count() >= 1);

        // Round 3: only two new observations — below the delta
        // threshold, no proposal.
        observe(&store, "syntax error near N", "fix seven");
        observe(&store, "syntax error near N", "fix eight");
        let quiet = distill(&store.list(), &proposals, &registry).unwrap();
        assert!(quiet.proposals_written.is_empty());
    }

    /// A human-installed skill without the `auto` tag is never
    /// touched by the loop, and its cluster is consumed so it does
    /// not resurface every pass.
    #[test]
    fn human_owned_names_are_protected() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("skills");
        let human = global.join("sql-syntax-error-near-n");
        std::fs::create_dir_all(&human).unwrap();
        std::fs::write(
            human.join("SKILL.md"),
            "---\nname: sql-syntax-error-near-n\ndescription: Hand-written.\ntags: [handmade]\n---\nprecious hand-written content\n",
        )
        .unwrap();

        let store = ObservationStore::open(tmp.path());
        for i in 0..4 {
            observe(&store, "syntax error near N", &format!("fix {i}"));
        }
        let proposals = Proposals::open(tmp.path());
        let registry = SkillRegistry::empty().with_root(SkillTier::Global, global.clone());

        let report = distill(&store.list(), &proposals, &registry).unwrap();
        assert!(report.proposals_written.is_empty());
        assert!(
            report
                .skipped
                .iter()
                .any(|(_, reason)| reason.contains("human-installed"))
        );
        // Consumed: a second pass considers the cluster but skips it
        // as already consumed — no proposal, no noise.
        let again = distill(&store.list(), &proposals, &registry).unwrap();
        assert!(again.proposals_written.is_empty());
        assert!(
            again
                .skipped
                .iter()
                .any(|(_, reason)| reason.contains("already consumed"))
        );
        // And the hand-written content is intact.
        assert!(
            std::fs::read_to_string(human.join("SKILL.md"))
                .unwrap()
                .contains("precious hand-written content")
        );
    }
}
