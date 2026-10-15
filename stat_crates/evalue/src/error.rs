use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvalueError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Compute(String),
}

pub type Result<T> = std::result::Result<T, EvalueError>;
