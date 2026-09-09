#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceQuery {
    pub filters: Vec<(String, String)>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub order_by: Option<String>,
    pub only: Vec<String>,
}

impl ResourceQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    pub fn filter(mut self, field: impl Into<String>, value: impl Into<String>) -> Self {
        self.filters.push((field.into(), value.into()));
        self
    }

    pub fn order_by(mut self, field: impl Into<String>) -> Self {
        self.order_by = Some(field.into());
        self
    }

    pub fn only<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.only = fields.into_iter().map(Into::into).collect();
        self
    }

    pub(crate) fn query_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = self.filters.clone();
        if let Some(order_by) = &self.order_by {
            pairs.push(("order_by".into(), order_by.clone()));
        }
        if let Some(limit) = self.limit {
            pairs.push(("limit".into(), limit.to_string()));
        }
        if let Some(offset) = self.offset {
            pairs.push(("offset".into(), offset.to_string()));
        }
        if !self.only.is_empty() {
            pairs.push(("only".into(), self.only.join(",")));
        }
        pairs
    }
}
