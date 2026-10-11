//! Deterministic distillation: observation clusters → skill proposals.
//!
//! Where EvoScientist spends an auxiliary LLM per turn to distill
//! observations and another to synthesize skills, this loop is pure
//! code end to end. The observation-domain half — semantic
//! clustering over record-time vectors — lives in
//! [`evolution_core::cluster`]; this module owns only the skill
//! half:
//!
//! 1. **Consume exactly once**: clusters already covered by a
//!    proposal or a processed marker are skipped — the loop is
//!    idempotent by construction (stable `cluster_hash`).
//! 2. **Synthesize a skeleton**: a draft SKILL.md assembled from the
//!    cluster's own summaries and bodies. It is conservative —
//!    "here are the recorded fixes" — never invented advice. A later
//!    LLM pass may rewrite it; the proposal format and the approval
//!    gate do not care who wrote the draft.
//!
//! The generated name derives from the anchor, so the same pattern
//! yields the same skill name across runs — stable, greppable, and
//! collision-checkable against the installed library.

use std::collections::HashMap;

use crate::error::SkillError;
use crate::format;
use crate::proposals::Proposals;
use evolution_core::cluster::{Candidate, MIN_CLUSTER, candidates};
use evolution_core::observation::{Observation, ObservationKind};

/// The skill name a cluster distills to. Failure names keep the bare
/// `<node>-<signature>` form; recipe and caveat names carry a kind
/// suffix so the three namespaces never collide.
pub fn skill_name(candidate: &Candidate) -> Option<String> {
    let sig_slug = slug(&candidate.signature);
    let kind_slug = slug(&candidate.node_kind);
    if kind_slug.is_empty() {
        return None;
    }
    let stem = if sig_slug.is_empty() {
        match candidate.kind {
            ObservationKind::Failure => format!("{kind_slug}-fixes"),
            ObservationKind::Recipe | ObservationKind::Caveat => kind_slug.clone(),
        }
    } else {
        format!("{kind_slug}-{sig_slug}")
    };
    let capped = match candidate.kind {
        ObservationKind::Failure => stem[..stem.len().min(60)].to_string(),
        // Truncate the stem BEFORE appending the kind suffix so
        // the suffix always survives the 64-char name cap — that
        // is what keeps the failure/recipe/caveat namespaces
        // disjoint on long signatures.
        ObservationKind::Recipe => format!("{}-recipe", &stem[..stem.len().min(52)]),
        ObservationKind::Caveat => format!("{}-caveat", &stem[..stem.len().min(52)]),
    };
    format::sanitize_name(&capped)
}

#[cfg(test)]
mod naming_tests {
    use super::*;

    /// The kind suffix is not decoration: without it, a failure whose
    /// signature ends in `recipe` could collide with a recipe skill.
    #[tokio::test]
    async fn kind_suffixes_keep_skill_namespaces_disjoint() {
        let failure = Candidate {
            kind: ObservationKind::Failure,
            node_kind: "sql".into(),
            signature: "recipe".into(),
            observations: Vec::new(),
        };
        let recipe = Candidate {
            kind: ObservationKind::Recipe,
            node_kind: "sql".into(),
            signature: "recipe".into(),
            observations: Vec::new(),
        };

        assert_eq!(skill_name(&failure).unwrap(), "sql-recipe");
        assert_eq!(skill_name(&recipe).unwrap(), "sql-recipe-recipe");
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

/// Defuse a signature for embedding in the YAML plain-scalar
/// description: `": "` would terminate the value and `" #"` would
/// start a comment. The body keeps the untouched text.
fn yaml_safe(text: &str) -> String {
    text.replace(": ", ":").replace(" #", "#")
}

/// Render the skeleton SKILL.md for a candidate.
///
/// Conservative by design: it presents what was observed, not advice
/// the loop has no basis for. The approval gate (a human, for now)
/// decides whether the pattern deserves promotion.
pub fn render_skill_md(candidate: &Candidate) -> String {
    // Per-kind profile: capitalized noun for the description, plain
    // noun for the entries heading, how the signature is introduced,
    // the description tail, and the kind tag after `auto`.
    let (cap_noun, noun, sig_prefix, tail, kind_tag) = match candidate.kind {
        ObservationKind::Failure => (
            "Fixes",
            "fixes",
            " failing with",
            "Consult before debugging a similar failure.",
            "",
        ),
        ObservationKind::Recipe => (
            "Recipes",
            "recipes",
            " for",
            "Consult before improvising a similar procedure.",
            ", recipe",
        ),
        ObservationKind::Caveat => (
            "Caveats",
            "caveats",
            " on",
            "Consult before applying the usual approach here.",
            ", caveat",
        ),
    };
    let sig_line = if candidate.signature.is_empty() {
        String::new()
    } else {
        format!("{sig_prefix} `{}`", candidate.signature)
    };
    let mut out = String::new();
    out.push_str(&format!(
        "---\nname: {}\ndescription: {} recorded for the `{}` node kind{}. \
         {}\ntags: [auto{}, {}]\n---\n\n",
        skill_name(candidate).unwrap_or_default(),
        cap_noun,
        candidate.node_kind,
        yaml_safe(&sig_line),
        tail,
        kind_tag,
        slug(&candidate.node_kind),
    ));
    out.push_str(&format!(
        "# Auto-distilled: `{}`{}\n\n",
        candidate.node_kind, sig_line
    ));
    out.push_str(&format!(
        "Distilled from {} recorded {} observation(s). Each entry below was \
         learned during real work; none of it is invented.\n\n",
        candidate.observations.len(),
        noun,
    ));
    out.push_str(&format!("## Recorded {noun}\n\n"));
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
        "## Notes on auto-generation\n\n- This skill was generated \
         deterministically from observation records. Verify each entry \
         still applies before use, and edit or uninstall it if the \
         underlying node behavior has changed.\n",
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
/// callers pass the observation list, the record-time vectors, the
/// proposal area, and a registry view for existing-skill resolution.
pub fn distill(
    observations: &[Observation],
    embeddings: &HashMap<String, Vec<f32>>,
    proposals: &Proposals,
    registry: &crate::registry::SkillRegistry,
) -> Result<DistillReport, SkillError> {
    let mut report = DistillReport::default();
    let processed = proposals.processed_clusters();
    let installed = registry.list();
    for candidate in candidates(observations, embeddings)? {
        report.candidates_considered += 1;
        let hash = candidate.hash();
        if processed.contains(&hash) {
            report
                .skipped
                .push((hash, "cluster already consumed".into()));
            continue;
        }
        let Some(name) = skill_name(&candidate) else {
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
                "update: {} new {} observation(s) on `{}`{}",
                fresh.len(),
                candidate.kind.label(),
                candidate.node_kind,
                if candidate.signature.is_empty() {
                    String::new()
                } else {
                    format!(" / `{}`", candidate.signature)
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
            "{} anchored {} observation(s) on `{}`{}",
            candidate.observations.len(),
            candidate.kind.label(),
            candidate.node_kind,
            if candidate.signature.is_empty() {
                String::new()
            } else {
                format!(" / `{}`", candidate.signature)
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
mod testing {
    //! Shared plumbing: every distillation test records through a real
    //! hub so observations carry the vectors the clustering reads.
    use super::*;
    use evolution_core::ObservationHub;
    use evolution_core::storage::ObservationStorage;

    /// The record-time vectors for these observations.
    pub(crate) async fn embeddings(
        hub: &ObservationHub,
        observations: &[Observation],
    ) -> HashMap<String, Vec<f32>> {
        let ids = observations
            .iter()
            .map(|o| o.id.clone())
            .collect::<Vec<_>>();
        hub.load_embeddings(&ids).await.unwrap()
    }

    /// One full distillation pass over everything the hub holds.
    pub(crate) async fn distill_all(
        hub: &ObservationHub,
        proposals: &Proposals,
        registry: &crate::registry::SkillRegistry,
    ) -> DistillReport {
        let observations = hub.storage().list().await;
        let embeddings = embeddings(hub, &observations).await;
        distill(&observations, &embeddings, proposals, registry).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distill::testing::distill_all;
    use evolution_core::ObservationHub;
    use evolution_core::observation::{ObservationInput, ObservationKind, ObservationSource};

    async fn observe(hub: &ObservationHub, error: &str, body: &str) {
        hub.record(ObservationInput {
            kind: ObservationKind::Failure,
            source: ObservationSource::Agent,
            summary: format!("fix for {error}"),
            body: body.into(),
            node_kind: Some("file_to_dataframe".into()),
            error: Some(error.into()),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn coloned_signatures_still_render_valid_frontmatter() {
        // A failure error text containing ": " (plain-scalar
        // terminator) and " #" (comment starter) must not break the
        // generated SKILL.md frontmatter. (Recipes and caveats no
        // longer embed a signature at all — they cluster per node.)
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        for i in 1..=3 {
            observe(
                &hub,
                &format!("note: cast check #{i} failed for port 0"),
                &format!("body {i}"),
            )
            .await;
        }
        let proposals = Proposals::open(tmp.path());
        let registry = crate::registry::SkillRegistry::empty().with_root(
            crate::registry::SkillTier::Global,
            tmp.path().join("skills"),
        );
        let report = distill_all(&hub, &proposals, &registry).await;
        assert_eq!(report.proposals_written.len(), 1);
        let name = &report.proposals_written[0];
        assert!(proposals.validate_dir(name).is_empty());
    }

    #[tokio::test]
    async fn distill_writes_then_consumes_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        observe(&hub, "boom at step N", "restart with clean state").await;

        observe(&hub, "boom at step N", "restart and bump timeout").await;

        observe(&hub, "boom at step N", "restart, bump timeout, verify").await;

        let proposals = Proposals::open(tmp.path());

        let registry = crate::registry::SkillRegistry::empty().with_root(
            crate::registry::SkillTier::Global,
            tmp.path().join("skills"),
        );
        let report = distill_all(&hub, &proposals, &registry).await;
        assert_eq!(report.proposals_written.len(), 1);
        let name = &report.proposals_written[0];
        assert!(proposals.root().join(name).join("SKILL.md").is_file());

        // The written proposal passes the strict contract.
        assert!(proposals.validate_dir(name).is_empty());

        // Second pass: nothing new — the pending proposal blocks its
        // own cluster (listed as a proposal with that name).
        let again = distill_all(&hub, &proposals, &registry).await;
        assert!(again.proposals_written.is_empty());
        assert_eq!(again.skipped.len(), 1);
    }
}

#[cfg(test)]
mod update_tests {
    use super::*;
    use crate::distill::testing::distill_all;
    use crate::registry::{SkillRegistry, SkillTier};
    use evolution_core::ObservationHub;
    use evolution_core::observation::{ObservationInput, ObservationKind, ObservationSource};

    async fn observe(hub: &ObservationHub, error: &str, body: &str) {
        hub.record(ObservationInput {
            kind: ObservationKind::Failure,
            source: ObservationSource::Agent,
            summary: format!("fix for {error}"),
            body: body.into(),
            node_kind: Some("sql".into()),
            error: Some(error.into()),
        })
        .await
        .unwrap();
    }

    async fn observe_recipe(hub: &ObservationHub, summary: &str, body: &str) {
        hub.record(ObservationInput {
            kind: ObservationKind::Recipe,
            source: ObservationSource::Agent,
            summary: summary.into(),
            body: body.into(),
            node_kind: Some("sql".into()),
            error: None,
        })
        .await
        .unwrap();
    }

    /// The same evolution arc as [`create_then_update_full_arc`], on
    /// recipe observations clustered by summary signature: three
    /// recordings create the skill, three more revise it.
    #[tokio::test]
    async fn recipe_create_then_update_full_arc() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        let proposals = Proposals::open(tmp.path());
        let registry =
            SkillRegistry::empty().with_root(SkillTier::Global, tmp.path().join("skills"));
        let global = tmp.path().join("skills");

        observe_recipe(&hub, "use CTE for join 1", "recipe one").await;

        observe_recipe(&hub, "use CTE for join 2", "recipe two").await;

        observe_recipe(&hub, "use CTE for join 3", "recipe three").await;

        let report = distill_all(&hub, &proposals, &registry).await;
        assert_eq!(report.proposals_written.len(), 1);
        let name = report.proposals_written[0].clone();
        assert!(name.ends_with("-recipe"), "{name}");

        proposals.approve(&name, &global).unwrap();
        let skill_md = global.join(&name).join("SKILL.md");
        let content = std::fs::read_to_string(&skill_md).unwrap();
        assert!(content.contains("## Recorded recipes"));
        assert!(content.contains("recipe three"));

        // Three NEW recordings on the same summary signature → an
        // update proposal measured against the approved manifest.
        observe_recipe(&hub, "use CTE for join 4", "recipe four").await;

        observe_recipe(&hub, "use CTE for join 5", "recipe five").await;

        observe_recipe(&hub, "use CTE for join 6", "recipe six").await;

        let updated = distill_all(&hub, &proposals, &registry).await;
        assert_eq!(updated.updated_existing, vec![name.clone()]);
        let proposal = proposals.find(&name).unwrap();
        assert!(proposal.update);
        assert_eq!(proposal.status, crate::ProposalStatus::Pending);
    }

    /// The full evolution arc of one pattern: three observations
    /// create the skill, three more revise it, and the revision is
    /// an overlay with the previous version archived.");
    #[tokio::test]
    async fn create_then_update_full_arc() {
        let tmp = tempfile::tempdir().unwrap();
        let hub = ObservationHub::new(tmp.path());
        let proposals = Proposals::open(tmp.path());
        let registry =
            SkillRegistry::empty().with_root(SkillTier::Global, tmp.path().join("skills"));
        let global = tmp.path().join("skills");

        // Round 1: three observations → create proposal → approve.
        observe(&hub, "syntax error near N", "fix one").await;

        observe(&hub, "syntax error near N", "fix two").await;

        observe(&hub, "syntax error near N", "fix three").await;

        let report = distill_all(&hub, &proposals, &registry).await;
        assert_eq!(report.proposals_written.len(), 1);
        let name = report.proposals_written[0].clone();

        proposals.approve(&name, &global).unwrap();
        let skill_md = global.join(&name).join("SKILL.md");
        let content = std::fs::read_to_string(&skill_md).unwrap();
        assert!(content.contains("fix one"));
        assert!(content.contains("fix three"));

        // Round 2: three NEW observations on the same anchor → an
        // update proposal whose body appends only the new evidence.
        observe(&hub, "syntax error near N", "fix four").await;

        observe(&hub, "syntax error near N", "fix five").await;

        observe(&hub, "syntax error near N", "fix six").await;

        let updated = distill_all(&hub, &proposals, &registry).await;
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
        observe(&hub, "syntax error near N", "fix seven").await;

        observe(&hub, "syntax error near N", "fix eight").await;

        let quiet = distill_all(&hub, &proposals, &registry).await;
        assert!(quiet.proposals_written.is_empty());
    }

    /// A human-installed skill without the `auto` tag is never
    /// touched by the loop, and its cluster is consumed so it does not resurface every pass.");
    #[tokio::test]
    async fn human_owned_names_are_protected() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("skills");
        let human = global.join("sql-syntax-error-near-n");
        std::fs::create_dir_all(&human).unwrap();
        std::fs::write(
            human.join("SKILL.md"),
            "---\nname: sql-syntax-error-near-n\ndescription: Hand-written.\ntags: [handmade]\n---\nprecious hand-written content\n",
        )
        .unwrap();

        let hub = ObservationHub::new(tmp.path());
        for i in 0..4 {
            observe(&hub, "syntax error near N", &format!("fix {i}")).await;
        }
        let proposals = Proposals::open(tmp.path());
        let registry = SkillRegistry::empty().with_root(SkillTier::Global, global.clone());

        let report = distill_all(&hub, &proposals, &registry).await;
        assert!(report.proposals_written.is_empty());
        assert!(
            report
                .skipped
                .iter()
                .any(|(_, reason)| reason.contains("human-installed"))
        );
        // Consumed: a second pass considers the cluster but skips it
        // as already consumed — no proposal, no noise.
        let again = distill_all(&hub, &proposals, &registry).await;
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
