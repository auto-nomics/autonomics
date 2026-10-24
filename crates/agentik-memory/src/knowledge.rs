pub struct KnowledgePath(Vec<String>);

impl KnowledgePath {
    #[must_use]
    pub fn new(segments: impl Into<Vec<String>>) -> Self {
        Self(segments.into())
    }

    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.0
    }
}

pub struct Knowledge {
    id: uuid::Uuid,
    path: KnowledgePath,
    content: String,
}

impl Knowledge {
    #[must_use]
    pub fn new(id: uuid::Uuid, path: KnowledgePath, content: String) -> Self {
        Self { id, path, content }
    }

    #[must_use]
    pub const fn id(&self) -> uuid::Uuid {
        self.id
    }

    #[must_use]
    pub fn path(&self) -> &KnowledgePath {
        &self.path
    }

    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }
}
