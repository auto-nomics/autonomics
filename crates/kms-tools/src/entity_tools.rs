//! Entity tools (9): create / update / delete / get / search / list +
//! nomenclature add / update / delete.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use kms::{EntityFilter, KmsService, Language, Nomenclature};
use serde_json::json;
use uuid::Uuid;

fn svc_err(e: String) -> ToolError {
    ToolError::ExecutionFailed { source: e.into() }
}

fn lang_from_str(s: &str) -> Language {
    match s {
        "EN" => Language::EN,
        _ => Language::ZH,
    }
}

fn names_to_json(names: &[Nomenclature]) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = names
        .iter()
        .map(|n| {
            json!({
                "id": n.id.to_string(),
                "lang": n.lang.as_str(),
                "full": n.full,
                "abbr": n.abbr,
            })
        })
        .collect();
    json!(arr)
}

// ═══════════════════════════════════════════════════════════════════
// kms_create_entity
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_create_entity",
    description = "Create a new entity in the knowledge graph. Each (lang, full) combination \
                   must be unique — duplicates within the same call are silently removed. \
                   If an entity with the same name already exists, it is returned as-is."
)]
pub struct CreateEntityInput {
    #[desc = "Array of nomenclatures: [{lang: 'ZH'|'EN', full: string, abbr?: string}]"]
    pub names: Vec<NomenclatureInput>,
    #[desc = "Brief definition of the entity"]
    pub definition: String,
}

#[tool(name = "_nomenclature", description = "")]
pub struct NomenclatureInput {
    pub lang: String,
    pub full: String,
    pub abbr: Option<String>,
}

pub struct KmsCreateEntityTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsCreateEntityTool {
    type Input = CreateEntityInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if input.definition.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "definition must not be empty".into(),
            });
        }
        if input.names.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "names must not be empty".into(),
            });
        }
        let nomenclatures: Vec<Nomenclature> = input
            .names
            .into_iter()
            .map(|n| Nomenclature {
                id: Uuid::new_v4(),
                lang: lang_from_str(&n.lang),
                full: n.full,
                abbr: n.abbr,
            })
            .collect();

        let (entity, existed) = self
            .svc
            .create_entity(nomenclatures, &input.definition)
            .await
            .map_err(svc_err)?;

        Ok(ToolResult::success_json(json!({
            "name": entity.name.first().map(|n| n.full.as_str()).unwrap_or(""),
            "definition": entity.definition,
            "existed": existed,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_update_entity
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_update_entity",
    description = "Update an entity's definition and/or nomenclatures. Provide name_ref OR id \
                   to locate the entity."
)]
pub struct UpdateEntityInput {
    #[desc = "Current nomenclature full name of the entity to update (use id if entity has no nomenclature)"]
    pub name_ref: Option<String>,
    #[desc = "UUID of the entity to update (use when entity has no nomenclature)"]
    pub id: Option<String>,
    #[desc = "New definition for the entity"]
    pub definition: Option<String>,
    #[desc = "New nomenclature array: [{lang: 'ZH'|'EN', full: string, abbr?: string}]"]
    pub names: Option<Vec<NomenclatureInput>>,
}

pub struct KmsUpdateEntityTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsUpdateEntityTool {
    type Input = UpdateEntityInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entity = if let Some(id_str) = input.id {
            let id = Uuid::parse_str(&id_str).map_err(|e| svc_err(e.to_string()))?;
            self.svc
                .update_entity_by_id(
                    id,
                    input.definition.as_deref(),
                    input.names.map(|ns| {
                        ns.into_iter()
                            .map(|n| Nomenclature {
                                id: Uuid::new_v4(),
                                lang: lang_from_str(&n.lang),
                                full: n.full,
                                abbr: n.abbr,
                            })
                            .collect()
                    }),
                )
                .await
                .map_err(svc_err)?
        } else if let Some(name_ref) = input.name_ref {
            self.svc
                .update_entity_by_ref(
                    &name_ref,
                    input.definition.as_deref(),
                    input.names.map(|ns| {
                        ns.into_iter()
                            .map(|n| Nomenclature {
                                id: Uuid::new_v4(),
                                lang: lang_from_str(&n.lang),
                                full: n.full,
                                abbr: n.abbr,
                            })
                            .collect()
                    }),
                )
                .await
                .map_err(svc_err)?
        } else {
            return Err(ToolError::ValidationFailed {
                message: "must provide name_ref or id".into(),
            });
        };

        Ok(ToolResult::success_json(json!({
            "id": entity.id.to_string(),
            "names": names_to_json(&entity.name),
            "definition": entity.definition,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_delete_entity
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_delete_entity",
    description = "Delete an entity and all its nomenclatures by UUID."
)]
pub struct DeleteEntityInput {
    #[desc = "UUID of the entity to delete"]
    pub id: String,
}

pub struct KmsDeleteEntityTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsDeleteEntityTool {
    type Input = DeleteEntityInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let id = Uuid::parse_str(&input.id).map_err(|e| svc_err(e.to_string()))?;
        self.svc.delete_entity(id).await.map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({ "deleted": input.id })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_get_entity
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_get_entity",
    description = "Get an entity by its nomenclature name."
)]
pub struct GetEntityInput {
    #[desc = "Nomenclature full name of the entity"]
    pub name: String,
}

pub struct KmsGetEntityTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsGetEntityTool {
    type Input = GetEntityInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let id = self.svc.resolve(&input.name).await.map_err(svc_err)?;
        let entity = self.svc.get_entity(id).await.map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({
            "id": entity.id.to_string(),
            "names": names_to_json(&entity.name),
            "definition": entity.definition,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_search_entity
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_search_entity",
    description = "Search entities by nomenclature name (prefix match)."
)]
pub struct SearchEntityInput {
    #[desc = "Search keyword"]
    pub keyword: String,
}

pub struct KmsSearchEntityTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsSearchEntityTool {
    type Input = SearchEntityInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entities = self
            .svc
            .search_entity(&input.keyword)
            .await
            .map_err(svc_err)?;
        let results: Vec<_> = entities
            .iter()
            .map(|e| {
                json!({
                    "id": e.id.to_string(),
                    "name": e.name.first().map(|n| n.full.as_str()).unwrap_or(""),
                    "definition": e.definition,
                })
            })
            .collect();
        Ok(ToolResult::success_json(json!({
            "count": results.len(),
            "entities": results,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_list_entities
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_list_entities",
    description = "List entities, optionally filtered by condition."
)]
pub struct ListEntitiesInput {
    #[desc = "Filter: 'empty_definition', 'no_nomenclature', or 'all' (default)"]
    #[default = "all"]
    pub filter: Option<String>,
}

pub struct KmsListEntitiesTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsListEntitiesTool {
    type Input = ListEntitiesInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let filter = match input.filter.as_deref() {
            Some("empty_definition") => EntityFilter::EmptyDefinition,
            Some("no_nomenclature") => EntityFilter::NoNomenclature,
            _ => EntityFilter::All,
        };
        let entities = self.svc.list_entities(filter).await.map_err(svc_err)?;
        let results: Vec<_> = entities
            .iter()
            .map(|e| {
                json!({
                    "id": e.id.to_string(),
                    "names": names_to_json(&e.name),
                    "definition": e.definition,
                })
            })
            .collect();
        Ok(ToolResult::success_json(json!({
            "count": results.len(),
            "entities": results,
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_add_nomenclature
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_add_nomenclature",
    description = "Add a new nomenclature (name variant) to an existing entity."
)]
pub struct AddNomenclatureInput {
    #[desc = "UUID of the entity"]
    pub entity_id: String,
    #[desc = "Language: 'ZH' or 'EN'"]
    pub lang: String,
    #[desc = "Full name"]
    pub full: String,
    #[desc = "Abbreviation (optional)"]
    pub abbr: Option<String>,
}

pub struct KmsAddNomenclatureTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsAddNomenclatureTool {
    type Input = AddNomenclatureInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let id = Uuid::parse_str(&input.entity_id).map_err(|e| svc_err(e.to_string()))?;
        let entity = self
            .svc
            .add_nomenclature(id, lang_from_str(&input.lang), input.full, input.abbr)
            .await
            .map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({
            "entity_id": entity.id.to_string(),
            "names": names_to_json(&entity.name),
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_update_nomenclature
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_update_nomenclature",
    description = "Update an existing nomenclature's lang, full name, or abbreviation."
)]
pub struct UpdateNomenclatureInput {
    #[desc = "UUID of the entity"]
    pub entity_id: String,
    #[desc = "UUID of the nomenclature to update"]
    pub nomenclature_id: String,
    #[desc = "New language: 'ZH' or 'EN'"]
    pub lang: String,
    #[desc = "New full name"]
    pub full: String,
    #[desc = "New abbreviation (pass empty string to clear)"]
    pub abbr: Option<String>,
}

pub struct KmsUpdateNomenclatureTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsUpdateNomenclatureTool {
    type Input = UpdateNomenclatureInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entity_id = Uuid::parse_str(&input.entity_id).map_err(|e| svc_err(e.to_string()))?;
        let nom_id = Uuid::parse_str(&input.nomenclature_id).map_err(|e| svc_err(e.to_string()))?;
        let abbr = input
            .abbr
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let entity = self
            .svc
            .update_nomenclature(
                entity_id,
                nom_id,
                lang_from_str(&input.lang),
                input.full,
                abbr,
            )
            .await
            .map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({
            "entity_id": entity.id.to_string(),
            "names": names_to_json(&entity.name),
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// kms_delete_nomenclature
// ═══════════════════════════════════════════════════════════════════

#[tool(
    name = "kms_delete_nomenclature",
    description = "Delete a nomenclature from an entity. The entity must retain at least one nomenclature."
)]
pub struct DeleteNomenclatureInput {
    #[desc = "UUID of the entity"]
    pub entity_id: String,
    #[desc = "UUID of the nomenclature to delete"]
    pub nomenclature_id: String,
}

pub struct KmsDeleteNomenclatureTool {
    pub svc: Arc<KmsService>,
}

#[async_trait]
impl ToolFunction for KmsDeleteNomenclatureTool {
    type Input = DeleteNomenclatureInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entity_id = Uuid::parse_str(&input.entity_id).map_err(|e| svc_err(e.to_string()))?;
        let nom_id = Uuid::parse_str(&input.nomenclature_id).map_err(|e| svc_err(e.to_string()))?;
        let entity = self
            .svc
            .delete_nomenclature(entity_id, nom_id)
            .await
            .map_err(svc_err)?;
        Ok(ToolResult::success_json(json!({
            "entity_id": entity.id.to_string(),
            "names": names_to_json(&entity.name),
        })))
    }
}

// ═══════════════════════════════════════════════════════════════════
// Registration
// ═══════════════════════════════════════════════════════════════════

pub fn registrations(svc: Arc<KmsService>) -> Vec<ToolRegistration> {
    vec![
        KmsCreateEntityTool { svc: svc.clone() }.into(),
        KmsUpdateEntityTool { svc: svc.clone() }.into(),
        KmsDeleteEntityTool { svc: svc.clone() }.into(),
        KmsGetEntityTool { svc: svc.clone() }.into(),
        KmsSearchEntityTool { svc: svc.clone() }.into(),
        KmsListEntitiesTool { svc: svc.clone() }.into(),
        KmsAddNomenclatureTool { svc: svc.clone() }.into(),
        KmsUpdateNomenclatureTool { svc: svc.clone() }.into(),
        KmsDeleteNomenclatureTool { svc }.into(),
    ]
}
