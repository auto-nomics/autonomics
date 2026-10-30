use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: uuid::Uuid,
    pub title: String,
    pub alias: Vec<String>,
    pub content: String,
    pub ctime: DateTime<Utc>,
    pub mtime: DateTime<Utc>,
    pub in_links: Vec<uuid::Uuid>,
    pub out_links: Vec<uuid::Uuid>,
}
