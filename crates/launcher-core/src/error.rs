use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("authentication is still pending")]
    AuthPending,

    #[error("this Microsoft account does not own Minecraft")]
    NoEntitlement,

    #[error("unknown Minecraft version: {0}")]
    UnknownVersion(String),

    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        path: String,
        expected: String,
        actual: String,
    },

    #[error("no suitable Java installation found (need major version {0}); set the java path in settings")]
    JavaNotFound(u32),

    #[error("instance not found: {0}")]
    InstanceNotFound(String),

    #[error("instance already exists: {0}")]
    InstanceExists(String),

    #[error("Modrinth error: {0}")]
    Modrinth(String),

    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("{0}")]
    Other(String),
}
