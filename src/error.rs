use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("operation cancelled")]
    Cancelled,
    #[error("invalid publisher signature")]
    Signature,
    #[error("invalid update: {0}")]
    Invalid(String),
    #[error("unsafe path: {0}")]
    UnsafePath(String),
    #[error("download or extraction exceeds its configured limit")]
    SizeLimit,
    #[error("hash mismatch: {0}")]
    HashMismatch(String),
    #[error("another update owns the installation lock")]
    Busy,
    #[error("helper failed: {0}")]
    Helper(String),
    #[error("installation requires recovery: {0}")]
    Recovery(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}
