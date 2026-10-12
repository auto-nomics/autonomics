//! Deterministic distillation: observation clusters → skill proposals.
//!
//! Where EvoScientist spends an auxiliary LLM per turn to distill
//! observations and another to synthesize skills, this loop is pure
//! code end to end. The observation-domain half — incremental
//! semantic clustering over record-time vectors — lives in
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
//! Naming material (kind, node kind, human-readable signature) is
//! derived HERE, on the consuming side: the cluster module groups,
//! the skill domain names. The generated name derives from the
//! cluster's semantic center, so the same pattern yields the same
//! skill name across runs — stable, greppable, and
//! collision-checkable against the installed library.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::SkillError;
use crate::format;
use crate::proposals::Proposals;
use evolution_core::cluster::{Cluster, ClusterProcessor, MIN_CLUSTER, cluster_hash};
use evolution_core::observation::{Observation, ObservationKind};

/// The skill-side view of one surfaced cluster: the naming material
/// this domain derives (kind, node kind, signature) plus the
/// hydrated member records. Consumers that only group observations
/// should not have to carry naming concerns — and this module should
/// not have to carry clustering state.
pub struct Pattern {
    pub kind: ObservationKind,
    pub node_kind: String,
    pub signature: String,
    pub observations: Vec<Observation>,
}

impl Pattern {
    /// Hydrate a cluster's members and derive its signature. Members
    /// the caller filtered out (e.g. routed to another audience)
    /// sit this pass out; a cluster with no hydrated members is not
    /// this subsystem's business and yields `None`.
    pub fn of(
        cluster: &Cluster,
        observations: &[Observation],
        embeddings: &HashMap<String, Vec<f32>>,
    ) -> Result<Option<Pattern>, SkillError> {
        let by_id: HashMap<&str, &Observation> =
            observations.iter().map(|o| (o.id.as_str(), o)).collect();
        let mut members: Vec<&Observation> = cluster
            .members
            .iter()
            .filter_map(|id| by_id.get(id.as_str()).copied())
            .collect();
        members.sort_by(|a, b| a.id.cmp(&b.id));
        let Some(anchor) = members.first() else {
            return Ok(None);
        };
        // All members share one partition, so any one of them
        // carries the (kind, node_kind) the partition encoded.
        let signature = representative_signature(&members, embeddings)?;
        Ok(Some(Pattern {
            kind: anchor.kind,
            node_kind: anchor.node_kind.clone().unwrap_or_default(),
            signature,
            observations: members.into_iter().cloned().collect(),
        }))
    }
}

/// The skill name a pattern distills to. Failure names keep the bare
/// `<node>-<signature>` form; recipe and caveat names carry a kind
/// suffix so the three namespaces never collide.
pub fn skill_name(pattern: &Pattern) -> Option<String> {
    let sig_slug = slug(&pattern.signature);
    let kind_slug = slug(&pattern.node_kind);
    if kind_slug.is_empty() {
        return None;
    }
    let stem = if sig_slug.is_empty() {
        match pattern.kind {
            ObservationKind::Failure => format!("{kind_slug}-fixes"),
            ObservationKind::Recipe | ObservationKind::Caveat => kind_slug.clone(),
        }
    } else {
        format!("{kind_slug}-{sig_slug}")
    };
    let capped = match pattern.kind {
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
    #[test]
    fn kind_suffixes_keep_skill_namespaces_disjoint() {
        let failure = Pattern {
            kind: ObservationKind::Failure,
            node_kind: "sql".into(),
            signature: "recipe".into(),
            observations: Vec::new(),
        };
        let recipe = Pattern {
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

/// Render the skeleton SKILL.md for a pattern.
///
/// Conservative by design: it presents what was observed, not advice
/// the loop has no basis for. The approval gate (a human, for now)
/// decides whether the pattern deserves promotion.
pub fn render_skill_md(pattern: &Pattern) -> String {
    // Per-kind profile: capitalized noun for the description, plain
    // noun for the entries heading, how the signature is introduced,
    // the description tail, and the kind tag after `auto`.
    let (cap_noun, noun, sig_prefix, tail, kind_tag) = match pattern.kind {
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
    let sig_line = if pattern.signature.is_empty() {
        String::new()
    } else {
        format!("{sig_prefix} `{}`", pattern.signature)
    };
    let mut out = String::new();
    out.push_str(&format!(
        "---\nname: {}\ndescription: {} recorded for the `{}` node kind{}. \
         {}\ntags: [auto{}, {}]\n---\n\n",
        skill_name(pattern).unwrap_or_default(),
        cap_noun,
        pattern.node_kind,
        yaml_safe(&sig_line),
        tail,
        kind_tag,
        slug(&pattern.node_kind),
    ));
    out.push_str(&format!(
        "# Auto-distilled: `{}`{}\n\n",
        pattern.node_kind, sig_line
    ));
    out.push_str(&format!(
        "Distilled from {} recorded {} observation(s). Each entry below was \
         learned during real work; none of it is invented.\n\n",
        pattern.observations.len(),
        noun,
    ));
    out.push_str(&format!("## Recorded {noun}\n\n"));
    for observation in &pattern.observations {
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

/// One full distillation pass over surfaced clusters: filter
/// consumed → write proposals (create or update). Pure with respect
/// to the stores — callers pass the clustering pass's surfaced
/// clusters, the observations those clusters' members hydrate to,
/// the record-time vectors, the proposal area, and a registry view
/// for existing-skill resolution.
pub fn distill(
    clusters: &[Cluster],
    observations: &[Observation],
    embeddings: &HashMap<String, Vec<f32>>,
    proposals: &Proposals,
    registry: &crate::registry::SkillRegistry,
) -> Result<DistillReport, SkillError> {
    let mut report = DistillReport::default();
    let processed = proposals.processed_clusters();
    let installed = registry.list();
    // Deterministic proposal order independent of storage order.
    let mut patterns = Vec::new();
    for cluster in clusters {
        if let Some(pattern) = Pattern::of(cluster, observations, embeddings)? {
            patterns.push((cluster_hash(&cluster.members), pattern));
        }
    }
    patterns.sort_by(|a, b| {
        a.1.node_kind
            .cmp(&b.1.node_kind)
            .then(a.1.kind.cmp(&b.1.kind))
            .then(a.1.signature.cmp(&b.1.signature))
    });
    for (hash, pattern) in patterns {
        report.candidates_considered += 1;
        if processed.contains(&hash) {
            report
                .skipped
                .push((hash, "cluster already consumed".into()));
            continue;
        }
        let Some(name) = skill_name(&pattern) else {
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
            let fresh: Vec<&Observation> = pattern
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
                pattern.kind.label(),
                pattern.node_kind,
                if pattern.signature.is_empty() {
                    String::new()
                } else {
                    format!(" / `{}`", pattern.signature)
                }
            );
            proposals.submit(
                &name,
                &hash,
                &rationale,
                // The manifest records the FULL cluster membership so
                // the next delta is measured against this update.
                pattern.observations.iter().map(|o| o.id.clone()).collect(),
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
        std::fs::write(dir.join("SKILL.md"), render_skill_md(&pattern))?;
        let rationale = format!(
            "{} anchored {} observation(s) on `{}`{}",
            pattern.observations.len(),
            pattern.kind.label(),
            pattern.node_kind,
            if pattern.signature.is_empty() {
                String::new()
            } else {
                format!(" / `{}`", pattern.signature)
            }
        );
        proposals.submit(
            &name,
            &hash,
            &rationale,
            pattern.observations.iter().map(|o| o.id.clone()).collect(),
            false,
        )?;
        report.proposals_written.push(name);
    }
    Ok(report)
}

/// Choose the observation closest to the cluster's semantic center,
/// then derive a human-readable signature from it. This gives vector
/// clusters stable, reviewable names without depending on processing
/// order.
fn representative_signature(
    members: &[&Observation],
    embeddings: &HashMap<String, Vec<f32>>,
) -> Result<String, SkillError> {
    let vectors = members
        .iter()
        .filter_map(|member| embeddings.get(&member.id).map(|v| v.as_slice()))
        .collect::<Vec<_>>();
    // No vectors under the current signature: fall back to the first
    // member's text rather than failing the whole pass.
    let Some(mean) = text_embedding::mean_vector(&vectors)
        .map_err(|e| SkillError::Evolution(evolution_core::Error::Embedding(e.to_string())))
        .ok()
        .or_else(|| vectors.first().map(|v| v.to_vec()))
    else {
        return Ok(String::new());
    };
    let similarities = vectors
        .iter()
        .enumerate()
        .filter(|(_, v)| v.len() == mean.len())
        .map(|(index, vector)| {
            text_embedding::cosine(&mean, vector)
                .map(|similarity| (similarity, index))
                .map_err(|e| SkillError::Evolution(evolution_core::Error::Embedding(e.to_string())))
        })
        .collect::<Result<Vec<_>, SkillError>>()?;
    // Reindex against the vector subset's source members.
    let with_vector = members
        .iter()
        .filter(|m| embeddings.contains_key(&m.id))
        .collect::<Vec<_>>();
    let representative = similarities
        .into_iter()
        .max_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, index)| with_vector[index])
        .unwrap_or(&members[0]);

    if representative.kind == ObservationKind::Failure
        && let Some(error) = representative.error.as_deref()
    {
        let normalized = signature(error);
        if !normalized.is_empty() {
            return Ok(normalized);
        }
    }
    Ok(keyword_signature(&representative.summary))
}

/// A short, readable signature for semantic clusters. Unlike error
/// signatures, it does not preserve the `N` placeholder: names such as
/// "generic recipe 3" should cluster to "generic recipe".
fn keyword_signature(text: &str) -> String {
    const STOP_WORDS: [&str; 14] = [
        "a", "an", "and", "are", "at", "for", "in", "is", "of", "on", "or", "the", "this", "to",
    ];
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| word.len() > 1 && !word.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_ascii_lowercase)
        .filter(|word| !STOP_WORDS.contains(&word.as_str()))
        .take(6)
        .collect::<Vec<_>>()
        .join(" ")
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

#[cfg(test)]
mod testing {
    //! Shared plumbing: every distillation test records through a real
    //! hub so observations carry the vectors the clustering reads.
    use super::*;
    use evolution_core::ObservationHub;
    use evolution_core::storage::ObservationStorage;

    /// One full distillation pass over everything the hub holds.
    pub(crate) async fn distill_all(
        hub: &ObservationHub,
        proposals: &Proposals,
        registry: &crate::registry::SkillRegistry,
    ) -> DistillReport {
        let processor = ClusterProcessor::new(Arc::clone(hub.storage()));
        let report = processor
            .execute_cluster(&hub.embedding_signature())
            .await
            .unwrap();
        let observations = hub.storage().list().await;
        let ids = observations
            .iter()
            .map(|o| o.id.clone())
            .collect::<Vec<_>>();
        let embeddings = hub.load_embeddings(&ids).await.unwrap();
        distill(
            &report.surfaced,
            &observations,
            &embeddings,
            proposals,
            registry,
        )
        .unwrap()
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

        proposals.apply(&name, &global).unwrap();
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
    /// an overlay with the previous version archived.
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

        proposals.apply(&name, &global).unwrap();
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
        proposals.apply(&name, &global).unwrap();
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
    /// touched by the loop, and its cluster is consumed so it does not resurface every pass.
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
