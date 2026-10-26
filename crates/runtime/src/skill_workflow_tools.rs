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

use agentik_core::memory::{
    KmsSystem, KnowledgeEvent, KnowledgeEvidenceInput, MemoryStore, SkillArtifact,
};
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
        workflow with fixed params, runs the DAG, and checks the full run \
        report (statuses, summary counts, output files paths, row counts, \
        output types). Returns a pass/fail report per case. Eval nodes are \
        added and removed from the session DAG automatically — the session \
        is left as it was."
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
    pub kms: Option<Arc<KmsSystem>>,
}

#[async_trait]
impl ToolFunction for SkillRunWorkflowTool {
    type Input = SkillRunWorkflowInput;

    async fn run(&self, input: SkillRunWorkflowInput) -> Result<ToolResult, ToolError> {
        let skill = input.skill.trim().to_string();
        let workflow = input.workflow.clone().unwrap_or_else(|| "main".to_string());
        if let Some(system) = &self.kms
            && let Some(artifacts) = kms_artifacts(system, &skill).await
        {
            self.manager.record_usage(&skill, skills::UsageKind::Run);
            record_skill_usage(system, &skill, skills::UsageKind::Run).await;
            let relative_path = format!("workflow/{workflow}.toml");
            let Some(artifact) = artifacts
                .iter()
                .find(|artifact| artifact.relative_path == relative_path)
            else {
                return Ok(ToolResult::error(format!(
                    "skill_run_workflow: no workflow {workflow:?} for skill {skill:?}"
                )));
            };
            let template = match skills::workflow::WorkflowTemplate::from_toml(&artifact.content) {
                Ok(template) => template,
                Err(error) => {
                    return Ok(ToolResult::error(format!(
                        "skill_run_workflow: workflow {workflow:?} of {skill:?}: {error}"
                    )));
                }
            };
            return self
                .build_workflow(input, &skill, &workflow, template)
                .await;
        }

        let Some(doc) = self.load_doc(&skill) else {
            return Ok(ToolResult::error(format!(
                "skill_run_workflow: no skill named {skill:?}"
            )));
        };
        self.manager.record_usage(&skill, skills::UsageKind::Run);
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
        self.build_workflow(input, &skill, &workflow, template)
            .await
    }
}

impl SkillRunWorkflowTool {
    async fn build_workflow(
        &self,
        input: SkillRunWorkflowInput,
        skill: &str,
        workflow: &str,
        template: skills::workflow::WorkflowTemplate,
    ) -> Result<ToolResult, ToolError> {
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
            match self.client.run_dag(Some(format!("skill:{skill}"))).await {
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
    pub kms: Option<Arc<KmsSystem>>,
}

#[async_trait]
impl ToolFunction for SkillEvalTool {
    type Input = SkillEvalInput;

    async fn run(&self, input: SkillEvalInput) -> Result<ToolResult, ToolError> {
        let skill = input.skill.trim();
        if let Some(system) = &self.kms
            && let Some(artifacts) = kms_artifacts(system, skill).await
        {
            self.manager.record_usage(skill, skills::UsageKind::Eval);
            record_skill_usage(system, skill, skills::UsageKind::Eval).await;
            return self
                .run_kms_eval(skill, input.eval.as_deref(), &artifacts)
                .await;
        }

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
                let outcome = self.run_case(skill, &dir, &case).await;
                match outcome {
                    Ok((nodes, node_kinds)) => {
                        pairs.push((case, nodes));
                        kinds.push(node_kinds);
                    }
                    Err(e) => build_errors.push(format!("case {:?}: {e}", case.name)),
                }
            }
        }

        Ok(self.finish_eval(skill, pairs, kinds, build_errors).await)
    }
}

async fn kms_artifacts(system: &Arc<KmsSystem>, skill: &str) -> Option<Vec<SkillArtifact>> {
    let store = system.store();
    let versions = store.list_skill_versions(10_000).await.ok()?;
    let version = versions
        .into_iter()
        .find(|version| version.status == "active" && version.skill_name == skill)?;
    store.list_skill_artifacts(version.id).await.ok()
}

async fn record_skill_usage(system: &KmsSystem, skill: &str, kind: skills::UsageKind) {
    let input = KnowledgeEvidenceInput {
        source_kind: "skill_usage".to_string(),
        source_id: uuid::Uuid::new_v4().to_string(),
        evidence_kind: kind.as_str().to_string(),
        summary: format!("skill {skill} recorded {} usage", kind.as_str()),
        body: format!(
            "Skill {skill:?} was used through the runtime ({}). This is fitness \
             evidence for future procedure consolidation and diagnosis.",
            kind.as_str()
        ),
        anchor_path: Some(format!("/procedures/{skill}")),
        error: None,
    };
    if let Err(error) = system
        .submit_event(KnowledgeEvent::ObservationRecorded { input })
        .await
    {
        tracing::warn!(skill, kind = kind.as_str(), %error, "cannot queue skill usage evidence");
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
    async fn run_kms_eval(
        &self,
        skill: &str,
        requested: Option<&str>,
        artifacts: &[SkillArtifact],
    ) -> Result<ToolResult, ToolError> {
        let eval_artifacts = artifacts
            .iter()
            .filter(|artifact| {
                artifact
                    .relative_path
                    .strip_prefix("evals/")
                    .is_some_and(|path| path.ends_with(".toml"))
            })
            .collect::<Vec<_>>();
        if eval_artifacts.is_empty() {
            return Ok(ToolResult::error(format!(
                "skill_eval: skill {skill:?} bundles no evals"
            )));
        }

        let selected: Vec<&SkillArtifact> =
            match requested.map(str::trim).filter(|v| !v.is_empty()) {
                Some(requested) => {
                    let Some(artifact) = eval_artifacts.iter().copied().find(|artifact| {
                        artifact.relative_path == format!("evals/{requested}.toml")
                    }) else {
                        return Ok(ToolResult::error(format!(
                            "skill_eval: no eval {requested:?}"
                        )));
                    };
                    vec![artifact]
                }
                None => eval_artifacts,
            };

        let mut pairs = Vec::new();
        let mut kinds = Vec::new();
        let mut build_errors = Vec::new();
        for artifact in selected {
            let cases = match skills::eval::parse_cases(&artifact.content) {
                Ok(cases) => cases,
                Err(error) => {
                    build_errors.push(format!("eval {}: {error}", artifact.relative_path));
                    continue;
                }
            };
            for case in cases {
                let outcome = self.run_kms_case(skill, artifacts, &case).await;
                match outcome {
                    Ok((nodes, node_kinds)) => {
                        pairs.push((case, nodes));
                        kinds.push(node_kinds);
                    }
                    Err(error) => build_errors.push(format!("case {:?}: {error}", case.name)),
                }
            }
        }
        Ok(self.finish_eval(skill, pairs, kinds, build_errors).await)
    }

    fn load_doc(&self, skill: &str) -> Option<skills::SkillDocument> {
        // Same helper shape as the run tool; duplicated rather than
        // trait-abstracted — two call sites, and the shared state
        // differs (manager + client).
        self.manager.registry().get(skill).unwrap_or_else(|e| {
            tracing::warn!(skill = skill, error = %e, "skill eval tool: unreadable skill");
            None
        })
    }

    async fn finish_eval(
        &self,
        skill: &str,
        pairs: Vec<(skills::EvalCase, Vec<serde_json::Value>)>,
        kinds: Vec<Vec<(String, String)>>,
        build_errors: Vec<String>,
    ) -> ToolResult {
        if pairs.is_empty() {
            return ToolResult::error(format!(
                "skill_eval: no eval case ran. {}",
                build_errors.join("; ")
            ));
        }
        let report = skills::eval::evaluate_all(&pairs);
        for (index, case) in report.cases.iter().enumerate() {
            if !case.passed {
                self.record_eval_failure(skill, case, &kinds[index]).await;
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
        for error in &build_errors {
            out.push_str(&format!("\nsetup error: {error}\n"));
        }
        let passed = report.passed && build_errors.is_empty();
        let mut result = ToolResult::success(out);
        if !passed {
            result.is_error = Some(true);
        }
        result
    }

    async fn run_kms_case(
        &self,
        skill: &str,
        artifacts: &[SkillArtifact],
        case: &skills::EvalCase,
    ) -> Result<(Vec<serde_json::Value>, Vec<(String, String)>), String> {
        let relative_path = format!("workflow/{}.toml", case.workflow);
        let artifact = artifacts
            .iter()
            .find(|artifact| artifact.relative_path == relative_path)
            .ok_or_else(|| format!("missing workflow {:?}", case.workflow))?;
        let template = skills::workflow::WorkflowTemplate::from_toml(&artifact.content)
            .map_err(|e| e.to_string())?;
        let rendered = template.render(&case.params).map_err(|e| e.to_string())?;
        let node_kinds = rendered
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node.kind.clone()))
            .collect::<Vec<_>>();
        let prefix = format!("{}__", case.name);
        build_dag(&self.client, &rendered, &prefix)
            .await
            .map_err(|e| e.to_string())?;
        let report = self
            .client
            .run_dag(Some(format!("skill:{skill}")))
            .await
            .map_err(|e| e.to_string())?;
        let nodes = report
            .nodes
            .iter()
            .map(|node| {
                let mut value = serde_json::to_value(node).unwrap_or(serde_json::Value::Null);
                let stripped = value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|id| id.strip_prefix(&prefix))
                    .map(str::to_string);
                if let (Some(rest), Some(object)) = (stripped, value.as_object_mut()) {
                    object.insert("id".into(), serde_json::Value::String(rest));
                }
                value
            })
            .collect::<Vec<_>>();

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

    /// Run one eval case end-to-end: build (prefixed), run, strip the
    /// prefix from node reports, then remove the nodes.
    /// Run one eval case, returning its stripped node reports plus the
    /// template's id→kind map (for anchoring failure observations).
    async fn run_case(
        &self,
        skill: &str,
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
        let report = self
            .client
            .run_dag(Some(format!("skill:{skill}")))
            .await
            .map_err(|e| e.to_string())?;

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

    /// Record one failed eval case as KMS evidence anchored to the skill.
    async fn record_eval_failure(
        &self,
        skill: &str,
        case: &skills::eval::CaseResult,
        _node_kinds: &[(String, String)],
    ) {
        let failed: Vec<&skills::eval::CheckResult> =
            case.checks.iter().filter(|c| !c.passed).collect();
        let Some(first) = failed.first() else {
            return;
        };
        let mut body = format!("Eval case {} of skill {skill:?} failed:\n", case.name);
        for check in &failed {
            body.push_str(&format!("- {}: {}\n", check.node, check.reason));
        }
        body.push_str(
            "Inspect the eval inputs and the workflow template; a fix here \
             should update the template or its fixtures.",
        );
        let summary = format!("eval case {} fails for skill {skill}", case.name);
        if let Some(system) = &self.kms {
            let input = KnowledgeEvidenceInput {
                source_kind: "skill_failure".to_string(),
                source_id: uuid::Uuid::new_v4().to_string(),
                evidence_kind: "eval_failure".to_string(),
                summary,
                body,
                anchor_path: Some(format!("/procedures/{skill}")),
                error: Some(first.reason.clone()),
            };
            if let Err(error) = system
                .submit_event(KnowledgeEvent::ObservationRecorded { input })
                .await
            {
                tracing::warn!(
                    skill,
                    case = case.name.as_str(),
                    %error,
                    "cannot queue KMS eval-failure evidence"
                );
            }
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
    kms: Option<Arc<KmsSystem>>,
) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(SkillRunWorkflowTool {
            manager: manager.clone(),
            client: client.clone(),
            kms: kms.clone(),
        }),
        ToolRegistration::from(SkillEvalTool {
            manager,
            client,
            kms,
        }),
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

    async fn kms_system_with_echo_skill(failing_eval: bool) -> Arc<KmsSystem> {
        let storage = Arc::new(
            agentik_core::memory::TursoKmsStorage::open_in_memory()
                .await
                .unwrap(),
        );
        storage
            .import_skill_version(agentik_core::memory::KnowledgeSkillVersionInput {
                skill_name: "kms-echo-flow".to_string(),
                title: "KMS echo flow".to_string(),
                body_md: "Workflow and eval are loaded from KMS artifacts.".to_string(),
                metadata_json: serde_json::json!({
                    "description": "KMS-backed echo workflow.",
                    "tags": ["kms"],
                    "tier": "workspace"
                }),
                node_path: "/procedures/kms-echo-flow".to_string(),
                source_tier: "workspace".to_string(),
                source_path: None,
                artifacts: vec![
                    agentik_core::memory::KnowledgeSkillArtifactInput {
                        relative_path: "workflow/main.toml".to_string(),
                        content: r#"
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
"#
                        .to_string(),
                    },
                    agentik_core::memory::KnowledgeSkillArtifactInput {
                        relative_path: "evals/smoke.toml".to_string(),
                        content: if failing_eval {
                            r#"
[[case]]
name = "smoke"
description = "This check intentionally fails."
workflow = "main"

[[case.checks]]
node = "a"
rows_min = 99
"#
                        } else {
                            r#"
[[case]]
name = "smoke"
description = "Both nodes run."
workflow = "main"

[[case.checks]]
node = "a"

[[case.checks]]
node = "b"
"#
                        }
                        .to_string(),
                    },
                ],
            })
            .await
            .unwrap();
        KmsSystem::new(
            agentik_core::memory::MemoryConfig::new(),
            storage,
            Arc::new(agentik_core::memory::EmptyMemorySource),
        )
        .await
    }

    async fn wait_for_evidence(system: &KmsSystem, evidence_kind: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let evidence = system.store().list_pending_evidence(100).await.unwrap();
            if evidence
                .iter()
                .any(|item| item.evidence_kind == evidence_kind)
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "KMS evidence worker never produced {evidence_kind}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn workflow_instantiation_builds_and_runs_in_the_session_dag() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(&tmp.path().join("skills"));

        let tool = SkillRunWorkflowTool {
            manager: Arc::new(skills::SkillManager::new(tmp.path())),
            client: make_client(),
            kms: None,
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
    async fn workflow_instantiation_prefers_kms_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let kms = kms_system_with_echo_skill(false).await;
        let tool = SkillRunWorkflowTool {
            manager: Arc::new(skills::SkillManager::new(tmp.path())),
            client: make_client(),
            kms: Some(Arc::clone(&kms)),
        };
        let out = tool
            .run(SkillRunWorkflowInput {
                skill: "kms-echo-flow".into(),
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
        wait_for_evidence(&kms, "run").await;
        kms.shutdown();
    }

    #[tokio::test]
    async fn evals_run_check_and_clean_up() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(&tmp.path().join("skills"));
        let client = make_client();

        let tool = SkillEvalTool {
            manager: Arc::new(skills::SkillManager::new(tmp.path())),
            client: client.clone(),
            kms: None,
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
    async fn evals_run_from_kms_artifacts_and_clean_up() {
        let tmp = tempfile::tempdir().unwrap();
        let client = make_client();
        let kms = kms_system_with_echo_skill(false).await;
        let tool = SkillEvalTool {
            manager: Arc::new(skills::SkillManager::new(tmp.path())),
            client: client.clone(),
            kms: Some(Arc::clone(&kms)),
        };
        let out = tool
            .run(SkillEvalInput {
                skill: "kms-echo-flow".into(),
                eval: Some("smoke".into()),
            })
            .await
            .unwrap();
        let text = out.text_content();
        assert!(text.contains("PASS"), "{text}");
        assert!(text.contains("case smoke — pass"), "{text}");

        let view = client.view_dag().await.unwrap_or_default();
        let leaked = view
            .lines()
            .filter(|line| line.contains("smoke__a") || line.contains("smoke__b"))
            .count();
        assert_eq!(leaked, 0, "KMS eval nodes leaked into the session: {view}");
        wait_for_evidence(&kms, "eval").await;
        kms.shutdown();
    }

    #[tokio::test]
    async fn failing_kms_evals_feed_knowledge_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        let kms = kms_system_with_echo_skill(true).await;
        let tool = SkillEvalTool {
            manager: Arc::new(skills::SkillManager::new(tmp.path())),
            client: make_client(),
            kms: Some(Arc::clone(&kms)),
        };
        let out = tool
            .run(SkillEvalInput {
                skill: "kms-echo-flow".into(),
                eval: Some("smoke".into()),
            })
            .await
            .unwrap();
        assert!(out.is_error.unwrap_or(false));

        wait_for_evidence(&kms, "eval").await;
        wait_for_evidence(&kms, "eval_failure").await;
        let evidence = kms.store().list_pending_evidence(100).await.unwrap();
        let failure = evidence
            .iter()
            .find(|item| item.evidence_kind == "eval_failure")
            .expect("eval-failure evidence");
        assert_eq!(failure.source_kind, "skill_failure");
        assert_eq!(
            failure.anchor_path.as_deref(),
            Some("/procedures/kms-echo-flow")
        );
        assert!(failure.error.is_some());
        kms.shutdown();
    }

    #[tokio::test]
    async fn missing_skill_and_bad_params_surface_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let run_tool = SkillRunWorkflowTool {
            manager: Arc::new(skills::SkillManager::new(tmp.path())),
            client: make_client(),
            kms: None,
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
}
