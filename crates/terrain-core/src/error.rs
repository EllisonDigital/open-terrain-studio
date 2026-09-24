use thiserror::Error;

/// Every error the terrain engine can report.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("unknown node type '{0}'")]
    UnknownNodeType(String),
    #[error("node '{0}' not found")]
    NodeNotFound(String),
    #[error("node '{node}' has no port '{port}'")]
    PortNotFound { node: String, port: String },
    #[error("cannot connect {from} to {to}: {reason}")]
    InvalidLink {
        from: String,
        to: String,
        reason: String,
    },
    #[error("connecting these nodes would create a cycle")]
    Cycle,
    #[error("node '{node}' needs an input on '{port}'")]
    MissingInput { node: String, port: String },
    #[error("invalid value for parameter '{param}': {reason}")]
    InvalidParam { param: String, reason: String },
    #[error("node '{node}' failed: {message}")]
    NodeFailed { node: String, message: String },
    #[error("invalid resolution {0} (must be between 2 and 16384)")]
    InvalidResolution(u32),
    #[error("evaluation cancelled")]
    Cancelled,
    #[error("project file error: {0}")]
    Project(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("image export error: {0}")]
    Image(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
