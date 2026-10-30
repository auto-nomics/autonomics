use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    /// 存储层错误原样透传(条目不存在、约束冲突、行损坏等)。
    #[error("{0}")]
    Storage(#[from] crate::storage::error::Error),
    /// worker 已不在:控制通道关闭(全部句柄已 drop)或回执丢失。
    #[error("wiki worker gone: {0}")]
    WorkerGone(String),
}
