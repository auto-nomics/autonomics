use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] turso::Error),
    #[error("entity not found: {0}")]
    NotFound(Uuid),
    #[error("{0}")]
    Other(String),
}

impl From<uuid::Error> for StorageError {
    fn from(e: uuid::Error) -> Self {
        StorageError::Other(format!("invalid uuid: {e}"))
    }
}
