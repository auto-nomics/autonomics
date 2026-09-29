//! Engine-bound skill tools: workflow instantiation and skill evals.
//!
//! The pure halves — template parsing/rendering ([`skills::workflow`])
//! and check evaluation ([`skills::eval`]) — live in the skills crate.
//! This module binds them to the session's
//! [`DataEngineClient`](data_engine::runtime::DataEngineClient): the
//! rendered nodes and edges go through the same `add_node` /
//! `add_edge_port` path the agent's own DAG tooling uses, so a
//! workflow-built DAG is indistinguishable from a hand-built one
//! (`view_dag`, `run_dag`, history, everything just works).
//!
//! Node ids are namespaced with a prefix (default
//! `<workflow-stem>__`) so a template's ids cannot collide with nodes
//! the agent already placed in the session DAG.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;
use skills::manager::SkillManager;
use skills::workflow::RenderedWorkflow;

// ────────────────────────── inputs ──────────────────────────

#[tool(
    name = "skill_run_workflow",
    description = "Instantiate a skill's workflow template into the session DAG. \
        First call skill_workflows <skill> to see the templates and their \
        parameter schemas. Nodes are created and edges wired; the DAG is NOT \
        run by default — review with view_dag, then run_dag. Node ids are \
        prefixed for namespacing; the result reports the id mapping."
)]
pub struct SkillRunWorkflowInput {
    #[desc = "Exact skill name."]
    pub skill: String,
    #[desc = "Workflow template stem (from skill_workflows). Default \"main\"."]
    pub workflow: Option<String>,
    #[desc = "JSON object of template parameters (defaults applied automatically)."]
    pub params: serde_json::Value,
    #[desc = "Prefix applied to node ids. Default \"<workflow>__\"."]
    pub id_prefix: Option<String>,
    #[desc = "Run the DAG immediately after building it. Default false."]
    pub run: Option<bool>,
}

#[tool(
    name = "skill_eval",
    description = "Run one skill's bundled evals: each case instantiates its \
        workflow with fixed params, runs the DAG, and checks the run report \
        (statuses, row counts, output files). Returns a pass/fail report per \
        case. Eval nodes are added and removed from the session DAG \
        automatically — the session is left as it was."
)]
pub struct SkillEvalInput {
    #[desc = "Exact skill name."]
    pub skill: String,
    #[desc = "Eval file stem to run (from skill_workflows listing). Omit to run all evals."]
    pub eval: Option<String>,
}

// ────────────────────────── tools ──────────────────────────

pub struct SkillRunWorkflowTool {
    pub manager: Arc<SkillManager>,
    pub client: Arc<DataEngineClient>,
}

#[async_trait]
impl ToolFunction for SkillRunWorkflowTool {
    type Input = SkillRunWorkflowInput;

    async fn run(&self, input: SkillRunWorkflowInput) -> Result<ToolResult, ToolError> {
        let skill = input.skill.trim();
        let workflow = input.workflow.unwrap_or_else(|| "main".to_string());
        let Some(doc) = self.load_doc(skill) else {
            return Ok(ToolResult::error(format!(
                "skill_run_workflow: no skill named {skill:?}"
            )));
        };
        self.manager.record_usage(skill, skills::UsageKind::Run);
        let Some(dir) = doc.dir.clone() else {
            return Ok(ToolResult::error(format!(
                "skill_run_workflow: skill {skill:?} is builtin and carries no workflows"
            )));
        };
        let template = match skills::workflow::WorkflowTemplate::load(&dir, &workflow) {
            Ok(t) => t,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "skill_run_workflow: workflow {workflow:?} of {skill:?}: {e}"
                )));
            }
        };
        let rendered = match template.render(&input.params) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "skill_run_workflow: rendering {skill:?}/{workflow}: {e}"
                )));
            }
        };
        let prefix = input
            .id_prefix
            .clone()
            .unwrap_or_else(|| format!("{workflow}__"));

        if let Err(e) = build_dag(&self.client, &rendered, &prefix).await {
            return Ok(ToolResult::error(format!(
                "skill_run_workflow: building DAG: {e}"
            )));
        }

        let mut out = format!(
            "Built workflow {skill:?}/{workflow}: {} node(s), {} edge(s).\n",
            rendered.nodes.len(),
            rendered.edges.len()
        );
        out.push_str("Node id mapping (template → session):\n");
        for node in &rendered.nodes {
            out.push_str(&format!("  {} → {prefix}{}\n", node.id, node.id));
        }

        if input.run.unwrap_or(false) {
            match self.client.run_dag().await {
                Ok(report) => {
                    let failed: Vec<&str> = report
                        .statuses
                        .iter()
                        .filter(|(_, s)| !matches!(s, data_engine::dag::RuntimeStatus::Success))
                        .map(|(id, _)| id.as_str())
                        .collect();
                    if report.ok {
                        out.push_str("\nDAG run: OK\n");
                    } else {
                        out.push_str(&format!(
                            "\nDAG run: FAILED. Non-succeeded nodes: {}\n",
                            failed.join(", ")
                        ));
                    }
                }
                Err(e) => {
                    return Ok(ToolResult::error(format!(
                        "skill_run_workflow: DAG built but run failed: {e}"
                    )));
                }
            }
        } else {
            out.push_str("\nDAG not run — review with view_dag, then run_dag.");
        }
        Ok(ToolResult::success(out))
    }
}

pub struct SkillEvalTool {
    pub manager: Arc<SkillManager>,
    pub client: Arc<DataEngineClient>,
}

#[async_trait]
impl ToolFunction for SkillEvalTool {
    type Input = SkillEvalInput;

    async fn run(&self, input: SkillEvalInput) -> Result<ToolResult, ToolError> {
        let skill = input.skill.trim();
        let Some(doc) = self.load_doc(skill) else {
            return Ok(ToolResult::error(format!(
                "skill_eval: no skill named {skill:?}"
            )));
        };
        self.manager.record_usage(skill, skills::UsageKind::Eval);
        let Some(dir) = doc.dir.clone() else {
            return Ok(ToolResult::error(format!(
                "skill_eval: skill {skill:?} is builtin and carries no evals"
            )));
        };
        if doc.evals.is_empty() {
            return Ok(ToolResult::error(format!(
                "skill_eval: skill {skill:?} bundles no evals"
            )));
        }
        let stems: Vec<String> = match input.eval.as_deref() {
            Some(one) => {
                let one = one.trim();
                if !doc.evals.iter().any(|s| s == one) {
                    return Ok(ToolResult::error(format!(
                        "skill_eval: no eval {one:?} (available: {})",
                        doc.evals.join(", ")
                    )));
                }
                vec![one.to_string()]
            }
            None => doc.evals.clone(),
        };

        let mut pairs: Vec<(skills::EvalCase, Vec<serde_json::Value>)> = Vec::new();
        // Node id→kind maps per case, parallel to `pairs`.
        let mut kinds: Vec<Vec<(String, String)>> = Vec::new();
        let mut build_errors: Vec<String> = Vec::new();
        for stem in &stems {
            let cases = match skills::eval::load_cases(&dir, stem) {
                Ok(cases) => cases,
                Err(e) => {
                    build_errors.push(format!("eval {stem:?}: {e}"));
                    continue;
                }
            };
            for case in cases {
                let outcome = self.run_case(&dir, &case).await;
                match outcome {
                    Ok((nodes, node_kinds)) => {
                        pairs.push((case, nodes));
                        kinds.push(node_kinds);
                    }
                    Err(e) => build_errors.push(format!("case {:?}: {e}", case.name)),
                }
            }
        }

        if pairs.is_empty() {
            return Ok(ToolResult::error(format!(
                "skill_eval: no eval case ran. {}",
                build_errors.join("; ")
            )));
        }
        let report = skills::eval::evaluate_all(&pairs);
        // Auto-capture: every failed case becomes an anchored
        // observation, so repeated eval failures feed the
        // distillation loop without anyone remembering to record
        // them — the anchor is the first failed check's node kind.
        for (index, case) in report.cases.iter().enumerate() {
            if !case.passed {
                self.record_eval_failure(skill, case, &kinds[index]);
            }
        }
        let mut out = format!(
            "Eval report for {skill:?}: {}\n",
            if report.passed { "PASS" } else { "FAIL" }
        );
        for case in &report.cases {
            out.push_str(&format!(
                "\ncase {} — {}\n",
                case.name,
                if case.passed { "pass" } else { "FAIL" }
            ));
            for check in &case.checks {
                if check.passed {
                    out.push_str(&format!("  ok   {}\n", check.node));
                } else {
                    out.push_str(&format!("  FAIL {} — {}\n", check.node, check.reason));
                }
            }
        }
        for e in &build_errors {
            out.push_str(&format!("\nsetup error: {e}\n"));
        }
        let passed = report.passed && build_errors.is_empty();
        let mut result = ToolResult::success(out);
        if !passed {
            result.is_error = Some(true);
        }
        Ok(result)
    }
}

impl SkillRunWorkflowTool {
    fn load_doc(&self, skill: &str) -> Option<skills::SkillDocument> {
        self.manager.registry().get(skill).unwrap_or_else(|e| {
            tracing::warn!(skill = skill, error = %e, "skill workflow tool: unreadable skill");
            None
        })
    }
}

impl SkillEvalTool {
    fn load_doc(&self, skill: &str) -> Option<skills::SkillDocument> {
        // Same helper shape as the run tool; duplicated rather than
        // trait-abstracted — two call sites, and the shared state
        // differs (manager + client).
        self.manager.registry().get(skill).unwrap_or_else(|e| {
            tracing::warn!(skill = skill, error = %e, "skill eval tool: unreadable skill");
            None
        })
    }

    /// Run one eval case end-to-end: build (prefixed), run, strip the
    /// prefix from node reports, then remove the nodes.
    /// Run one eval case, returning its stripped node reports plus the
    /// template's id→kind map (for anchoring failure observations).
    async fn run_case(
        &self,
        dir: &std::path::Path,
        case: &skills::EvalCase,
    ) -> Result<(Vec<serde_json::Value>, Vec<(String, String)>), String> {
        let template = skills::workflow::WorkflowTemplate::load(dir, &case.workflow)
            .map_err(|e| e.to_string())?;
        let rendered = template.render(&case.params).map_err(|e| e.to_string())?;
        let node_kinds: Vec<(String, String)> = rendered
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.kind.clone()))
            .collect();
        let prefix = format!("{}__", case.name);
        build_dag(&self.client, &rendered, &prefix)
            .await
            .map_err(|e| e.to_string())?;
        let report = self.client.run_dag().await.map_err(|e| e.to_string())?;

        // Strip the eval prefix so checks reference template ids.
        let nodes: Vec<serde_json::Value> = report
            .nodes
            .iter()
            .map(|node| {
                let mut value = serde_json::to_value(node).unwrap_or(serde_json::Value::Null);
                let stripped = value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|id| id.strip_prefix(&prefix))
                    .map(str::to_string);
                if let (Some(rest), Some(obj)) = (stripped, value.as_object_mut()) {
                    obj.insert("id".into(), serde_json::Value::String(rest));
                }
                value
            })
            .collect();

        // Best-effort cleanup: the session DAG must look untouched.
        // Edges first — a node with live wiring refuses removal — then
        // nodes in reverse declaration order (a stable, if heuristic,
        // topological order for the common chain case).
        for edge in rendered.edges.iter().rev() {
            let _ = self
                .client
                .delete_edge(
                    format!("{prefix}{}", edge.from),
                    edge.from_port,
                    format!("{prefix}{}", edge.to),
                    edge.to_port,
                )
                .await;
        }
        for node in rendered.nodes.iter().rev() {
            let _ = self
                .client
                .remove_node(format!("{prefix}{}", node.id))
                .await;
        }
        Ok((nodes, node_kinds))
    }

    /// Record one failed eval case as an anchored observation:
    /// summary carries the case identity, the body lists the failed
    /// checks, and the anchor is the first failed check's node kind
    /// plus its failure reason. Repeated eval failures therefore
    /// cluster in distillation exactly like manual observations.
    fn record_eval_failure(
        &self,
        skill: &str,
        case: &skills::eval::CaseResult,
        node_kinds: &[(String, String)],
    ) {
        let failed: Vec<&skills::eval::CheckResult> =
            case.checks.iter().filter(|c| !c.passed).collect();
        let Some(first) = failed.first() else {
            return;
        };
        let anchor_kind = node_kinds
            .iter()
            .find(|(id, _)| id == &first.node)
            .map(|(_, kind)| kind.clone());
        let mut body = format!("Eval case {} of skill {skill:?} failed:\n", case.name);
        for check in &failed {
            body.push_str(&format!("- {}: {}\n", check.node, check.reason));
        }
        body.push_str(
            "Inspect the eval inputs and the workflow template; a fix here \
             should update the template or its fixtures.",
        );
        let input = skills::ObservationInput {
            kind: skills::ObservationKind::Failure,
            source: skills::ObservationSource::Eval,
            summary: format!("eval case {} fails for skill {skill}", case.name),
            body,
            node_kind: anchor_kind,
            error: Some(first.reason.clone()),
        };
        if let Err(e) = self.manager.record_observation(input) {
            tracing::warn!(skill = skill, error = %e, "cannot record eval-failure observation");
        }
    }
}

/// Add all nodes, then all edges, through the engine client.
async fn build_dag(
    client: &DataEngineClient,
    rendered: &RenderedWorkflow,
    prefix: &str,
) -> Result<(), String> {
    for node in &rendered.nodes {
        client
            .add_node(
                format!("{prefix}{}", node.id),
                node.kind.clone(),
                node.spec.clone(),
            )
            .await
            .map_err(|e| format!("add_node {:?}: {e}", node.id))?;
    }
    for edge in &rendered.edges {
        client
            .add_edge_port(
                format!("{prefix}{}", edge.from),
                edge.from_port,
                format!("{prefix}{}", edge.to),
                edge.to_port,
            )
            .await
            .map_err(|e| format!("add_edge {:?}→{:?}: {e}", edge.from, edge.to))?;
    }
    Ok(())
}

/// Build the engine-bound skill tool registrations.
pub fn skill_workflow_registrations(
    manager: Arc<SkillManager>,
    client: Arc<DataEngineClient>,
) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(SkillRunWorkflowTool {
            manager: manager.clone(),
            client: client.clone(),
        }),
        ToolRegistration::from(SkillEvalTool { manager, client }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_core::tools::ToolFunction as _;

    #[test]
    fn prefix_defaults_are_namespaced() {
        // The prefix contract used above: `<stem>__` keeps template ids
        // collision-free while remaining greppable in view_dag output.
        let prefix = format!("{}__", "main");
        assert_eq!(prefix, "main__");
        assert!("main__read".strip_prefix(&prefix).is_some());
    }

    /// A skill whose workflow chains two `echo` nodes, plus a smoke
    /// eval over it.
    fn write_skill(root: &std::path::Path) {
        let dir = root.join("echo-flow");
        std::fs::create_dir_all(dir.join("workflow")).unwrap();
        std::fs::create_dir_all(dir.join("evals")).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: echo-flow\ndescription: Two-node echo workflow for tests.\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("workflow").join("main.toml"),
            r#"
description = "Chain two echo nodes."

[params]
label = { type = "string", default = "hello" }

[[node]]
id = "a"
kind = "echo"
spec = {}

[[node]]
id = "b"
kind = "echo"
spec = {}

[[edge]]
from = "a"
from_port = 0
to = "b"
to_port = 0
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("evals").join("smoke.toml"),
            r#"
[[case]]
name = "smoke"
description = "Both nodes run."
workflow = "main"

[[case.checks]]
node = "a"

[[case.checks]]
node = "b"
"#,
        )
        .unwrap();
    }

    fn make_client() -> Arc<DataEngineClient> {
        let infra = Arc::new(container_runtime::ContainerExecutionInfra::from_config(
            container_runtime::PodmanConfig::default(),
        ));
        let engine = data_engine::data_engine::DataEngine::builder()
            .with_container_execution(infra)
            .build();
        Arc::new(
            data_engine::runtime::DataEngineManager::new(engine)
                .client_for_session("skill-workflow-test"),
        )
    }

    #[tokio::test]
    async fn workflow_instantiation_builds_and_runs_in_the_session_dag() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(&tmp.path().join("skills"));

        let tool = SkillRunWorkflowTool {
            manager: Arc::new(skills::SkillManager::new(tmp.path())),
            client: make_client(),
        };
        let out = tool
            .run(SkillRunWorkflowInput {
                skill: "echo-flow".into(),
                workflow: Some("main".into()),
                params: serde_json::json!({}),
                id_prefix: None,
                run: Some(true),
            })
            .await
            .unwrap();
        let text = out.text_content();
        assert!(text.contains("2 node(s), 1 edge(s)"), "{text}");
        assert!(text.contains("a → main__a"), "{text}");
        assert!(text.contains("DAG run: OK"), "{text}");
    }

    #[tokio::test]
    async fn evals_run_check_and_clean_up() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(&tmp.path().join("skills"));
        let client = make_client();

        let tool = SkillEvalTool {
            manager: Arc::new(skills::SkillManager::new(tmp.path())),
            client: client.clone(),
        };
        let out = tool
            .run(SkillEvalInput {
                skill: "echo-flow".into(),
                eval: None,
            })
            .await
            .unwrap();
        let text = out.text_content();
        assert!(text.contains("PASS"), "{text}");
        assert!(text.contains("case smoke — pass"), "{text}");

        // Cleanup: eval nodes were removed from the session DAG.
        // view_dag renders a text listing; no prefixed ids may remain.
        let view = client.view_dag().await.unwrap_or_default();
        let leaked = view
            .lines()
            .filter(|l| l.contains("__a") || l.contains("__b"))
            .count();
        assert_eq!(
            leaked, 0,
            "eval nodes must be removed after the run: {view}"
        );
    }

    #[tokio::test]
    async fn missing_skill_and_bad_params_surface_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let run_tool = SkillRunWorkflowTool {
            manager: Arc::new(skills::SkillManager::new(tmp.path())),
            client: make_client(),
        };
        let missing = run_tool
            .run(SkillRunWorkflowInput {
                skill: "ghost".into(),
                workflow: None,
                params: serde_json::json!({}),
                id_prefix: None,
                run: None,
            })
            .await
            .unwrap();
        assert!(missing.is_error.unwrap_or(false));

        write_skill(&tmp.path().join("skills"));
        let bad_params = run_tool
            .run(SkillRunWorkflowInput {
                skill: "echo-flow".into(),
                workflow: None,
                params: serde_json::json!({ "label": 5 }),
                id_prefix: None,
                run: None,
            })
            .await
            .unwrap();
        assert!(bad_params.is_error.unwrap_or(false));
    }

    /// A failing eval records an anchored observation through the
    /// manager; the same failure three times clusters and distills
    /// into a pending proposal — the RSI loop's smallest full turn.
    #[tokio::test]
    async fn failing_evals_feed_the_distillation_loop() {
        let tmp = tempfile::tempdir().unwrap();
        // A skill whose check can never pass.
        let dir = tmp.path().join("skills").join("broken-flow");
        std::fs::create_dir_all(dir.join("evals")).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: broken-flow\ndescription: A skill whose eval always fails.\n---\nb\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("evals").join("always.toml"),
            r#"
[[case]]
name = "always"
workflow = "main"
[[case.checks]]
node = "a"
rows_min = 99
"#,
        )
        .unwrap();
        write_skill(&tmp.path().join("skills"));
        // Swap the skill identity: reuse the echo workflow, but under
        // the broken-flow name.
        std::fs::create_dir_all(dir.join("workflow")).unwrap();
        std::fs::write(
            dir.join("workflow").join("main.toml"),
            r#"
description = "Echo chain."
[[node]]
id = "a"
kind = "echo"
spec = {}
"#,
        )
        .unwrap();

        let manager = Arc::new(skills::SkillManager::new(tmp.path()));
        let tool = SkillEvalTool {
            manager: manager.clone(),
            client: make_client(),
        };
        for _ in 0..3 {
            let out = tool
                .run(SkillEvalInput {
                    skill: "broken-flow".into(),
                    eval: None,
                })
                .await
                .unwrap();
            assert!(out.is_error.unwrap_or(false));
        }

        // Three identical failures → one content-hashed observation
        // (idempotent), so the cluster threshold needs a hand: record
        // two more variants of the same anchor.
        let observations = manager.observations();
        assert_eq!(observations.list().len(), 1, "identical failures dedupe");
        for body in ["variant b", "variant c"] {
            manager
                .record_observation(skills::ObservationInput {
                    kind: skills::ObservationKind::Failure,
                    source: skills::ObservationSource::Agent,
                    summary: "eval always fails".into(),
                    body: body.into(),
                    node_kind: Some("echo".into()),
                    error: Some("output_rows is None, expected >= 99".into()),
                })
                .unwrap();
        }

        let report = manager.distill().unwrap();
        assert_eq!(report.proposals_written.len(), 1, "{report:?}");
        let name = &report.proposals_written[0];
        let proposals = manager.proposals();
        assert_eq!(proposals.list()[0].status, skills::ProposalStatus::Pending);
        assert!(proposals.validate_dir(name).is_empty());

        // Approve: the skill lands in the global tier and the
        // generation advances — the loop closed.
        let generation = manager.generation();
        let outcome = manager.approve_proposal(name).unwrap();
        assert!(outcome.destination.ends_with(name.as_str()));
        assert_eq!(manager.generation(), generation + 1);
        let names: Vec<String> = manager
            .registry()
            .list()
            .into_iter()
            .map(|e| e.meta.name)
            .collect();
        assert!(names.contains(name));

        // And a second distill pass is a no-op.
        let again = manager.distill().unwrap();
        assert!(again.proposals_written.is_empty());
    }
}
