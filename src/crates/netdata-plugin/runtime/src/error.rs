use std::fmt;

/// Result type for runtime operations
pub type Result<T> = std::result::Result<T, RuntimeError>;

/// Error types that can occur in the plugin runtime
#[derive(Debug)]
pub enum RuntimeError {
    /// Transport layer error
    Transport(Box<dyn std::error::Error + Send + Sync>),
    /// Function handler error
    FunctionHandler(String),
    /// Configuration error
    Config(String),
    /// I/O error
    Io(std::io::Error),
    /// Generic error
    Other(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::Transport(e) => write!(f, "Transport error: {}", e),
            RuntimeError::FunctionHandler(e) => write!(f, "Function handler error: {}", e),
            RuntimeError::Config(e) => write!(f, "Configuration error: {}", e),
            RuntimeError::Io(e) => write!(f, "I/O error: {}", e),
            RuntimeError::Other(e) => write!(f, "Error: {}", e),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RuntimeError::Transport(e) => Some(e.as_ref()),
            RuntimeError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for RuntimeError {
    fn from(err: std::io::Error) -> Self {
        RuntimeError::Io(err)
    }
}
