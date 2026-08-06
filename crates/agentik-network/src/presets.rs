//! Preset network specs — ready-to-use topologies for common multi-agent
//! patterns.
//!
//! Each function returns a [`NetworkSpec`] that can be passed directly to
//! [`AgentNetwork::build`](crate::AgentNetwork::build). The caller must
//! supply matching [`AgentProfile`](agentik_core::AgentProfile)s for each
//! node's `profile` field.

use crate::spec::{
    EdgeSpec, EdgeTrigger, NetworkSpec, NodeSpec, TerminationSpec, TransformSpec,
};

/// Build an adversarial review loop (arena) between a **writer** and a
/// **reviewer**.
///
/// Topology:
/// ```text
/// writer ──(OnDone)──> reviewer
/// reviewer ──(OnDone)──> writer   (only if not accepted)
/// ```
///
/// The writer produces a manuscript from `initial_prompt`. The reviewer
/// critiques it. If the reviewer's response contains the accept pattern
/// (default: `VERDICT: ACCEPT`), the network terminates with the writer
/// winning. Otherwise the feedback is forwarded back to the writer for
/// revision. The loop ends after `max_rounds` revisions if no acceptance
/// is reached.
///
/// # Arguments
/// * `topic` — the writing prompt for the first turn.
/// * `max_rounds` — max revision cycles before forced termination.
/// * `writer_profile` — profile name for the writer node.
/// * `reviewer_profile` — profile name for the reviewer node.
/// * `accept_pattern` — case-insensitive substring that signals acceptance.
///   Defaults to `"VERDICT: ACCEPT"`.
pub fn arena(
    topic: impl Into<String>,
    max_rounds: usize,
    writer_profile: &str,
    reviewer_profile: &str,
    accept_pattern: Option<&str>,
) -> NetworkSpec {
    let accept = accept_pattern.unwrap_or("VERDICT: ACCEPT");

    NetworkSpec {
        name: "arena".into(),
        nodes: vec![
            NodeSpec {
                name: "writer".into(),
                profile: writer_profile.into(),
                initial_prompt: Some(topic.into()),
            },
            NodeSpec {
                name: "reviewer".into(),
                profile: reviewer_profile.into(),
                initial_prompt: None,
            },
        ],
        edges: vec![
            // writer → reviewer: submit manuscript
            EdgeSpec {
                from: "writer".into(),
                to: "reviewer".into(),
                trigger: EdgeTrigger::OnDone,
                transform: Some(TransformSpec {
                    template: "## Manuscript Submission\n\n{content}\n\n\
                        ---\n\
                        Review this manuscript. If it meets your standards, \
                        include exactly `VERDICT: ACCEPT` in your response. \
                        Otherwise provide specific, actionable feedback."
                        .into(),
                }),
            },
            // reviewer → writer: return feedback (only fires if not accepted)
            // NOTE: OnDone fires unconditionally; the conductor checks
            // termination conditions BEFORE routing. So if the reviewer
            // accepts, the network ends before this edge fires.
            EdgeSpec {
                from: "reviewer".into(),
                to: "writer".into(),
                trigger: EdgeTrigger::OnDone,
                transform: Some(TransformSpec {
                    template: "## Reviewer Feedback (Round {round})\n\n{content}\n\n\
                        ---\n\
                        Please revise your manuscript based on this feedback \
                        and resubmit."
                        .into(),
                }),
            },
        ],
        termination: TerminationSpec::Any {
            specs: vec![
                TerminationSpec::Condition {
                    node: "reviewer".into(),
                    pattern: accept.into(),
                },
                TerminationSpec::MaxRounds { max: max_rounds * 2 },
            ],
        },
    }
}

/// Build a linear pipeline of N nodes: `node_0 → node_1 → ... → node_{n-1}`.
///
/// Each node forwards its output to the next. The network terminates when
/// the final node completes.
///
/// # Arguments
/// * `name` — network name.
/// * `stages` — list of `(node_name, profile_name)` pairs, in pipeline order.
/// * `initial_prompt` — prompt for the first node.
pub fn pipeline(
    name: &str,
    stages: &[(String, String)],
    initial_prompt: impl Into<String>,
) -> NetworkSpec {
    assert!(stages.len() >= 2, "pipeline needs at least 2 stages");

    let prompt = initial_prompt.into();
    let nodes: Vec<NodeSpec> = stages
        .iter()
        .enumerate()
        .map(|(i, (node_name, profile))| NodeSpec {
            name: node_name.clone(),
            profile: profile.clone(),
            initial_prompt: if i == 0 { Some(prompt.clone()) } else { None },
        })
        .collect();

    let edges: Vec<EdgeSpec> = stages
        .windows(2)
        .map(|pair| EdgeSpec {
            from: pair[0].0.clone(),
            to: pair[1].0.clone(),
            trigger: EdgeTrigger::OnDone,
            transform: None,
        })
        .collect();

    let last_node = stages.last().unwrap().0.clone();

    NetworkSpec {
        name: name.into(),
        nodes,
        edges,
        termination: TerminationSpec::AnyNodeDone {
            nodes: vec![last_node],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_spec_validates() {
        let spec = arena(
            "Write about LDSC",
            3,
            "writer-profile",
            "reviewer-profile",
            None,
        );
        spec.validate().unwrap();
    }

    #[test]
    fn arena_spec_has_cycle() {
        let spec = arena("test", 3, "w", "r", None);
        assert_eq!(spec.edges.len(), 2);
        assert_eq!(spec.edges[0].from, "writer");
        assert_eq!(spec.edges[0].to, "reviewer");
        assert_eq!(spec.edges[1].from, "reviewer");
        assert_eq!(spec.edges[1].to, "writer");
    }

    #[test]
    fn arena_max_rounds_doubles() {
        // max_rounds counts per-node completions. In a 2-node cycle,
        // one "round" = 2 completions (writer + reviewer). So MaxRounds
        // should be max_rounds * 2.
        let spec = arena("test", 3, "w", "r", None);
        match &spec.termination {
            TerminationSpec::Any { specs } => {
                let mr = specs.iter().find_map(|s| match s {
                    TerminationSpec::MaxRounds { max } => Some(*max),
                    _ => None,
                });
                assert_eq!(mr, Some(6)); // 3 rounds * 2 completions
            }
            _ => panic!("expected Any"),
        }
    }

    #[test]
    fn pipeline_spec_validates() {
        let spec = pipeline(
            "research-pipeline",
            &[
                ("literature".into(), "lit-profile".into()),
                ("analysis".into(), "analysis-profile".into()),
                ("writing".into(), "writer-profile".into()),
            ],
            "Research topic: BMI GWAS",
        );
        spec.validate().unwrap();
        assert_eq!(spec.edges.len(), 2);
        assert_eq!(spec.nodes.len(), 3);
    }
}
