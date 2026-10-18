use reqwest::{Client, Url};
use tracing::debug;

use crate::error::{Result, WebMorphError};

#[derive(Clone)]
pub struct HttpClient {
    client: Client,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl HttpClient {
    pub fn parse_url(input: &str) -> Result<Url> {
        let url =
            Url::parse(input).map_err(|error| WebMorphError::InvalidUrl(error.to_string()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(WebMorphError::InvalidUrl(
                "only http and https URLs are supported".into(),
            ));
        }
        Ok(url)
    }

    pub async fn get_text(&self, url: &Url) -> Result<String> {
        debug!(%url, "fetching page");
        Ok(self
            .client
            .get(url.clone())
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::HttpClient;

    #[test]
    fn accepts_http_urls() {
        assert!(HttpClient::parse_url("https://example.com").is_ok());
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(HttpClient::parse_url("file:///tmp/page.html").is_err());
        assert!(HttpClient::parse_url("not a url").is_err());
    }
}
