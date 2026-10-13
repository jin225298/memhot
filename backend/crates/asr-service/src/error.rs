use thiserror::Error;

#[derive(Error, Debug)]
pub enum AsrError {
    #[error("ASR provider error: {0}")]
    ProviderError(String),

    #[error("ASR response truncated")]
    TruncatedResponse,

    #[error("Invalid ASR response: {0}")]
    InvalidResponse(String),

    #[error("Audio format not supported: {0}")]
    UnsupportedFormat(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Authentication failed: {0}")]
    AuthenticationError(String),

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("Service unavailable")]
    ServiceUnavailable,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, AsrError>;
