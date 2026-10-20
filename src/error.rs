use thiserror::Error;

pub type Result<T> = std::result::Result<T, WebMorphError>;

#[derive(Debug, Error)]
pub enum WebMorphError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("TLS negotiation failed: {0}")]
    Tls(String),
    #[error("DNS lookup failed: {0}")]
    Dns(String),
    #[error("request timed out")]
    Timeout,
    #[error("unsupported response content: {0}")]
    UnsupportedContent(String),
    #[error("HTTP status {0}")]
    HttpStatus(u16),
    #[error("site requires authentication (HTTP 401); WebMorph does not bypass sign-in controls")]
    AuthenticationRequired,
    #[error(
        "access denied (HTTP 403); the site may require authorization or block automated access"
    )]
    AccessDenied,
    #[error("site rate-limited this request (HTTP 429); retry later")]
    RateLimited,
    #[error(
        "redirect could not be followed (HTTP status {0}); the redirect may exceed limits or violate network policy"
    )]
    Redirect(u16),
    #[error("response exceeded the {limit} byte limit")]
    ResponseTooLarge { limit: usize },
    #[error("request cancelled")]
    Cancelled,
    #[error("terminal operation failed: {0}")]
    Terminal(#[from] std::io::Error),
    #[error("browser storage failed: {0}")]
    Storage(#[from] crate::storage::StorageError),
    #[error("Chromium is unavailable: {0}")]
    BrowserUnavailable(String),
    #[error("Chromium engine failed: {0}")]
    Browser(String),
}
