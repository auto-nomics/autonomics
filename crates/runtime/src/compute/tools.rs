//! Agent tools resolve the operator registry on invocation, not agent creation.
use super::ComputeRegistry;
use agentik_core::{
    storage::AgentStorage,
    tools::{
        DynToolFunction, ExecutionDeadline, ExecutionMode, ToolContext, ToolError, ToolFunction,
        ToolRegistration,
    },
};
use agentik_sdk::types::{ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

fn error(message: impl Into<String>) -> ToolError {
    ToolError::ValidationFailed {
        message: message.into(),
    }
}

/// Each existing agent observes operator changes before its next request.
/// This uses the standard context interface, independently of optional goal governance.
pub(crate) struct ComputeContext {
    registry: Arc<ComputeRegistry>,
    revision: std::sync::atomic::AtomicU64,
}
impl ComputeContext {
    pub(crate) fn new(registry: Arc<ComputeRegistry>) -> Self {
        Self {
            registry,
            revision: std::sync::atomic::AtomicU64::new(0),
        }
    }
}
#[async_trait]
impl agentik_core::context::ContextProvider for ComputeContext {
    async fn poll(&self) -> Option<String> {
        let catalog = self.registry.catalog();
        if self
            .revision
            .swap(catalog.revision, std::sync::atomic::Ordering::AcqRel)
            == catalog.revision
        {
            return None;
        }
        let sources: Vec<_> = catalog
            .data_sources
            .iter()
            .map(|s| s.mount_path())
            .collect();
        let deployment = self.registry
            .target(&catalog.policy.target_id, Some(catalog.policy.target_revision))
            .ok()?.deployment();
        let status = serde_json::to_string(&json!({
            "policy":catalog.policy,"data_sources":sources,"deployment":deployment
        })).ok()?;
        Some(format!(
            "[Operator compute policy]\n{status}\nNew DAG runs bind this target once. Existing runs and jobs retain their original target/revision. If deployment.source_compatible is false, report both source revisions: the operator must deploy a measured matching worker and enroll a new target revision. Retrying a DAG or approving a download cannot repair that mismatch. Data-source VFS mounts are independent and remain accessible after compute switches. Use compute_status to inspect mounts and bindings. Never repeat uncertain work on another server."
        ))
    }
}

/// Tool admission and the underlying image builder both enforce placement.
/// Direct tool callers use the same gate as an agent session.
pub(crate) fn guard_environment_tools(
    tools: Vec<ToolRegistration>,
    registry: Option<&Arc<ComputeRegistry>>,
) -> Vec<ToolRegistration> {
    let Some(registry) = registry else {
        return tools;
    };
    tools
        .into_iter()
        .map(|template| {
            if !matches!(
                template.definition.name.as_str(),
                "environment_build" | "environment_install"
            ) {
                return template;
            }
            ToolRegistration::new(
                template.definition.clone(),
                Arc::new(EnvironmentTool {
                    registry: registry.clone(),
                    template,
                }),
            )
        })
        .collect()
}
struct EnvironmentTool {
    registry: Arc<ComputeRegistry>,
    template: ToolRegistration,
}
impl EnvironmentTool {
    fn admit(&self) -> Result<(), ToolError> {
        let (target, _) = self.registry.selected().map_err(error)?;
        if !matches!(target.target.backend, super::ComputeBackend::Local) {
            return Err(error(
                "selected remote target requires prebuilt admitted environments; local image building is unavailable under this compute policy",
            ));
        }
        Ok(())
    }
}
#[async_trait]
impl DynToolFunction for EnvironmentTool {
    async fn execute(&self, input: Value) -> Result<ToolResult, ToolError> {
        self.admit()?;
        self.template.implementation.execute(input).await
    }
    async fn execute_with_context(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        self.admit()?;
        self.template
            .implementation
            .execute_with_context(input, ctx)
            .await
    }
    fn validate_input(&self, input: &Value) -> Result<(), ToolError> {
        self.template.implementation.validate_input(input)
    }
    fn execution_mode(&self) -> ExecutionMode {
        self.template.implementation.execution_mode()
    }
    fn timeout_seconds(&self) -> u64 {
        self.template.implementation.timeout_seconds()
    }
    fn execution_deadline(&self) -> ExecutionDeadline {
        self.template.implementation.execution_deadline()
    }
    fn definition(&self) -> ToolDefinition {
        self.template.definition.clone()
    }
}

#[agentik_proc::tool(
    name = "compute_status",
    description = "Read the operator's current compute target, immutable target revisions, independent data-source mounts and recorded run bindings. Compute selection does not remove data access. Use stable VFS data paths from mounts. Agents cannot change the global target. Existing jobs must be observed at their original target_id and target_revision; never repeat an uncertain submission on another target."
)]
pub struct StatusInput {}
struct StatusTool(Arc<ComputeRegistry>);

#[agentik_proc::tool(
    name = "compute_readiness",
    description = "Inspect requested plugin/node addresses, optional exact expected resident manifest/script/image identities, and OCI-to-SIF mappings on the selected immutable target. Returns blockers and next actions. CONFIGURATION_READY is not remote execution, data access, dependency or end-to-end acceptance. Use this before claiming a workflow is deployed; source_compatible and registered alone are insufficient. No jobs or mutations."
)]
pub struct ReadinessInput {
    pub nodes: Vec<String>,
    pub images: Vec<String>,
    pub expected_identities: std::collections::BTreeMap<String, Value>,
}
struct ReadinessTool(Arc<ComputeRegistry>, Arc<data_engine::runtime::DataEngineManager>);
pub(crate) fn readiness_registration(registry: Arc<ComputeRegistry>, engine: Arc<data_engine::runtime::DataEngineManager>) -> ToolRegistration {
    ReadinessTool(registry, engine).into()
}
#[async_trait]
impl ToolFunction for ReadinessTool {
    type Input = ReadinessInput;
    async fn run(&self, input: ReadinessInput) -> Result<ToolResult, ToolError> {
        let request = super::ReadinessRequest { nodes:input.nodes, images:input.images,
            expected_identities:input.expected_identities };
        Ok(ToolResult::success_json(self.0.readiness(&request, &self.1.list_nodes()).map_err(error)?))
    }
}

#[agentik_proc::tool(
    name = "compute_image_evidence",
    description = "Read the resident OCI to SIF mapping for an explicit compute target revision, even when its private images.json is outside VFS. Returns the exact requested/resolved reference, SIF path/SHA, target identity and profile hashes held by that runtime. Does not execute, hash remote SIF bytes, submit work, alter placement or certify dependencies."
)]
#[serde(deny_unknown_fields)]
pub struct ImageEvidenceInput {
    pub target_id: String,
    pub target_revision: u64,
    pub reference: String,
}
struct ImageEvidenceTool(Arc<ComputeRegistry>);
#[async_trait]
impl ToolFunction for ImageEvidenceTool {
    type Input = ImageEvidenceInput;
    async fn run(&self, input: ImageEvidenceInput) -> Result<ToolResult, ToolError> {
        let evidence = self.0.image_evidence(&input.target_id, input.target_revision, &input.reference).map_err(error)?;
        Ok(ToolResult::success_json(serde_json::to_value(evidence).map_err(|e| error(e.to_string()))?))
    }
}
#[async_trait]
impl ToolFunction for StatusTool {
    type Input = StatusInput;
    async fn run(&self, _: StatusInput) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::success_json(
            serde_json::to_value(self.0.status().map_err(error)?)
                .map_err(|e| error(e.to_string()))?,
        ))
    }
}

pub(crate) fn registrations(
    registry: Arc<ComputeRegistry>,
    agent_path: &str,
    storage: Arc<dyn AgentStorage>,
    files: Arc<vfs::OpendalFileStorage>,
) -> Vec<ToolRegistration> {
    let mut tools: Vec<ToolRegistration> = vec![StatusTool(registry.clone()).into(), ImageEvidenceTool(registry.clone()).into()];
    // Definitions need no live connection or state-directory creation.
    let catalog = registry.catalog();
    let candidate = catalog.targets.iter().find_map(|t| {
        registry
            .target(&t.id, Some(t.revision))
            .ok()
            .and_then(|b| b.hpc.clone())
    });
    let client = candidate.unwrap_or_else(|| Arc::new(hpc_tools::HpcClient::definition_only()));
    let mut templates = hpc_tools::registrations(client.clone());
    templates.extend(hpc_tools::transfer::registrations(client, files.clone()));
    for mut template in templates {
        template.definition.input_schema.properties.insert("target_id".into(), json!({"type":"string","description":"Operator compute target owning this account or existing job. Required when multiple remote targets exist. submit must match the global target."}));
        template.definition.input_schema.properties.insert("target_revision".into(), json!({"type":"integer","minimum":1,"description":"Original immutable target revision for existing jobs. Obtain from compute_status or monitor records."}));
        template.definition.description.push_str(" Target selection is operator-controlled. Supply target_id and target_revision to access old targets after switching. Data reads/transfers remain available when a target stops accepting computation.");
        tools.push(ToolRegistration::new(
            template.definition.clone(),
            Arc::new(RoutedTool {
                registry: registry.clone(),
                template,
                actor: agent_path.into(),
                storage: storage.clone(),
                files: files.clone(),
            }),
        ));
    }
    tools
}

struct RoutedTool {
    registry: Arc<ComputeRegistry>,
    template: ToolRegistration,
    actor: String,
    storage: Arc<dyn AgentStorage>,
    files: Arc<vfs::OpendalFileStorage>,
}
impl RoutedTool {
    async fn invoke(
        &self,
        mut input: Value,
        ctx: Option<&ToolContext>,
    ) -> Result<ToolResult, ToolError> {
        let object = input
            .as_object_mut()
            .ok_or_else(|| error("tool input must be an object"))?;
        let id = object
            .remove("target_id")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| error("invalid target_id"))
            })
            .transpose()?;
        let revision = object
            .remove("target_revision")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_u64()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| error("invalid target_revision"))
            })
            .transpose()?;
        let name = self.template.definition.name.as_str();
        let action = input["action"].as_str().unwrap_or("");
        if name == "hpc_jobs" && action == "monitors" {
            let session = ctx
                .and_then(|c| c.session.as_ref())
                .ok_or_else(|| error("monitor inventory requires the owning conversation"))?;
            let agent = self
                .storage
                .get_agent_by_name(&self.actor)
                .await
                .map_err(|e| error(e.to_string()))?
                .ok_or_else(|| error("monitor owner missing"))?;
            if self
                .storage
                .agent_context_id(agent.id)
                .await
                .map_err(|e| error(e.to_string()))?
                != Some(session.session_id)
            {
                return Err(error("monitor context identity mismatch"));
            }
            let views = self
                .registry
                .monitors
                .views(Some(&self.actor))
                .map_err(error)?
                .into_iter()
                .filter(|r| {
                    r.agent_id == agent.id.to_string()
                        && r.session_id == session.session_id.to_string()
                })
                .collect::<Vec<_>>();
            return Ok(ToolResult::success_json(
                json!({"monitors":views,"scientific_admission_granted":false}),
            ));
        }
        let submit = name == "hpc_jobs" && matches!(action, "submit" | "requeue" | "release");
        let mut admission = None;
        let target = if submit {
            let (selected, policy) = self.registry.selected().map_err(error)?;
            if id.as_ref().is_some_and(|id| id != &selected.target.id)
                || revision.is_some_and(|r| r != selected.target.revision)
            {
                return Err(error(
                    "new computation must use the global compute target; inspect the old submission before switching",
                ));
            }
            if action == "submit" {
                let session = ctx
                    .and_then(|c| c.session.as_ref())
                    .ok_or_else(|| error("submission requires the owning conversation"))?;
                let agent = self
                    .storage
                    .get_agent_by_name(&self.actor)
                    .await
                    .map_err(|e| error(e.to_string()))?
                    .ok_or_else(|| error("submission owner missing"))?;
                if self
                    .storage
                    .agent_context_id(agent.id)
                    .await
                    .map_err(|e| error(e.to_string()))?
                    != Some(session.session_id)
                {
                    return Err(error("submission context identity mismatch"));
                }
                let records = self
                    .registry
                    .monitors
                    .views(Some(&self.actor))
                    .map_err(error)?;
                if records.iter().any(|r| {
                    r.agent_id == agent.id.to_string()
                        && r.session_id == session.session_id.to_string()
                        && input["submission_key"].as_str() == Some(r.submission_key.as_str())
                        && matches!(
                            r.state.as_str(),
                            "awaiting_submission" | "submission_uncertain"
                        )
                        && (r.target_id.as_deref() != Some(selected.target.id.as_str())
                            || r.target_revision != Some(selected.target.revision))
                }) {
                    return Err(error(
                        "submission_key has an unresolved intent at another target revision; recover that original intent without cross-target resubmission",
                    ));
                }
            }
            admission = Some(
                self.registry
                    .record_binding(&selected, &policy)
                    .map_err(error)?,
            );
            selected
        } else if let Some(id) = id {
            let catalog = self.registry.catalog();
            if revision.is_none()
                && name == "hpc_jobs"
                && catalog.targets.iter().filter(|t| t.id == id).count() > 1
            {
                return Err(error(
                    "existing job management requires its original target_revision",
                ));
            }
            self.registry.target(&id, revision).map_err(error)?
        } else {
            let catalog = self.registry.catalog();
            let remote: Vec<_> = catalog
                .targets
                .iter()
                .filter(|t| matches!(t.backend, super::ComputeBackend::Slurm { .. }))
                .collect();
            if remote.len() != 1 || revision.is_some() {
                return Err(error(
                    "specify target_id and original target_revision; consult compute_status",
                ));
            }
            self.registry
                .target(&remote[0].id, Some(remote[0].revision))
                .map_err(error)?
        };
        if let super::ComputeBackend::Slurm { settings } = &target.target.backend {
            if !settings.tools_enabled {
                return Err(error(
                    "account tools are disabled for this target; computation routing is independent",
                ));
            }
        }
        let client = target.hpc.clone().ok_or_else(|| {
            error("selected target is local; HPC operations require a remote target")
        })?;
        let hub = self
            .registry
            .monitors
            .target_hub(&target.monitor_key)
            .map_err(error)?;
        let mut registrations = hpc_tools::jobs::monitored_registrations(
            client.clone(),
            hpc_tools::jobs::Monitoring {
                hub,
                agent_path: self.actor.clone(),
                storage: self.storage.clone(),
            },
        );
        registrations.extend(hpc_tools::files::registrations(client.clone()));
        registrations.extend(hpc_tools::transfer::registrations(
            client,
            self.files.clone(),
        ));
        let tool = registrations
            .into_iter()
            .find(|r| r.definition.name == name)
            .ok_or_else(|| error("target tool unavailable"))?;
        let mut result = match ctx {
            Some(ctx) => tool.implementation.execute_with_context(input, ctx).await,
            None => tool.implementation.execute(input).await,
        }?;
        if let agentik_types::ToolResultContent::Json(value) = &mut result.content {
            let value = value
                .as_object_mut()
                .ok_or_else(|| error("target returned a non-object tool response"))?;
            value.insert(
                "compute_target".into(),
                json!({"target_id":target.target.id,"target_revision":target.target.revision}),
            );
            if let Some(binding) = admission {
                value.insert(
                    "execution_binding".into(),
                    serde_json::to_value(binding).map_err(|e| error(e.to_string()))?,
                );
            }
        }
        // Original tools retain their submission nonce, ownership, logs and transfer checks.
        Ok(result)
    }
}
#[async_trait]
impl DynToolFunction for RoutedTool {
    async fn execute(&self, input: Value) -> Result<ToolResult, ToolError> {
        self.invoke(input, None).await
    }
    async fn execute_with_context(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        self.invoke(input, Some(ctx)).await
    }
    fn validate_input(&self, _: &Value) -> Result<(), ToolError> {
        Ok(())
    }
    fn execution_mode(&self) -> ExecutionMode {
        self.template.implementation.execution_mode()
    }
    fn timeout_seconds(&self) -> u64 {
        self.template.implementation.timeout_seconds()
    }
    fn execution_deadline(&self) -> ExecutionDeadline {
        self.template.implementation.execution_deadline()
    }
    fn definition(&self) -> ToolDefinition {
        self.template.definition.clone()
    }
}
