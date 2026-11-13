use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("integrity check failed")]
    Integrity,
    #[error("restore rejected: {0}")]
    Restore(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("ECU rejected service 0x{service:02X}: 0x{code:02X} ({message})")]
    NegativeResponse {
        service: u8,
        code: u8,
        message: String,
    },
}

pub type Result<T> = std::result::Result<T, CoreError>;
