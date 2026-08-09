use thiserror::Error;

#[derive(Debug, Error)]
pub enum HierIntError {
    #[error("empty input data")]
    Empty,
    #[error("dimension mismatch: {0}")]
    DimMismatch(String),
    #[error("invalid parameter: {0}")]
    InvalidParam(String),
    #[error("optimization did not converge in {max_iter} iterations")]
    NotConverged { max_iter: usize },
    #[error("numerical error: {0}")]
    Numerical(String),
}

pub type Result<T> = std::result::Result<T, HierIntError>;
