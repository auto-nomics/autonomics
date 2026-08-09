//! Repository traits + Turso-backed implementations.
//!
//! Ported from dendrite's sqlx-based `sqlite.rs`. The trait shapes are
//! preserved; the implementations rewrite every query to the Turso
//! (`turso::Connection`) API. Row parsing is manual since Turso does not
//! offer a `FromRow` derive.

use std::collections::HashMap;

use turso::{Value, params_from_iter};
use uuid::Uuid;

use crate::language::Language;
use crate::storage::error::StorageError;
use crate::storage::types::{Entity, Index, Knowledge, KnowledgeType, Nomenclature, TargetType};

// ─────────────────────────── row helpers ───────────────────────────

fn text_col(row: &turso::Row, idx: usize) -> Result<String, StorageError> {
    match row.get_value(idx)? {
        Value::Text(s) => Ok(s),
        Value::Null => Ok(String::new()),
        other => Err(StorageError::Other(format!(
            "expected TEXT at column {idx}, got {other:?}"
        ))),
    }
}

fn opt_text_col(row: &turso::Row, idx: usize) -> Result<Option<String>, StorageError> {
    match row.get_value(idx)? {
        Value::Text(s) if !s.is_empty() => Ok(Some(s)),
        _ => Ok(None),
    }
}

fn int_col(row: &turso::Row, idx: usize) -> Result<i64, StorageError> {
    match row.get_value(idx)? {
        Value::Integer(v) => Ok(v),
        other => Err(StorageError::Other(format!(
            "expected INTEGER at column {idx}, got {other:?}"
        ))),
    }
}

fn opt_int_col(row: &turso::Row, idx: usize) -> Result<Option<i64>, StorageError> {
    match row.get_value(idx)? {
        Value::Integer(v) => Ok(Some(v)),
        _ => Ok(None),
    }
}

fn opt_uuid_col(row: &turso::Row, idx: usize) -> Result<Option<Uuid>, StorageError> {
    match row.get_value(idx)? {
        Value::Text(s) if !s.is_empty() => Ok(Some(Uuid::parse_str(&s)?)),
        _ => Ok(None),
    }
}

fn parse_uuid(s: &str) -> Result<Uuid, StorageError> {
    Uuid::parse_str(s).map_err(StorageError::from)
}

fn v(uuid: Uuid) -> Value {
    Value::Text(uuid.to_string())
}

fn opt_v(opt: Option<Uuid>) -> Value {
    match opt {
        Some(id) => Value::Text(id.to_string()),
        None => Value::Null,
    }
}

/// Collect all remaining rows from a `turso::Rows` cursor, applying a
/// mapping function. Drains the cursor completely.
async fn collect_rows<T, F>(rows: &mut turso::Rows, map: F) -> Result<Vec<T>, StorageError>
where
    F: Fn(&turso::Row) -> Result<T, StorageError>,
{
    let mut out = Vec::new();
    loop {
        match rows.next().await {
            Ok(Some(row)) => out.push(map(&row)?),
            Ok(None) => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

// ─────────────────────────── types for local-view ───────────────────────────

/// Raw ancestor path row, ordered from the requested node up to the root.
#[derive(Debug, Clone)]
pub struct AncestorRow {
    pub id: String,
    pub title: Option<String>,
    pub target: Option<String>,
    pub target_type: Option<String>,
    pub parent_id: Option<String>,
    pub position: i64,
    pub depth: i64,
}

/// Lightweight child row for projection into `IndexView`.
#[derive(Debug, Clone)]
pub struct ChildRow {
    pub id: String,
    pub title: Option<String>,
    pub target_type: Option<String>,
    pub position: i64,
}

/// Aggregate statistics about a subtree.
#[derive(Debug, Clone, Default)]
pub struct SubtreeStatsRow {
    pub total_nodes: usize,
    pub knowledge_count: usize,
    pub group_count: usize,
    pub max_depth: usize,
    pub knowledge_titles: Vec<String>,
    pub truncated: bool,
}

// ═══════════════════════════════════════════════════════════════════
// Entity repo — free functions taking `&turso::Connection`.
// ═══════════════════════════════════════════════════════════════════

pub async fn entity_create(
    conn: &turso::Connection,
    entity: &Entity,
) -> Result<Uuid, StorageError> {
    // Wrap entity + nomenclatures in a transaction so a partial failure
    // (e.g. UNIQUE constraint on nomenclature) rolls back the entity row too.
    conn.execute("BEGIN", ()).await?;

    let result = entity_create_inner(conn, entity).await;
    match result {
        Ok(id) => {
            conn.execute("COMMIT", ()).await?;
            Ok(id)
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(e)
        }
    }
}

async fn entity_create_inner(
    conn: &turso::Connection,
    entity: &Entity,
) -> Result<Uuid, StorageError> {
    conn.execute(
        "INSERT INTO entities (id, definition) VALUES (?1, ?2)",
        params_from_iter([v(entity.id), Value::Text(entity.definition.clone())]),
    )
    .await?;

    for nom in &entity.name {
        conn.execute(
            "INSERT INTO nomenclatures (id, entity_id, lang, full, abbr) VALUES (?1, ?2, ?3, ?4, ?5)",
            params_from_iter([
                v(nom.id),
                v(entity.id),
                Value::Text(nom.lang.as_str().to_string()),
                Value::Text(nom.full.clone()),
                nom.abbr.clone().map(Value::Text).unwrap_or(Value::Null),
            ]),
        )
        .await?;
    }
    Ok(entity.id)
}

pub async fn entity_get(conn: &turso::Connection, id: Uuid) -> Result<Entity, StorageError> {
    let mut rows = conn
        .query(
            "SELECT id, definition FROM entities WHERE id = ?1",
            params_from_iter([v(id)]),
        )
        .await?;

    let (definition,) = match rows.next().await? {
        Some(row) => (text_col(&row, 1)?,),
        None => return Err(StorageError::NotFound(id)),
    };

    let nomenclatures = fetch_nomenclatures(conn, id).await?;
    Ok(Entity {
        id,
        name: nomenclatures,
        definition,
    })
}

async fn fetch_nomenclatures(
    conn: &turso::Connection,
    entity_id: Uuid,
) -> Result<Vec<Nomenclature>, StorageError> {
    let mut rows = conn
        .query(
            "SELECT id, lang, full, abbr FROM nomenclatures WHERE entity_id = ?1",
            params_from_iter([v(entity_id)]),
        )
        .await?;

    let mut noms = Vec::new();
    loop {
        match rows.next().await? {
            Some(row) => {
                let id = parse_uuid(&text_col(&row, 0)?)?;
                let lang = Language::from_str(&text_col(&row, 1)?);
                let full = text_col(&row, 2)?;
                let abbr = opt_text_col(&row, 3)?;
                noms.push(Nomenclature {
                    id,
                    lang,
                    full,
                    abbr,
                });
            }
            None => break,
        }
    }
    Ok(noms)
}

pub async fn entity_list_all(conn: &turso::Connection) -> Result<Vec<Entity>, StorageError> {
    let mut rows = conn
        .query(
            "SELECT id, definition FROM entities",
            params_from_iter([] as [Value; 0]),
        )
        .await?;

    let entities: Vec<(Uuid, String)> = collect_rows(&mut rows, |row| {
        Ok((parse_uuid(&text_col(row, 0)?)?, text_col(row, 1)?))
    })
    .await?;

    // Batch-fetch all nomenclatures in one query.
    let mut nom_rows = conn
        .query(
            "SELECT id, entity_id, lang, full, abbr FROM nomenclatures",
            params_from_iter([] as [Value; 0]),
        )
        .await?;

    let mut nom_map: HashMap<Uuid, Vec<Nomenclature>> = HashMap::new();
    loop {
        match nom_rows.next().await? {
            Some(row) => {
                let nom_id = parse_uuid(&text_col(&row, 0)?)?;
                let entity_id = parse_uuid(&text_col(&row, 1)?)?;
                let lang = Language::from_str(&text_col(&row, 2)?);
                let full = text_col(&row, 3)?;
                let abbr = opt_text_col(&row, 4)?;
                nom_map.entry(entity_id).or_default().push(Nomenclature {
                    id: nom_id,
                    lang,
                    full,
                    abbr,
                });
            }
            None => break,
        }
    }

    Ok(entities
        .into_iter()
        .map(|(id, definition)| Entity {
            id,
            name: nom_map.remove(&id).unwrap_or_default(),
            definition,
        })
        .collect())
}

pub async fn entity_search_by_name(
    conn: &turso::Connection,
    keyword: &str,
) -> Result<Vec<Entity>, StorageError> {
    let pattern = format!("{keyword}%");
    let mut rows = conn
        .query(
            "SELECT DISTINCT e.id, e.definition \
             FROM entities e JOIN nomenclatures n ON n.entity_id = e.id \
             WHERE n.full LIKE ?1",
            params_from_iter([Value::Text(pattern)]),
        )
        .await?;

    let entity_ids: Vec<(Uuid, String)> = collect_rows(&mut rows, |row| {
        Ok((parse_uuid(&text_col(row, 0)?)?, text_col(row, 1)?))
    })
    .await?;

    let mut entities = Vec::with_capacity(entity_ids.len());
    for (id, definition) in entity_ids {
        let name = fetch_nomenclatures(conn, id).await?;
        entities.push(Entity {
            id,
            name,
            definition,
        });
    }
    Ok(entities)
}

pub async fn entity_find_by_exact_name(
    conn: &turso::Connection,
    name: &str,
) -> Result<Option<Entity>, StorageError> {
    let mut rows = conn
        .query(
            "SELECT DISTINCT e.id, e.definition \
             FROM entities e JOIN nomenclatures n ON n.entity_id = e.id \
             WHERE n.full = ?1 LIMIT 1",
            params_from_iter([Value::Text(name.to_string())]),
        )
        .await?;

    match rows.next().await? {
        Some(row) => {
            let id = parse_uuid(&text_col(&row, 0)?)?;
            let definition = text_col(&row, 1)?;
            let nomenclatures = fetch_nomenclatures(conn, id).await?;
            Ok(Some(Entity {
                id,
                name: nomenclatures,
                definition,
            }))
        }
        None => Ok(None),
    }
}

pub async fn entity_update(conn: &turso::Connection, entity: &Entity) -> Result<(), StorageError> {
    conn.execute("BEGIN", ()).await?;

    let result = entity_update_inner(conn, entity).await;
    match result {
        Ok(()) => {
            conn.execute("COMMIT", ()).await?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(e)
        }
    }
}

async fn entity_update_inner(
    conn: &turso::Connection,
    entity: &Entity,
) -> Result<(), StorageError> {
    let affected = conn
        .execute(
            "UPDATE entities SET definition = ?1 WHERE id = ?2",
            params_from_iter([Value::Text(entity.definition.clone()), v(entity.id)]),
        )
        .await?;

    if affected == 0 {
        return Err(StorageError::NotFound(entity.id));
    }

    // Replace all nomenclatures: delete old, insert new.
    conn.execute(
        "DELETE FROM nomenclatures WHERE entity_id = ?1",
        params_from_iter([v(entity.id)]),
    )
    .await?;

    for nom in &entity.name {
        conn.execute(
            "INSERT INTO nomenclatures (id, entity_id, lang, full, abbr) VALUES (?1, ?2, ?3, ?4, ?5)",
            params_from_iter([
                v(nom.id),
                v(entity.id),
                Value::Text(nom.lang.as_str().to_string()),
                Value::Text(nom.full.clone()),
                nom.abbr.clone().map(Value::Text).unwrap_or(Value::Null),
            ]),
        )
        .await?;
    }
    Ok(())
}

pub async fn entity_delete(conn: &turso::Connection, id: Uuid) -> Result<(), StorageError> {
    let affected = conn
        .execute(
            "DELETE FROM entities WHERE id = ?1",
            params_from_iter([v(id)]),
        )
        .await?;
    if affected == 0 {
        return Err(StorageError::NotFound(id));
    }
    Ok(())
}

pub async fn entity_add_nomenclature(
    conn: &turso::Connection,
    entity_id: Uuid,
    nom: &Nomenclature,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO nomenclatures (id, entity_id, lang, full, abbr) VALUES (?1, ?2, ?3, ?4, ?5)",
        params_from_iter([
            v(nom.id),
            v(entity_id),
            Value::Text(nom.lang.as_str().to_string()),
            Value::Text(nom.full.clone()),
            nom.abbr.clone().map(Value::Text).unwrap_or(Value::Null),
        ]),
    )
    .await?;
    Ok(())
}

pub async fn entity_update_nomenclature(
    conn: &turso::Connection,
    entity_id: Uuid,
    nom: &Nomenclature,
) -> Result<(), StorageError> {
    let affected = conn
        .execute(
            "UPDATE nomenclatures SET lang = ?1, full = ?2, abbr = ?3 WHERE id = ?4 AND entity_id = ?5",
            params_from_iter([
                Value::Text(nom.lang.as_str().to_string()),
                Value::Text(nom.full.clone()),
                nom.abbr.clone().map(Value::Text).unwrap_or(Value::Null),
                v(nom.id),
                v(entity_id),
            ]),
        )
        .await?;
    if affected == 0 {
        return Err(StorageError::NotFound(nom.id));
    }
    Ok(())
}

pub async fn entity_delete_nomenclature(
    conn: &turso::Connection,
    nomenclature_id: Uuid,
) -> Result<(), StorageError> {
    let affected = conn
        .execute(
            "DELETE FROM nomenclatures WHERE id = ?1",
            params_from_iter([v(nomenclature_id)]),
        )
        .await?;
    if affected == 0 {
        return Err(StorageError::NotFound(nomenclature_id));
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════
// Knowledge repo — free functions.
// ═══════════════════════════════════════════════════════════════════

fn row_to_knowledge(row: &turso::Row) -> Result<Knowledge, StorageError> {
    let id = parse_uuid(&text_col(row, 0)?)?;
    let title = text_col(row, 1)?;
    let knowledge_type = KnowledgeType::convert_from_str(&text_col(row, 2)?);
    let entities_str = text_col(row, 3)?;
    let entities: Vec<Uuid> = entities_str
        .split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect();
    let content = opt_text_col(row, 4)?;
    let source_document_id = opt_uuid_col(row, 5)?;
    let source_chunk_idx = opt_int_col(row, 6)?;
    Ok(Knowledge {
        id,
        title,
        knowledge_type,
        entities,
        content,
        source_document_id,
        source_chunk_idx,
    })
}

const KNOWLEDGE_COLS: &str =
    "id, title, knowledge_type, entities, content, source_document_id, source_chunk_idx";

pub async fn knowledge_create(
    conn: &turso::Connection,
    knowledge: &Knowledge,
) -> Result<Uuid, StorageError> {
    let entities_str = knowledge
        .entities
        .iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join(",");

    conn.execute(
        &format!("INSERT INTO knowledges ({KNOWLEDGE_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
        params_from_iter([
            v(knowledge.id),
            Value::Text(knowledge.title.clone()),
            Value::Text(knowledge.knowledge_type.as_str().to_string()),
            Value::Text(entities_str),
            knowledge
                .content
                .clone()
                .map(Value::Text)
                .unwrap_or(Value::Null),
            opt_v(knowledge.source_document_id),
            knowledge
                .source_chunk_idx
                .map(Value::Integer)
                .unwrap_or(Value::Null),
        ]),
    )
    .await?;
    Ok(knowledge.id)
}

pub async fn knowledge_get(conn: &turso::Connection, id: Uuid) -> Result<Knowledge, StorageError> {
    let mut rows = conn
        .query(
            &format!("SELECT {KNOWLEDGE_COLS} FROM knowledges WHERE id = ?1"),
            params_from_iter([v(id)]),
        )
        .await?;

    match rows.next().await? {
        Some(row) => row_to_knowledge(&row),
        None => Err(StorageError::NotFound(id)),
    }
}

pub async fn knowledge_list_all(conn: &turso::Connection) -> Result<Vec<Knowledge>, StorageError> {
    let mut rows = conn
        .query(
            &format!("SELECT {KNOWLEDGE_COLS} FROM knowledges"),
            params_from_iter([] as [Value; 0]),
        )
        .await?;
    collect_rows(&mut rows, row_to_knowledge).await
}

pub async fn knowledge_find_by_title(
    conn: &turso::Connection,
    title: &str,
) -> Result<Option<Knowledge>, StorageError> {
    let mut rows = conn
        .query(
            &format!("SELECT {KNOWLEDGE_COLS} FROM knowledges WHERE title = ?1 LIMIT 1"),
            params_from_iter([Value::Text(title.to_string())]),
        )
        .await?;

    match rows.next().await? {
        Some(row) => Ok(Some(row_to_knowledge(&row)?)),
        None => Ok(None),
    }
}

pub async fn knowledge_find_by_entity(
    conn: &turso::Connection,
    entity_id: Uuid,
) -> Result<Vec<Knowledge>, StorageError> {
    // SQLite `entities` column is a comma-joined UUID list; filter in Rust
    // after fetching all rows (same approach as the original dendrite code).
    let all = knowledge_list_all(conn).await?;
    Ok(all
        .into_iter()
        .filter(|k| k.entities.contains(&entity_id))
        .collect())
}

pub async fn knowledge_update(
    conn: &turso::Connection,
    knowledge: &Knowledge,
) -> Result<(), StorageError> {
    let entities_str = knowledge
        .entities
        .iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let affected = conn
        .execute(
            "UPDATE knowledges SET title = ?1, knowledge_type = ?2, entities = ?3, \
             content = ?4, source_document_id = ?5, source_chunk_idx = ?6 WHERE id = ?7",
            params_from_iter([
                Value::Text(knowledge.title.clone()),
                Value::Text(knowledge.knowledge_type.as_str().to_string()),
                Value::Text(entities_str),
                knowledge
                    .content
                    .clone()
                    .map(Value::Text)
                    .unwrap_or(Value::Null),
                opt_v(knowledge.source_document_id),
                knowledge
                    .source_chunk_idx
                    .map(Value::Integer)
                    .unwrap_or(Value::Null),
                v(knowledge.id),
            ]),
        )
        .await?;
    if affected == 0 {
        return Err(StorageError::NotFound(knowledge.id));
    }
    Ok(())
}

pub async fn knowledge_delete(conn: &turso::Connection, id: Uuid) -> Result<(), StorageError> {
    let affected = conn
        .execute(
            "DELETE FROM knowledges WHERE id = ?1",
            params_from_iter([v(id)]),
        )
        .await?;
    if affected == 0 {
        return Err(StorageError::NotFound(id));
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════
// Index repo — free functions.
// ═══════════════════════════════════════════════════════════════════

const INDEX_COLS: &str = "id, title, target, target_type, parent_id, position";

fn row_to_index(row: &turso::Row) -> Result<Index, StorageError> {
    let id = parse_uuid(&text_col(row, 0)?)?;
    let title = opt_text_col(row, 1)?;
    let target = opt_uuid_col(row, 2)?;
    let target_type = TargetType::from_str(&text_col(row, 3)?);
    let parent_id = opt_uuid_col(row, 4)?;
    let position = int_col(row, 5)?;
    Ok(Index {
        id,
        title,
        target,
        target_type,
        parent_id,
        position,
    })
}

pub async fn index_create(conn: &turso::Connection, entry: &Index) -> Result<Uuid, StorageError> {
    conn.execute(
        "INSERT INTO indexes (id, title, target, target_type, parent_id, position) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params_from_iter([
            v(entry.id),
            entry.title.clone().map(Value::Text).unwrap_or(Value::Null),
            opt_v(entry.target),
            Value::Text(entry.target_type.as_str().to_string()),
            opt_v(entry.parent_id),
            Value::Integer(entry.position),
        ]),
    )
    .await?;
    Ok(entry.id)
}

pub async fn index_get(conn: &turso::Connection, id: Uuid) -> Result<Index, StorageError> {
    let mut rows = conn
        .query(
            &format!("SELECT {INDEX_COLS} FROM indexes WHERE id = ?1"),
            params_from_iter([v(id)]),
        )
        .await?;
    match rows.next().await? {
        Some(row) => row_to_index(&row),
        None => Err(StorageError::NotFound(id)),
    }
}

pub async fn index_list_all(conn: &turso::Connection) -> Result<Vec<Index>, StorageError> {
    let mut rows = conn
        .query(
            &format!("SELECT {INDEX_COLS} FROM indexes"),
            params_from_iter([] as [Value; 0]),
        )
        .await?;
    collect_rows(&mut rows, row_to_index).await
}

pub async fn index_find_by_title(
    conn: &turso::Connection,
    title: &str,
) -> Result<Option<Index>, StorageError> {
    let mut rows = conn
        .query(
            &format!("SELECT {INDEX_COLS} FROM indexes WHERE title = ?1 LIMIT 1"),
            params_from_iter([Value::Text(title.to_string())]),
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row_to_index(&row)?)),
        None => Ok(None),
    }
}

pub async fn index_find_root(conn: &turso::Connection) -> Result<Index, StorageError> {
    let mut rows = conn
        .query(
            &format!("SELECT {INDEX_COLS} FROM indexes WHERE parent_id IS NULL LIMIT 1"),
            params_from_iter([] as [Value; 0]),
        )
        .await?;
    match rows.next().await? {
        Some(row) => row_to_index(&row),
        None => Err(StorageError::NotFound(Uuid::nil())),
    }
}

pub async fn index_update(conn: &turso::Connection, entry: &Index) -> Result<(), StorageError> {
    let affected = conn
        .execute(
            "UPDATE indexes SET title = ?1, target = ?2, target_type = ?3, \
             parent_id = ?4, position = ?5 WHERE id = ?6",
            params_from_iter([
                entry.title.clone().map(Value::Text).unwrap_or(Value::Null),
                opt_v(entry.target),
                Value::Text(entry.target_type.as_str().to_string()),
                opt_v(entry.parent_id),
                Value::Integer(entry.position),
                v(entry.id),
            ]),
        )
        .await?;
    if affected == 0 {
        return Err(StorageError::NotFound(entry.id));
    }
    Ok(())
}

pub async fn index_delete(conn: &turso::Connection, id: Uuid) -> Result<(), StorageError> {
    let affected = conn
        .execute(
            "DELETE FROM indexes WHERE id = ?1",
            params_from_iter([v(id)]),
        )
        .await?;
    if affected == 0 {
        return Err(StorageError::NotFound(id));
    }
    Ok(())
}

pub async fn index_children_of(
    conn: &turso::Connection,
    parent_id: Option<Uuid>,
) -> Result<Vec<Index>, StorageError> {
    let mut rows = match parent_id {
        Some(pid) => {
            conn.query(
                &format!("SELECT {INDEX_COLS} FROM indexes WHERE parent_id = ?1 ORDER BY position"),
                params_from_iter([v(pid)]),
            )
            .await?
        }
        None => {
            conn.query(
                &format!(
                    "SELECT {INDEX_COLS} FROM indexes WHERE parent_id IS NULL ORDER BY position"
                ),
                params_from_iter([] as [Value; 0]),
            )
            .await?
        }
    };
    collect_rows(&mut rows, row_to_index).await
}

pub async fn index_subtree_knowledge_ids(
    conn: &turso::Connection,
    index_id: Uuid,
) -> Result<Vec<Uuid>, StorageError> {
    // Turso (limbo) does not support `WITH RECURSIVE`, so we traverse
    // the subtree iteratively: BFS via a Rust-side stack.
    let mut ids = Vec::new();
    let mut stack = vec![index_id];
    while let Some(node_id) = stack.pop() {
        let children = index_children_of(conn, Some(node_id)).await?;
        for child in &children {
            if child.target_type == TargetType::Knowledge {
                if let Some(target) = child.target {
                    ids.push(target);
                }
            }
            stack.push(child.id);
        }
    }
    Ok(ids)
}

pub async fn index_reparent(
    conn: &turso::Connection,
    id: Uuid,
    new_parent_id: Uuid,
    position: i64,
) -> Result<(), StorageError> {
    // Cycle check: refuse if new_parent_id is a descendant of id.
    // Turso (limbo) does not support `WITH RECURSIVE`, so we walk
    // descendants iteratively.
    let mut stack = vec![id];
    while let Some(node_id) = stack.pop() {
        let children = index_children_of(conn, Some(node_id)).await?;
        for child in &children {
            if child.id == new_parent_id {
                return Err(StorageError::Other(
                    "cannot reparent: new_parent is a descendant of id".into(),
                ));
            }
            stack.push(child.id);
        }
    }

    let affected = conn
        .execute(
            "UPDATE indexes SET parent_id = ?1, position = ?2 WHERE id = ?3",
            params_from_iter([v(new_parent_id), Value::Integer(position), v(id)]),
        )
        .await?;
    if affected == 0 {
        return Err(StorageError::NotFound(id));
    }
    Ok(())
}

pub async fn index_reindex_positions(
    conn: &turso::Connection,
    parent_id: Option<Uuid>,
) -> Result<(), StorageError> {
    let children = index_children_of(conn, parent_id).await?;
    for (i, child) in children.into_iter().enumerate() {
        conn.execute(
            "UPDATE indexes SET position = ?1 WHERE id = ?2",
            params_from_iter([Value::Integer(i as i64), v(child.id)]),
        )
        .await?;
    }
    Ok(())
}

pub async fn index_orphan_knowledge_titles(
    conn: &turso::Connection,
) -> Result<Vec<String>, StorageError> {
    let mut rows = conn
        .query(
            "SELECT k.title FROM knowledges k \
             WHERE k.title NOT IN ( \
                 SELECT i2.title FROM indexes i2 \
                 WHERE i2.target_type = 'knowledge' AND i2.target IS NOT NULL \
             )",
            params_from_iter([] as [Value; 0]),
        )
        .await?;
    collect_rows(&mut rows, |row| text_col(row, 0)).await
}

pub async fn index_find_by_target(
    conn: &turso::Connection,
    target_id: Uuid,
) -> Result<Vec<Index>, StorageError> {
    let mut rows = conn
        .query(
            &format!("SELECT {INDEX_COLS} FROM indexes WHERE target = ?1"),
            params_from_iter([v(target_id)]),
        )
        .await?;
    collect_rows(&mut rows, row_to_index).await
}

pub async fn index_downgrade_to_group(
    conn: &turso::Connection,
    id: Uuid,
) -> Result<(), StorageError> {
    let affected = conn
        .execute(
            "UPDATE indexes SET target_type = 'group', target = NULL WHERE id = ?1",
            params_from_iter([v(id)]),
        )
        .await?;
    if affected == 0 {
        return Err(StorageError::NotFound(id));
    }
    Ok(())
}

// ── local-view (stateless) primitives ──

pub async fn index_ancestor_path_rows(
    conn: &turso::Connection,
    node_id: Uuid,
) -> Result<Vec<AncestorRow>, StorageError> {
    // Turso (limbo) does not support `WITH RECURSIVE`, so we walk
    // up the parent chain iteratively.
    let mut path = Vec::new();
    let mut current = Some(node_id);
    while let Some(id) = current {
        let node = index_get(conn, id).await?;
        path.push(AncestorRow {
            id: id.to_string(),
            title: node.title.clone(),
            target: node.target.map(|t| t.to_string()),
            target_type: Some(node.target_type.as_str().to_string()),
            parent_id: node.parent_id.map(|p| p.to_string()),
            position: node.position,
            depth: 0, // set below
        });
        current = node.parent_id;
    }
    // path is ordered node → root; assign depth 0, 1, 2, ...
    for (i, row) in path.iter_mut().enumerate() {
        row.depth = i as i64;
    }
    Ok(path)
}

pub async fn index_child_rows(
    conn: &turso::Connection,
    node_id: Uuid,
) -> Result<Vec<ChildRow>, StorageError> {
    let mut rows = conn
        .query(
            "SELECT id, title, target_type, position \
             FROM indexes WHERE parent_id = ?1 ORDER BY position",
            params_from_iter([v(node_id)]),
        )
        .await?;

    collect_rows(&mut rows, |row| {
        Ok(ChildRow {
            id: text_col(row, 0)?,
            title: opt_text_col(row, 1)?,
            target_type: opt_text_col(row, 2)?,
            position: int_col(row, 3)?,
        })
    })
    .await
}

pub async fn index_subtree_stats(
    conn: &turso::Connection,
    node_id: Uuid,
    title_limit: usize,
) -> Result<SubtreeStatsRow, StorageError> {
    // Turso (limbo) does not support `WITH RECURSIVE`, so we walk
    // the subtree iteratively (BFS), counting nodes and collecting
    // knowledge titles.
    let mut total_nodes = 0usize;
    let mut knowledge_count = 0usize;
    let mut group_count = 0usize;
    let mut max_depth = 0usize;
    let mut all_titles = Vec::new();

    // Stack of (node_id, depth). Start with the root of the subtree.
    let mut stack: Vec<(Uuid, usize)> = vec![(node_id, 0)];
    while let Some((id, depth)) = stack.pop() {
        total_nodes += 1;
        if depth > max_depth {
            max_depth = depth;
        }

        let node = index_get(conn, id).await?;
        match node.target_type {
            TargetType::Knowledge => {
                knowledge_count += 1;
                if let Some(target_id) = node.target {
                    if let Ok(k) = knowledge_get(conn, target_id).await {
                        all_titles.push(k.title);
                    }
                }
            }
            TargetType::Group => {
                group_count += 1;
            }
        }

        let children = index_children_of(conn, Some(id)).await?;
        for child in &children {
            stack.push((child.id, depth + 1));
        }
    }

    // Sort titles alphabetically (to match the original SQL ORDER BY).
    all_titles.sort();

    let truncated = all_titles.len() > title_limit;
    all_titles.truncate(title_limit);

    Ok(SubtreeStatsRow {
        total_nodes,
        knowledge_count,
        group_count,
        max_depth,
        knowledge_titles: all_titles,
        truncated,
    })
}

pub async fn index_sibling_count(
    conn: &turso::Connection,
    node_id: Uuid,
) -> Result<usize, StorageError> {
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM indexes \
             WHERE (parent_id IS NULL AND (SELECT parent_id FROM indexes WHERE id = ?1) IS NULL) \
                OR parent_id = (SELECT parent_id FROM indexes WHERE id = ?1)",
            params_from_iter([v(node_id)]),
        )
        .await?;

    let count = match rows.next().await? {
        Some(row) => int_col(&row, 0)?,
        None => 0,
    };
    Ok(count.max(0) as usize)
}
