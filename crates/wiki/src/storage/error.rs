use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    /// 底层数据库(turso)错误:约束冲突、IO、SQL 语法等。
    #[error("database error: {0}")]
    Database(#[from] turso::Error),
    /// 目标条目不存在(`get_entry`/`update_entry` 找不到 id 时)。
    #[error("entry not found: {0}")]
    NotFound(String),
    /// 标题(trim 后精确匹配)已被占用;`existing_id` 是占用者,调用方
    /// 可据此提示「该标题已是词条 X」。
    #[error("duplicate title: {title} already used by entry {existing_id}")]
    DuplicateTitle { title: String, existing_id: String },
    /// 标题 trim 后为空,拒绝写入。
    #[error("title is empty after trim")]
    EmptyTitle,
    /// 行数据与预期形状不符(列类型错、`alias` 不是合法 JSON 等)。
    #[error("corrupt row: {0}")]
    CorruptRow(String),
    /// 兜底:业务规则或未归类错误,消息原样透出。
    #[error("{0}")]
    Unknown(String),
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Unknown(value.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::CorruptRow(format!("invalid JSON: {value}"))
    }
}
