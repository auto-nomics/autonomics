use uuid::Uuid;

use crate::language::Language;

#[derive(Debug, Clone)]
pub struct Entity {
    pub id: Uuid,
    pub name: Vec<Nomenclature>,
    pub definition: String,
}

#[derive(Debug, Clone)]
pub struct Nomenclature {
    pub id: Uuid,
    pub lang: Language,
    pub full: String,
    pub abbr: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KnowledgeType {
    Aspect,
    Relation,
}

impl KnowledgeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            KnowledgeType::Aspect => "aspect",
            KnowledgeType::Relation => "relation",
        }
    }

    pub fn convert_from_str(s: &str) -> Self {
        match s {
            "relation" => KnowledgeType::Relation,
            _ => KnowledgeType::Aspect,
        }
    }
}

/// Knowledge data model.
#[derive(Debug, Clone)]
pub struct Knowledge {
    pub id: Uuid,

    /// UNIQUE constraint — no two Knowledge entries share the same title,
    /// so the agent can reference a title one-to-one.
    pub title: String,
    pub knowledge_type: KnowledgeType,
    pub entities: Vec<Uuid>,
    pub content: Option<String>,

    /// Optional source document ID (foreign key to a document store) for
    /// provenance tracking.
    pub source_document_id: Option<Uuid>,

    /// Optional source chunk index for finer-grained provenance.
    pub source_chunk_idx: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TargetType {
    Group,
    Knowledge,
}

impl TargetType {
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetType::Group => "group",
            TargetType::Knowledge => "knowledge",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "knowledge" => TargetType::Knowledge,
            _ => TargetType::Group,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Index {
    pub id: Uuid,
    pub title: Option<String>,
    pub target: Option<Uuid>,
    pub target_type: TargetType,
    pub parent_id: Option<Uuid>,
    pub position: i64,
}
