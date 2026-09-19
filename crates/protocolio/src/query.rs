use crate::client::{MAX_PAGE_SIZE, MIN_PAGE_SIZE};
use crate::error::{ProtocolioError, Result};

#[derive(Debug, Clone, Default)]
pub struct ProtocolListQuery {
    pub filter: Option<String>,
    pub key: String,
    pub order_field: Option<String>,
    pub order_dir: Option<String>,
    pub fields: Option<String>,
    pub page_size: Option<u32>,
    pub page_id: Option<u32>,
    pub peer_reviewed: Option<bool>,
}

impl ProtocolListQuery {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<()> {
        nonempty(&self.key, "key")?;
        if let Some(filter) = self.filter.as_deref() {
            expect(
                filter,
                "filter",
                ["public", "user_public", "user_private", "shared_with_user"],
            )?;
        }
        if let Some(value) = self.order_field.as_deref() {
            expect(
                value,
                "order_field",
                ["activity", "relevance", "date", "name", "id"],
            )?;
        }
        if let Some(value) = self.order_dir.as_deref() {
            expect(value, "order_dir", ["asc", "desc"])?;
        }
        validate_page(self.page_size, self.page_id)
    }

    pub(crate) fn pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = Vec::new();
        push(&mut pairs, "filter", self.filter.as_deref());
        push(&mut pairs, "key", Some(&self.key));
        push(&mut pairs, "order_field", self.order_field.as_deref());
        push(&mut pairs, "order_dir", self.order_dir.as_deref());
        push(&mut pairs, "fields", self.fields.as_deref());
        if let Some(value) = self.page_size {
            pairs.push(("page_size", value.to_string()));
        }
        if let Some(value) = self.page_id {
            pairs.push(("page_id", value.to_string()));
        }
        if let Some(value) = self.peer_reviewed {
            pairs.push(("peer_reviewed", if value { "1" } else { "0" }.to_owned()));
        }
        pairs
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReagentListQuery {
    pub key: String,
    pub is_citeab: Option<bool>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub page_size: Option<u32>,
    pub page_id: Option<u32>,
}

impl ReagentListQuery {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<()> {
        nonempty(&self.key, "key")?;
        if let (Some(from), Some(to)) = (self.from, self.to) {
            if from > to {
                return Err(ProtocolioError::InvalidParameter(
                    "`from` must not be later than `to`".to_owned(),
                ));
            }
        }
        validate_page(self.page_size, self.page_id)
    }

    pub(crate) fn pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = Vec::new();
        push(&mut pairs, "key", Some(&self.key));
        if let Some(value) = self.is_citeab {
            pairs.push(("is_citeab", if value { "true" } else { "false" }.to_owned()));
        }
        if let Some(value) = self.from {
            pairs.push(("from", value.to_string()));
        }
        if let Some(value) = self.to {
            pairs.push(("to", value.to_string()));
        }
        if let Some(value) = self.page_size {
            pairs.push(("page_size", value.to_string()));
        }
        if let Some(value) = self.page_id {
            pairs.push(("page_id", value.to_string()));
        }
        pairs
    }
}

/// Selected sections for the independently rate-limited PDF export.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProtocolPdfView {
    #[default]
    Full,
    Compact,
    Materials,
    Commands,
    Steps,
}

impl ProtocolPdfView {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" | "" => Ok(Self::Full),
            "compact" | "compact_view" => Ok(Self::Compact),
            "materials" | "only_materials" => Ok(Self::Materials),
            "commands" | "only_commands" => Ok(Self::Commands),
            "steps" | "only_steps" => Ok(Self::Steps),
            _ => Err(ProtocolioError::InvalidParameter(format!(
                "unknown PDF view {value:?}"
            ))),
        }
    }

    pub(crate) fn pairs(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::Full => Vec::new(),
            Self::Compact => vec![("compact_view", "true".to_owned())],
            Self::Materials => vec![("only_materials", "true".to_owned())],
            Self::Commands => vec![("only_commands", "true".to_owned())],
            Self::Steps => vec![("only_steps", "true".to_owned())],
        }
    }
}

fn validate_page(page_size: Option<u32>, page_id: Option<u32>) -> Result<()> {
    if let Some(value) = page_size {
        if !(MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&value) {
            return Err(ProtocolioError::InvalidParameter(format!(
                "page_size must be between {MIN_PAGE_SIZE} and {MAX_PAGE_SIZE}"
            )));
        }
    }
    if let Some(value) = page_id {
        if value == 0 {
            return Err(ProtocolioError::InvalidParameter(
                "page_id must be greater than zero".to_owned(),
            ));
        }
    }
    Ok(())
}

fn nonempty(value: &str, name: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(ProtocolioError::InvalidParameter(format!(
            "{name} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn expect<const N: usize>(value: &str, name: &str, allowed: [&str; N]) -> Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(ProtocolioError::InvalidParameter(format!(
            "{name} must be one of {}",
            allowed.join(", ")
        )))
    }
}

fn push(pairs: &mut Vec<(&'static str, String)>, name: &'static str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        pairs.push((name, value.to_owned()));
    }
}
