use std::{
    io,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use futures_util::StreamExt;
use reqwest::{
    Client, Response, Url,
    dns::{Addrs, Name, Resolve, Resolving},
    header::{ACCEPT, CONTENT_TYPE},
    redirect::Policy,
};
use tokio::{net::lookup_host, sync::Semaphore, time::timeout};
use tokio_util::sync::CancellationToken;
use tracing::debug;
use zeroize::Zeroize;

use crate::error::{Result, WebMorphError};

#[derive(Clone, Debug)]
pub struct HttpConfig {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub max_response_bytes: usize,
    pub max_concurrent_requests: usize,
    pub max_redirects: usize,
    pub user_agent: String,
    /// Enables loopback/private destinations for trusted embeddings and local tests.
    pub allow_private_network: bool,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            max_response_bytes: 8 * 1024 * 1024,
            max_concurrent_requests: 4,
            max_redirects: 10,
            user_agent: concat!("WebMorph/", env!("CARGO_PKG_VERSION")).into(),
            allow_private_network: false,
        }
    }
}

#[derive(Clone)]
pub struct HttpClient {
    client: Client,
    config: HttpConfig,
    permits: std::sync::Arc<Semaphore>,
}

struct SensitiveFields(Vec<(String, String)>);

impl Drop for SensitiveFields {
    fn drop(&mut self) {
        for (name, value) in &mut self.0 {
            name.zeroize();
            value.zeroize();
        }
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new(HttpConfig::default()).expect("valid default HTTP configuration")
    }
}

impl HttpClient {
    pub fn append_form_query(url: &Url, fields: &[(String, String)]) -> Url {
        let mut url = url.clone();
        {
            let mut query = url.query_pairs_mut();
            for (name, value) in fields {
                query.append_pair(name, value);
            }
        }
        url
    }

    pub fn new(config: HttpConfig) -> Result<Self> {
        let redirects = config.max_redirects;
        let allow_private_network = config.allow_private_network;
        let client = Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .user_agent(&config.user_agent)
            .dns_resolver(Arc::new(ValidatingResolver {
                allow_private_network,
                timeout: config.connect_timeout,
            }))
            .redirect(Policy::custom(move |attempt| {
                let target = attempt.url();
                let blocked = attempt.previous().len() >= redirects
                    || !matches!(target.scheme(), "http" | "https")
                    || !allow_private_network && is_blocked_redirect(target)
                    || attempt
                        .previous()
                        .iter()
                        .any(|previous| previous.scheme() == "https" && target.scheme() == "http")
                    || attempt
                        .previous()
                        .first()
                        .is_some_and(|first| first.host_str() != target.host_str());
                if blocked {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(classify_reqwest)?;
        Ok(Self {
            permits: std::sync::Arc::new(Semaphore::new(config.max_concurrent_requests.max(1))),
            config,
            client,
        })
    }

    pub fn parse_url(input: &str) -> Result<Url> {
        let url = Url::parse(input).map_err(|e| WebMorphError::InvalidUrl(e.to_string()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(WebMorphError::InvalidUrl(
                "only http and https URLs are supported".into(),
            ));
        }
        if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
            return Err(WebMorphError::InvalidUrl(
                "URL must have a host and cannot contain credentials".into(),
            ));
        }
        Ok(url)
    }

    pub fn resolve_url(base: &Url, reference: &str) -> Result<Url> {
        let resolved = base
            .join(reference)
            .map_err(|e| WebMorphError::InvalidUrl(e.to_string()))?;
        Self::parse_url(resolved.as_str())
    }

    pub async fn get_text(&self, url: &Url) -> Result<String> {
        self.get_text_cancellable(url, CancellationToken::new())
            .await
    }

    pub async fn get_text_cancellable(
        &self,
        url: &Url,
        cancellation: CancellationToken,
    ) -> Result<String> {
        self.get_text_with_final_url_cancellable(url, cancellation)
            .await
            .map(|(_, body)| body)
    }

    pub async fn get_text_with_final_url_cancellable(
        &self,
        url: &Url,
        cancellation: CancellationToken,
    ) -> Result<(Url, String)> {
        let url = Self::parse_url(url.as_str())?;
        let _permit = tokio::select! {
            _ = cancellation.cancelled() => return Err(WebMorphError::Cancelled),
            permit = self.permits.clone().acquire_owned() => permit.map_err(|_| WebMorphError::Connection("request queue is closed".into()))?,
        };
        tokio::select! {
            _ = cancellation.cancelled() => return Err(WebMorphError::Cancelled),
            result = self.check_destination(&url) => result?,
        }
        debug!(host = url.host_str().unwrap_or("unknown"), "fetching page");
        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(WebMorphError::Cancelled),
            result = self.client.get(url).header(ACCEPT, "text/html,application/xhtml+xml;q=0.9").send() => result.map_err(classify_reqwest)?,
        };
        self.validate_response(&response)?;
        let final_url = response.url().clone();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        loop {
            let next = tokio::select! {
                _ = cancellation.cancelled() => return Err(WebMorphError::Cancelled),
                chunk = stream.next() => chunk,
            };
            match next {
                None => break,
                Some(Err(error)) => return Err(classify_reqwest(error)),
                Some(Ok(chunk)) => {
                    if body.len().saturating_add(chunk.len()) > self.config.max_response_bytes {
                        return Err(WebMorphError::ResponseTooLarge {
                            limit: self.config.max_response_bytes,
                        });
                    }
                    body.extend_from_slice(&chunk);
                }
            }
        }
        let body = String::from_utf8(body).map_err(|_| {
            WebMorphError::UnsupportedContent("HTML response is not valid UTF-8".into())
        })?;
        Ok((final_url, body))
    }

    /// Submit URL-encoded form pairs without logging their values. The caller
    /// must only invoke this after an explicit user submission action.
    pub async fn submit_form_cancellable(
        &self,
        url: &Url,
        fields: Vec<(String, String)>,
        cancellation: CancellationToken,
    ) -> Result<(Url, String)> {
        let fields = SensitiveFields(fields);
        let url = Self::parse_url(url.as_str())?;
        let _permit = tokio::select! {
            _ = cancellation.cancelled() => return Err(WebMorphError::Cancelled),
            permit = self.permits.clone().acquire_owned() => permit.map_err(|_| WebMorphError::Connection("request queue is closed".into()))?,
        };
        tokio::select! {
            _ = cancellation.cancelled() => return Err(WebMorphError::Cancelled),
            result = self.check_destination(&url) => result?,
        }
        debug!(
            host = url.host_str().unwrap_or("unknown"),
            "submitting URL-encoded form"
        );
        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(WebMorphError::Cancelled),
            result = self.client.post(url).header(ACCEPT, "text/html,application/xhtml+xml;q=0.9").form(&fields.0).send() => result.map_err(classify_reqwest)?,
        };
        self.validate_response(&response)?;
        let final_url = response.url().clone();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        loop {
            let next = tokio::select! {
                _ = cancellation.cancelled() => return Err(WebMorphError::Cancelled),
                chunk = stream.next() => chunk,
            };
            match next {
                None => break,
                Some(Err(error)) => return Err(classify_reqwest(error)),
                Some(Ok(chunk)) => {
                    if body.len().saturating_add(chunk.len()) > self.config.max_response_bytes {
                        return Err(WebMorphError::ResponseTooLarge {
                            limit: self.config.max_response_bytes,
                        });
                    }
                    body.extend_from_slice(&chunk);
                }
            }
        }
        let body = String::from_utf8(body).map_err(|_| {
            WebMorphError::UnsupportedContent("HTML response is not valid UTF-8".into())
        })?;
        Ok((final_url, body))
    }

    async fn check_destination(&self, url: &Url) -> Result<()> {
        if self.config.allow_private_network {
            return Ok(());
        }
        let host = url
            .host_str()
            .ok_or_else(|| WebMorphError::InvalidUrl("missing host".into()))?;
        if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
            return Err(WebMorphError::InvalidUrl(
                "local network destinations are blocked".into(),
            ));
        }
        let addresses: Vec<IpAddr> = match host.parse::<IpAddr>() {
            Ok(ip) => vec![ip],
            Err(_) => {
                let port = url.port_or_known_default().unwrap_or(80);
                timeout(self.config.connect_timeout, lookup_host((host, port)))
                    .await
                    .map_err(|_| WebMorphError::Timeout)?
                    .map_err(|e| WebMorphError::Dns(e.to_string()))?
                    .map(|addr: SocketAddr| addr.ip())
                    .collect()
            }
        };
        if addresses.is_empty() {
            return Err(WebMorphError::Dns(
                "hostname resolved to no addresses".into(),
            ));
        }
        if addresses.iter().any(is_non_public) {
            return Err(WebMorphError::InvalidUrl(
                "local or private network destinations are blocked".into(),
            ));
        }
        Ok(())
    }

    fn validate_response(&self, response: &Response) -> Result<()> {
        if !response.status().is_success() {
            return Err(WebMorphError::HttpStatus(response.status().as_u16()));
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if mime != "text/html" && mime != "application/xhtml+xml" {
            return Err(WebMorphError::UnsupportedContent(
                if content_type.is_empty() {
                    "missing Content-Type".into()
                } else {
                    content_type.into()
                },
            ));
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.config.max_response_bytes as u64)
        {
            return Err(WebMorphError::ResponseTooLarge {
                limit: self.config.max_response_bytes,
            });
        }
        Ok(())
    }
}

/// Validate the addresses used by reqwest itself, closing the gap between an
/// earlier policy lookup and the HTTP client's independent DNS lookup.
struct ValidatingResolver {
    allow_private_network: bool,
    timeout: Duration,
}

impl Resolve for ValidatingResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        let allow_private_network = self.allow_private_network;
        let resolution_timeout = self.timeout;
        Box::pin(async move {
            let addresses = match timeout(resolution_timeout, lookup_host((host.as_str(), 0))).await
            {
                Ok(Ok(addresses)) => addresses.collect::<Vec<_>>(),
                Ok(Err(error)) => return Err(Box::new(error) as _),
                Err(_) => {
                    return Err(Box::new(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "DNS resolution timed out",
                    )) as _);
                }
            };
            if addresses.is_empty() {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::NotFound,
                    "hostname resolved to no addresses",
                )) as _);
            }
            if !allow_private_network
                && addresses.iter().any(|address| is_non_public(&address.ip()))
            {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "local or private network destinations are blocked",
                )) as _);
            }
            let resolved: Addrs = Box::new(addresses.into_iter());
            Ok(resolved)
        })
    }
}

fn is_non_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || ip.octets()[0] == 0
                || ip.octets()[0] >= 224
                || is_special_ipv4(ip.octets())
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            !(s[0] & 0xfe00 == 0x2000)
                || ip.is_loopback()
                || ip.is_unspecified()
                || (s[0] == 0x2001 && (s[1] <= 0x01ff || s[1] == 0x0db8))
                || s[0] == 0x2002
        }
    }
}

fn is_special_ipv4(octets: [u8; 4]) -> bool {
    matches!(octets, [100, 64..=127, _, _])
        || matches!(octets, [192, 0, 0, _] | [192, 0, 2, _] | [192, 88, 99, _])
        || matches!(octets, [198, 18..=19, _, _] | [198, 51, 100, _])
        || matches!(octets, [203, 0, 113, _])
}

/// Resolve and validate a public destination for browser-backed navigation.
/// Returning every resolved address lets callers pin the host after validation
/// rather than resolving it again in a child browser process.
#[cfg(feature = "browser-engine")]
pub(crate) async fn resolve_public_destination(url: &Url) -> Result<Vec<IpAddr>> {
    let host = url
        .host_str()
        .ok_or_else(|| WebMorphError::InvalidUrl("missing host".into()))?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(WebMorphError::InvalidUrl(
            "local network destinations are blocked".into(),
        ));
    }
    let addresses: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => {
            let port = url.port_or_known_default().unwrap_or(80);
            timeout(Duration::from_secs(10), lookup_host((host, port)))
                .await
                .map_err(|_| WebMorphError::Timeout)?
                .map_err(|error| WebMorphError::Dns(error.to_string()))?
                .map(|address: SocketAddr| address.ip())
                .collect()
        }
    };
    if addresses.is_empty() {
        return Err(WebMorphError::Dns(
            "hostname resolved to no addresses".into(),
        ));
    }
    if addresses.iter().any(is_non_public) {
        return Err(WebMorphError::InvalidUrl(
            "local or private network destinations are blocked".into(),
        ));
    }
    Ok(addresses)
}

fn is_blocked_redirect(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return true;
    };
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|ip| is_non_public(&ip))
        .unwrap_or(false)
}

fn classify_reqwest(error: reqwest::Error) -> WebMorphError {
    if error.is_timeout() {
        WebMorphError::Timeout
    } else if error.is_connect() {
        let msg = error.to_string();
        let lower = msg.to_ascii_lowercase();
        if lower.contains("timed out") || lower.contains("timeout") {
            WebMorphError::Timeout
        } else if ["dns", "resolve", "lookup", "name or service"]
            .iter()
            .any(|part| lower.contains(part))
        {
            WebMorphError::Dns(msg)
        } else {
            WebMorphError::Connection(msg)
        }
    } else {
        WebMorphError::Connection(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{HttpClient, HttpConfig, is_non_public};
    use std::time::Duration;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::oneshot,
    };
    use tokio_util::sync::CancellationToken;

    async fn serve(response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            let _ = socket.read(&mut request).await;
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        format!("http://{address}/")
    }

    async fn serve_redirect() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for (status, headers, body) in [
                ("302 Found", "Location: /final\r\n", ""),
                ("200 OK", "Content-Type: text/html\r\n", "<p>redirected</p>"),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 2048];
                let _ = socket.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        format!("http://{address}/start")
    }

    fn local_client() -> HttpClient {
        HttpClient::new(HttpConfig {
            allow_private_network: true,
            ..HttpConfig::default()
        })
        .unwrap()
    }

    #[test]
    fn accepts_http_and_rejects_non_http_schemes() {
        assert!(HttpClient::parse_url("https://example.com").is_ok());
        assert!(HttpClient::parse_url("file:///tmp/page.html").is_err());
        assert!(HttpClient::parse_url("not a url").is_err());
        assert!(HttpClient::parse_url("https://user:pass@example.com").is_err());
    }

    #[test]
    fn resolves_relative_references() {
        let base = HttpClient::parse_url("https://example.com/a/page").unwrap();
        assert_eq!(
            HttpClient::resolve_url(&base, "../next").unwrap().as_str(),
            "https://example.com/next"
        );
    }

    #[test]
    fn blocks_reserved_non_public_address_ranges() {
        for address in ["100.64.0.1", "192.0.2.1", "198.18.0.1", "203.0.113.1"] {
            assert!(is_non_public(&address.parse().unwrap()), "{address}");
        }
        for address in ["2001:db8::1", "2002::1"] {
            assert!(is_non_public(&address.parse().unwrap()), "{address}");
        }
    }

    #[tokio::test]
    async fn fetches_html_from_local_server() {
        let url = serve("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 30\r\nConnection: close\r\n\r\n<html><title>ok</title></html>").await;
        let text = local_client()
            .get_text(&HttpClient::parse_url(&url).unwrap())
            .await
            .unwrap();
        assert!(text.contains("<title>ok</title>"));
    }

    #[tokio::test]
    async fn follows_same_host_redirects() {
        let url = serve_redirect().await;
        let (final_url, text) = local_client()
            .get_text_with_final_url_cancellable(
                &HttpClient::parse_url(&url).unwrap(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(text, "<p>redirected</p>");
        assert!(final_url.path().ends_with("/final"));
    }

    #[tokio::test]
    async fn blocks_local_destinations_by_default() {
        let url = HttpClient::parse_url("http://127.0.0.1/").unwrap();
        assert!(matches!(
            HttpClient::default().get_text(&url).await,
            Err(crate::error::WebMorphError::InvalidUrl(_))
        ));
    }

    #[tokio::test]
    async fn rejects_http_errors_and_non_html() {
        let url =
            serve("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
        assert!(matches!(
            local_client()
                .get_text(&HttpClient::parse_url(&url).unwrap())
                .await,
            Err(crate::error::WebMorphError::HttpStatus(404))
        ));
        let url = serve("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await;
        assert!(matches!(
            local_client()
                .get_text(&HttpClient::parse_url(&url).unwrap())
                .await,
            Err(crate::error::WebMorphError::UnsupportedContent(_))
        ));
    }

    #[tokio::test]
    async fn enforces_size_limit_and_cancellation() {
        let url = serve("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 20\r\nConnection: close\r\n\r\n01234567890123456789").await;
        let client = HttpClient::new(HttpConfig {
            max_response_bytes: 8,
            allow_private_network: true,
            ..HttpConfig::default()
        })
        .unwrap();
        let parsed = HttpClient::parse_url(&url).unwrap();
        assert!(matches!(
            client.get_text(&parsed).await,
            Err(crate::error::WebMorphError::ResponseTooLarge { .. })
        ));
        let token = CancellationToken::new();
        token.cancel();
        assert!(matches!(
            local_client().get_text_cancellable(&parsed, token).await,
            Err(crate::error::WebMorphError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn cancellation_interrupts_an_active_slow_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (started, accepted) = oneshot::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request).await;
            let _ = started.send(());
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let token = CancellationToken::new();
        let client = local_client();
        let url = HttpClient::parse_url(&format!("http://{address}/slow")).unwrap();
        let request = tokio::spawn({
            let token = token.clone();
            async move { client.get_text_cancellable(&url, token).await }
        });
        accepted.await.unwrap();
        token.cancel();
        assert!(matches!(
            request.await.unwrap(),
            Err(crate::error::WebMorphError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn limits_parallel_network_requests() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        tokio::spawn({
            let active = active.clone();
            let maximum = maximum.clone();
            async move {
                for _ in 0..2 {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let active = active.clone();
                    let maximum = maximum.clone();
                    tokio::spawn(async move {
                        let mut request = [0; 1024];
                        let _ = socket.read(&mut request).await;
                        let now = active.fetch_add(1, Ordering::Relaxed) + 1;
                        maximum.fetch_max(now, Ordering::Relaxed);
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        let body = "<p>ok</p>";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                        active.fetch_sub(1, Ordering::Relaxed);
                    });
                }
            }
        });
        let client = HttpClient::new(HttpConfig {
            allow_private_network: true,
            max_concurrent_requests: 1,
            ..HttpConfig::default()
        })
        .unwrap();
        let url = HttpClient::parse_url(&format!("http://{address}/bounded")).unwrap();
        let (first, second) = tokio::join!(client.get_text(&url), client.get_text(&url));
        assert!(first.is_ok() && second.is_ok());
        assert_eq!(maximum.load(Ordering::Relaxed), 1);
    }
}
