use crate::error::Error as EngineError;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("data engine server is not available")]
    ServerClosed,

    #[error(transparent)]
    Engine(#[from] EngineError),
}

pub type Result<T> = std::result::Result<T, ClientError>;
