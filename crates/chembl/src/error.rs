use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ChemblError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },

    #[error("ChEMBL rejected the request: {0}")]
    Api(String),

    #[error("deserialize error: {0}")]
    Deserialize(#[from] serde_json::Error),

    #[error("invalid resource or identifier: {0}")]
    InvalidPath(String),

    #[error("the response did not contain a record array: {0}")]
    UnexpectedResponse(Value),
}

pub type Result<T> = std::result::Result<T, ChemblError>;
