use thiserror::Error;

pub type Result<T> = std::result::Result<T, WebMorphError>;

#[derive(Debug, Error)]
pub enum WebMorphError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("terminal operation failed: {0}")]
    Terminal(#[from] std::io::Error),
}
