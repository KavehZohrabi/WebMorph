//! Optional browser-backed document loader. The feature-gated implementation
//! launches a local, sandboxed Chromium process and never connects to an
//! existing DevTools service.
use std::{future::Future, pin::Pin};

use tokio_util::sync::CancellationToken;

use crate::{dom::Document, error::Result};

pub type BrowserFuture<'a> = Pin<Box<dyn Future<Output = Result<(Document, String)>> + Send + 'a>>;

/// Backend boundary used by the TUI. The lightweight HTTP engine remains the
/// default and is selected whenever no browser backend is supplied.
pub trait BrowserBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn render<'a>(&'a self, url: &'a str, cancellation: CancellationToken) -> BrowserFuture<'a>;
}

#[cfg(feature = "browser-engine")]
mod chromium {
    use std::{net::IpAddr, path::PathBuf, time::Duration};

    use chromiumoxide::{
        Browser, BrowserConfig,
        cdp::browser_protocol::browser::{SetDownloadBehaviorBehavior, SetDownloadBehaviorParams},
    };
    use futures_util::StreamExt;
    use serde::Deserialize;
    use tokio::time::timeout;
    use tokio_util::sync::CancellationToken;

    use crate::{
        dom::Document,
        error::{Result, WebMorphError},
        html,
        network::{HttpClient, resolve_public_destination},
    };

    use super::BrowserBackend;

    const MAX_RENDERED_HTML: usize = 8 * 1024 * 1024;

    #[derive(Debug, Clone)]
    pub struct ChromiumConfig {
        pub navigation_timeout: Duration,
        pub max_rendered_html_bytes: usize,
        pub javascript_heap_limit_mb: usize,
        pub renderer_process_limit: usize,
    }

    impl Default for ChromiumConfig {
        fn default() -> Self {
            Self {
                navigation_timeout: Duration::from_secs(30),
                max_rendered_html_bytes: MAX_RENDERED_HTML,
                javascript_heap_limit_mb: 256,
                renderer_process_limit: 2,
            }
        }
    }

    #[derive(Debug, Clone)]
    pub struct ChromiumEngine {
        executable: PathBuf,
        config: ChromiumConfig,
    }

    #[derive(Deserialize)]
    struct DomSnapshot {
        url: String,
        html: String,
    }

    impl ChromiumEngine {
        /// Detect an already installed Chrome, Chromium, or Edge executable.
        /// This does not download, install, or launch the browser.
        pub fn detect() -> Result<Self> {
            let executable = chromiumoxide::detection::default_executable(
                chromiumoxide::detection::DetectionOptions::default(),
            )
            .map_err(|_| {
                WebMorphError::BrowserUnavailable(
                    "no compatible Chromium executable was found; install one or set CHROME".into(),
                )
            })?;
            Ok(Self {
                executable,
                config: ChromiumConfig::default(),
            })
        }

        pub fn with_config(mut self, config: ChromiumConfig) -> Self {
            self.config = config;
            self
        }

        pub fn executable(&self) -> &std::path::Path {
            &self.executable
        }

        /// Executes a JavaScript expression inside an isolated Chromium page.
        /// The result is returned as JSON text. This is an embedding API; the
        /// TUI never evaluates user-provided scripts implicitly.
        pub async fn execute_javascript(
            &self,
            url: &str,
            expression: &str,
            cancellation: CancellationToken,
        ) -> Result<String> {
            let url = HttpClient::parse_url(url)?;
            let result = tokio::select! {
                _ = cancellation.cancelled() => return Err(WebMorphError::Cancelled),
                result = timeout(self.config.navigation_timeout, self.execute_inner(&url, Some(expression))) => {
                    result.map_err(|_| WebMorphError::Timeout)?
                }
            }?;
            Ok(result.1)
        }

        async fn render_inner(&self, url: &reqwest::Url) -> Result<(Document, String)> {
            let ips = resolve_public_destination(url).await?;
            // Map only the validated initial host to a validated public address.
            // Other hosts (including loopback/private destinations) cannot be
            // resolved by this Chromium process. This intentionally blocks
            // third-party CDN content for the optional backend.
            let address = ips
                .first()
                .ok_or_else(|| WebMorphError::Dns("hostname resolved to no addresses".into()))?;
            let host = url
                .host_str()
                .ok_or_else(|| WebMorphError::InvalidUrl("missing host".into()))?;
            let rules = host_resolver_rules(host, address);
            let snapshot_json = self.execute_inner_with_rules(url, None, &rules).await?;
            let snapshot: DomSnapshot = serde_json::from_str(&snapshot_json).map_err(|error| {
                WebMorphError::Browser(format!("invalid DOM snapshot: {error}"))
            })?;
            if snapshot.html.len() > self.config.max_rendered_html_bytes {
                return Err(WebMorphError::ResponseTooLarge {
                    limit: self.config.max_rendered_html_bytes,
                });
            }
            let final_url = HttpClient::parse_url(&snapshot.url)?;
            if final_url.host_str() != Some(host) {
                return Err(WebMorphError::Browser(
                    "Chromium navigation left the permitted host".into(),
                ));
            }
            let document = html::parse(final_url.as_str(), &snapshot.html);
            Ok((document, final_url.to_string()))
        }

        async fn execute_inner(
            &self,
            url: &reqwest::Url,
            expression: Option<&str>,
        ) -> Result<(String, String)> {
            let ips = resolve_public_destination(url).await?;
            let address = ips
                .first()
                .ok_or_else(|| WebMorphError::Dns("hostname resolved to no addresses".into()))?;
            let host = url
                .host_str()
                .ok_or_else(|| WebMorphError::InvalidUrl("missing host".into()))?;
            let rules = host_resolver_rules(host, address);
            self.execute_inner_with_rules(url, expression, &rules)
                .await
                .map(|result| (url.to_string(), result))
        }

        async fn execute_inner_with_rules(
            &self,
            url: &reqwest::Url,
            expression: Option<&str>,
            resolver_rules: &str,
        ) -> Result<String> {
            let profile = tempfile::Builder::new()
                .prefix("webmorph-chromium-")
                .tempdir()
                .map_err(WebMorphError::Terminal)?;
            let config = BrowserConfig::builder()
                .chrome_executable(&self.executable)
                .new_headless_mode()
                .incognito()
                .respect_https_errors()
                .disable_cache()
                .user_data_dir(profile.path())
                .launch_timeout(Duration::from_secs(15))
                .request_timeout(self.config.navigation_timeout)
                .disable_default_args()
                .args([
                    "disable-background-networking".to_owned(),
                    "disable-default-apps".to_owned(),
                    "disable-dev-shm-usage".to_owned(),
                    "disable-extensions".to_owned(),
                    "disable-sync".to_owned(),
                    "no-first-run".to_owned(),
                    "remote-debugging-address=127.0.0.1".to_owned(),
                    format!(
                        "renderer-process-limit={}",
                        self.config.renderer_process_limit.max(1)
                    ),
                    format!(
                        "js-flags=--max-old-space-size={}",
                        self.config.javascript_heap_limit_mb.max(64)
                    ),
                    format!("host-resolver-rules={resolver_rules}"),
                ])
                .build()
                .map_err(|error| WebMorphError::Browser(error.to_string()))?;

            let (mut browser, mut handler) = Browser::launch(config).await.map_err(|error| {
                WebMorphError::Browser(format!("could not start Chromium: {error}"))
            })?;
            let handler_task = tokio::spawn(async move {
                while let Some(result) = handler.next().await {
                    if result.is_err() {
                        break;
                    }
                }
            });
            let result = async {
                browser.execute(SetDownloadBehaviorParams {
                    behavior: SetDownloadBehaviorBehavior::Deny,
                    browser_context_id: None,
                    download_path: None,
                    events_enabled: Some(false),
                }).await.map_err(|error| WebMorphError::Browser(error.to_string()))?;
                let page = browser.new_page("about:blank").await
                    .map_err(|error| WebMorphError::Browser(error.to_string()))?;
                page.goto(url.as_str()).await
                    .map_err(|error| WebMorphError::Browser(error.to_string()))?;
                let current_url = page.url().await
                    .map_err(|error| WebMorphError::Browser(error.to_string()))?
                    .unwrap_or_else(|| url.to_string());
                // goto waits for the page load event. This additional evaluation
                // waits for readyState and demonstrates that page JS runs in
                // Chromium before the current DOM is extracted.
                let ready: String = page.evaluate_expression("document.readyState").await
                    .map_err(|error| WebMorphError::Browser(error.to_string()))?
                    .into_value()
                    .map_err(|error| WebMorphError::Browser(error.to_string()))?;
                if ready != "complete" {
                    return Err(WebMorphError::Browser("page did not reach ready state".into()));
                }
                let is_snapshot = expression.is_none();
                // A bounded DOM snapshot keeps the CDP message and Rust-side
                // allocation within a predictable limit.
                let snapshot_limit = self.config.max_rendered_html_bytes.saturating_add(1);
                let expression = expression.map(str::to_owned).unwrap_or_else(|| format!(
                    "({{url: location.href, html: (document.documentElement?.outerHTML ?? '').slice(0, {snapshot_limit})}})"
                ));
                let value = page.evaluate_expression(expression.as_str()).await
                    .map_err(|error| WebMorphError::Browser(error.to_string()))?
                    .value()
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                if let Some(object) = value.as_object() {
                    if is_snapshot {
                        let html_size = object.get("html").and_then(serde_json::Value::as_str).map(str::len).unwrap_or(0);
                        if html_size > self.config.max_rendered_html_bytes {
                            return Err(WebMorphError::ResponseTooLarge { limit: self.config.max_rendered_html_bytes });
                        }
                    }
                }
                serde_json::to_string(&if is_snapshot {
                    value
                } else {
                    serde_json::json!({"url": current_url, "html": "", "result": value})
                }).map_err(|error| WebMorphError::Browser(error.to_string()))
            }.await;
            // Always request orderly shutdown and reap the child. If this future
            // is cancelled or times out, chromiumoxide's Drop kills its child.
            let _ = browser.close().await;
            let _ = browser.wait().await;
            handler_task.abort();
            result
        }
    }

    fn host_resolver_rules(host: &str, address: &IpAddr) -> String {
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        let address = match address {
            IpAddr::V4(address) => address.to_string(),
            IpAddr::V6(address) => format!("[{address}]"),
        };
        format!("MAP {host} {address}, MAP * ~NOTFOUND")
    }

    impl BrowserBackend for ChromiumEngine {
        fn name(&self) -> &'static str {
            "chromium"
        }

        fn render<'a>(
            &'a self,
            input: &'a str,
            cancellation: CancellationToken,
        ) -> super::BrowserFuture<'a> {
            Box::pin(async move {
                let url = HttpClient::parse_url(input)?;
                tokio::select! {
                    _ = cancellation.cancelled() => Err(WebMorphError::Cancelled),
                    result = timeout(self.config.navigation_timeout, self.render_inner(&url)) => {
                        result.map_err(|_| WebMorphError::Timeout)?
                    }
                }
            })
        }
    }

    pub use ChromiumConfig as Config;
    pub use ChromiumEngine as Engine;
}

#[cfg(feature = "browser-engine")]
pub use chromium::{Config as ChromiumConfig, Engine as ChromiumEngine};
