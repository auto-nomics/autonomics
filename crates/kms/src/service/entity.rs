//! Entity CRUD and nomenclature management.

use uuid::Uuid;

use crate::error::{Error, Result};
use crate::language::Language;
use crate::storage::repo;
use crate::storage::types::{Entity, Nomenclature};

use super::KmsService;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EntityFilter {
    EmptyDefinition,
    NoNomenclature,
    All,
}

// ── Entity ──
impl KmsService {
    pub async fn create_entity(
        &self,
        names: Vec<Nomenclature>,
        definition: &str,
    ) -> Result<(Entity, bool)> {
        let conn = &*self.storage.conn().await;

        // Dedup: if an entity with the same name already exists, return it.
        let lookup_name = names
            .iter()
            .find(|n| n.lang == Language::ZH)
            .or_else(|| names.first())
            .map(|n| n.full.as_str())
            .unwrap_or("");

        if !lookup_name.is_empty() {
            if let Some(existing) = repo::entity_find_by_exact_name(conn, lookup_name).await? {
                return Ok((existing, true));
            }
        }

        // Deduplicate input nomenclatures: keep only the first per (lang, full).
        let mut seen: Vec<(Language, String)> = Vec::new();
        let names: Vec<Nomenclature> = names
            .into_iter()
            .filter(|n| {
                let dup = seen.iter().any(|(l, f)| *l == n.lang && f == &n.full);
                if !dup {
                    seen.push((n.lang, n.full.clone()));
                }
                !dup
            })
            .collect();

        // Check each remaining nomenclature against existing DB records.
        let mut db_safe_names = Vec::new();
        for n in names {
            let exists = repo::entity_find_by_exact_name(conn, &n.full).await?;
            if exists.is_none() {
                db_safe_names.push(n);
            }
        }
        let names = db_safe_names;

        if names.is_empty() {
            return Err("所有提供的命名已存在于数据库中，无法创建新实体".into());
        }

        let entity = Entity {
            id: Uuid::new_v4(),
            name: names,
            definition: definition.to_string(),
        };
        repo::entity_create(conn, &entity).await?;
        Ok((entity, false))
    }

    pub async fn get_entity(&self, id: Uuid) -> Result<Entity> {
        Ok(repo::entity_get(&*self.storage.conn().await, id).await?)
    }

    pub async fn delete_entity(&self, id: Uuid) -> Result<()> {
        Ok(repo::entity_delete(&*self.storage.conn().await, id).await?)
    }

    pub async fn add_nomenclature(
        &self,
        entity_id: Uuid,
        lang: Language,
        full: String,
        abbr: Option<String>,
    ) -> Result<Entity> {
        let conn = &*self.storage.conn().await;
        let entity = repo::entity_get(conn, entity_id).await?;
        if entity.name.iter().any(|n| n.lang == lang && n.full == full) {
            return Err(Error::from(format!(
                "命名 ({:?}, {}) 已存在于该实体中",
                lang, full
            )));
        }
        let nom = Nomenclature {
            id: Uuid::new_v4(),
            lang,
            full: full.clone(),
            abbr,
        };
        repo::entity_add_nomenclature(conn, entity_id, &nom).await?;
        Ok(repo::entity_get(conn, entity_id).await?)
    }

    pub async fn update_nomenclature(
        &self,
        entity_id: Uuid,
        nomenclature_id: Uuid,
        lang: Language,
        full: String,
        abbr: Option<String>,
    ) -> Result<Entity> {
        let conn = &*self.storage.conn().await;
        let entity = repo::entity_get(conn, entity_id).await?;
        if !entity.name.iter().any(|n| n.id == nomenclature_id) {
            return Err("该 nomenclature 不属于此实体".into());
        }
        if entity
            .name
            .iter()
            .any(|n| n.id != nomenclature_id && n.lang == lang && n.full == full)
        {
            return Err(Error::from(format!(
                "命名 ({:?}, {}) 已存在于该实体的另一条记录中",
                lang, full
            )));
        }
        let nom = Nomenclature {
            id: nomenclature_id,
            lang,
            full,
            abbr,
        };
        repo::entity_update_nomenclature(conn, entity_id, &nom).await?;
        Ok(repo::entity_get(conn, entity_id).await?)
    }

    pub async fn delete_nomenclature(
        &self,
        entity_id: Uuid,
        nomenclature_id: Uuid,
    ) -> Result<Entity> {
        let conn = &*self.storage.conn().await;
        let entity = repo::entity_get(conn, entity_id).await?;
        if entity.name.len() <= 1 {
            return Err("实体至少需要保留一条命名".into());
        }
        if !entity.name.iter().any(|n| n.id == nomenclature_id) {
            return Err("该 nomenclature 不属于此实体".into());
        }
        repo::entity_delete_nomenclature(conn, nomenclature_id).await?;
        Ok(repo::entity_get(conn, entity_id).await?)
    }

    pub async fn search_entity(&self, keyword: &str) -> Result<Vec<Entity>> {
        Ok(repo::entity_search_by_name(&*self.storage.conn().await, keyword).await?)
    }

    pub async fn list_entities(&self, filter: EntityFilter) -> Result<Vec<Entity>> {
        let all = repo::entity_list_all(&*self.storage.conn().await).await?;
        match filter {
            EntityFilter::All => Ok(all),
            EntityFilter::EmptyDefinition => Ok(all
                .into_iter()
                .filter(|e| e.definition.is_empty())
                .collect()),
            EntityFilter::NoNomenclature => {
                Ok(all.into_iter().filter(|e| e.name.is_empty()).collect())
            }
        }
    }

    pub async fn update_entity_by_ref(
        &self,
        name_ref: &str,
        new_definition: Option<&str>,
        new_names: Option<Vec<Nomenclature>>,
    ) -> Result<Entity> {
        let id = self.resolve(name_ref).await?;
        self.update_entity(id, new_definition, new_names).await
    }

    pub async fn update_entity_by_id(
        &self,
        id: Uuid,
        new_definition: Option<&str>,
        new_names: Option<Vec<Nomenclature>>,
    ) -> Result<Entity> {
        self.update_entity(id, new_definition, new_names).await
    }

    async fn update_entity(
        &self,
        id: Uuid,
        new_definition: Option<&str>,
        new_names: Option<Vec<Nomenclature>>,
    ) -> Result<Entity> {
        let conn = &*self.storage.conn().await;
        let mut entity = repo::entity_get(conn, id).await?;
        if let Some(definition) = new_definition {
            entity.definition = definition.to_string();
        }
        if let Some(names) = new_names {
            entity.name = names;
        }
        repo::entity_update(conn, &entity).await?;
        Ok(entity)
    }
}
