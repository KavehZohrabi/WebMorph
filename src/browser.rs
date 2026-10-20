//! Optional browser-backed document loader. The feature-gated implementation
//! launches a local, sandboxed Chromium process and never connects to an
//! existing DevTools service.
use std::{future::Future, pin::Pin};

use tokio_util::sync::CancellationToken;

use crate::{dom::Document, error::Result};

pub type BrowserFuture<'a> = Pin<Box<dyn Future<Output = Result<(Document, String)>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserInteraction {
    Click {
        interactive_id: String,
    },
    SetValue {
        interactive_id: String,
        value: String,
    },
    SelectOption {
        interactive_id: String,
        value: String,
    },
    SubmitForm {
        form_index: usize,
    },
    Scroll {
        delta: i32,
    },
}

/// Backend boundary used by the TUI. The lightweight HTTP engine remains the
/// default and is selected whenever no browser backend is supplied.
pub trait BrowserBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn render<'a>(&'a self, url: &'a str, cancellation: CancellationToken) -> BrowserFuture<'a>;

    fn navigate<'a>(
        &'a self,
        _session_id: u64,
        url: &'a str,
        cancellation: CancellationToken,
    ) -> BrowserFuture<'a> {
        self.render(url, cancellation)
    }

    fn interact<'a>(
        &'a self,
        _session_id: u64,
        _interaction: BrowserInteraction,
        _cancellation: CancellationToken,
    ) -> BrowserFuture<'a> {
        Box::pin(async {
            Err(crate::error::WebMorphError::Browser(
                "this browser backend does not support interactions".into(),
            ))
        })
    }

    fn close_session<'a>(
        &'a self,
        _session_id: u64,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }

    fn close_all<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

#[cfg(feature = "browser-engine")]
mod chromium {
    use std::{collections::HashMap, net::IpAddr, path::PathBuf, pin::Pin, time::Duration};

    use chromiumoxide::{
        Browser, BrowserConfig,
        cdp::browser_protocol::browser::{SetDownloadBehaviorBehavior, SetDownloadBehaviorParams},
    };
    use futures_util::StreamExt;
    use serde::Deserialize;
    use tempfile::TempDir;
    use tokio::{sync::Mutex, task::JoinHandle, time::timeout};
    use tokio_util::sync::CancellationToken;

    use crate::browser_proxy::EgressProxy;
    use crate::{
        dom::Document,
        error::{Result, WebMorphError},
        html,
        network::{HttpClient, resolve_http_destination, resolve_public_destination},
    };

    use super::{BrowserBackend, BrowserInteraction};

    const MAX_RENDERED_HTML: usize = 8 * 1024 * 1024;
    const MAX_BROWSER_SESSIONS: usize = 4;
    const MAX_RENDERER_PROCESSES: usize = 4;
    const MAX_JAVASCRIPT_HEAP_MB: usize = 512;

    #[derive(Debug, Clone)]
    pub struct ChromiumConfig {
        pub navigation_timeout: Duration,
        pub max_rendered_html_bytes: usize,
        pub javascript_heap_limit_mb: usize,
        pub renderer_process_limit: usize,
        pub max_sessions: usize,
        /// Enables explicitly trusted local pages, primarily for local testing.
        pub allow_private_network: bool,
    }

    impl Default for ChromiumConfig {
        fn default() -> Self {
            Self {
                navigation_timeout: Duration::from_secs(30),
                max_rendered_html_bytes: MAX_RENDERED_HTML,
                javascript_heap_limit_mb: 256,
                renderer_process_limit: 2,
                max_sessions: 4,
                allow_private_network: false,
            }
        }
    }

    #[derive(Clone)]
    pub struct ChromiumEngine {
        executable: PathBuf,
        config: ChromiumConfig,
        sessions: std::sync::Arc<Mutex<HashMap<u64, std::sync::Arc<Mutex<ChromiumSession>>>>>,
    }

    struct ChromiumSession {
        browser: Browser,
        page: chromiumoxide::Page,
        _profile: TempDir,
        handler: JoinHandle<()>,
        hostname: String,
        element_map: HashMap<String, String>,
        proxy: Option<EgressProxy>,
    }

    #[derive(Deserialize)]
    struct DomSnapshot {
        url: String,
        html: String,
    }

    fn browser_element_lookup(key: &str) -> Result<String> {
        let key = serde_json::to_string(key)
            .map_err(|error| WebMorphError::Browser(error.to_string()))?;
        Ok(format!(
            "[...document.querySelectorAll('a[href],button,input,textarea,select,[role=\"button\"]')].find(node => window.__webmorphElementKeys?.get(node) === {key})"
        ))
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
                sessions: std::sync::Arc::new(Mutex::new(HashMap::new())),
            })
        }

        pub fn with_config(mut self, config: ChromiumConfig) -> Self {
            self.config = config;
            self
        }

        pub fn executable(&self) -> &std::path::Path {
            &self.executable
        }

        async fn create_session(&self, url: &reqwest::Url) -> Result<ChromiumSession> {
            let addresses =
                resolve_http_destination(url, self.config.allow_private_network).await?;
            let hostname = url
                .host_str()
                .ok_or_else(|| WebMorphError::InvalidUrl("missing host".into()))?
                .to_owned();
            let proxy = if self.config.allow_private_network {
                None
            } else {
                Some(EgressProxy::bind(false).await?)
            };
            let resolver_rules = if proxy.is_some() {
                "MAP * ~NOTFOUND".to_owned()
            } else {
                host_resolver_rules(
                    &hostname,
                    addresses.first().ok_or_else(|| {
                        WebMorphError::Dns("hostname resolved to no addresses".into())
                    })?,
                )
            };
            let mut launch_args = vec![
                "disable-background-networking".to_owned(),
                "disable-default-apps".to_owned(),
                "disable-dev-shm-usage".to_owned(),
                "disable-extensions".to_owned(),
                "disable-sync".to_owned(),
                "no-first-run".to_owned(),
                "remote-debugging-address=127.0.0.1".to_owned(),
                format!(
                    "renderer-process-limit={}",
                    self.config
                        .renderer_process_limit
                        .clamp(1, MAX_RENDERER_PROCESSES)
                ),
                format!(
                    "js-flags=--max-old-space-size={}",
                    self.config
                        .javascript_heap_limit_mb
                        .clamp(64, MAX_JAVASCRIPT_HEAP_MB)
                ),
                format!("host-resolver-rules={resolver_rules}"),
            ];
            if let Some(proxy) = &proxy {
                launch_args.push(format!("proxy-server=http://127.0.0.1:{}", proxy.port()));
                launch_args.push("proxy-bypass-list=<-loopback>".to_owned());
            }
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
                .args(launch_args)
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
            let init = async {
                browser
                    .execute(SetDownloadBehaviorParams {
                        behavior: SetDownloadBehaviorBehavior::Deny,
                        browser_context_id: None,
                        download_path: None,
                        events_enabled: Some(false),
                    })
                    .await
                    .map_err(|error| WebMorphError::Browser(error.to_string()))?;
                let page = browser
                    .new_page("about:blank")
                    .await
                    .map_err(|error| WebMorphError::Browser(error.to_string()))?;
                Ok::<_, WebMorphError>(page)
            }
            .await;
            match init {
                Ok(page) => Ok(ChromiumSession {
                    browser,
                    page,
                    _profile: profile,
                    handler: handler_task,
                    hostname,
                    element_map: HashMap::new(),
                    proxy,
                }),
                Err(error) => {
                    let _ = browser.close().await;
                    let _ = browser.wait().await;
                    handler_task.abort();
                    Err(error)
                }
            }
        }

        async fn session_snapshot(
            &self,
            session: &mut ChromiumSession,
        ) -> Result<(Document, String)> {
            let ready: String = session
                .page
                .evaluate_expression("document.readyState")
                .await
                .map_err(|error| WebMorphError::Browser(error.to_string()))?
                .into_value()
                .map_err(|error| WebMorphError::Browser(error.to_string()))?;
            if !matches!(ready.as_str(), "interactive" | "complete") {
                return Err(WebMorphError::Browser(
                    "page did not reach a readable state".into(),
                ));
            }
            let expression = format!(
                r#"(() => {{
                  const nodes = [...document.querySelectorAll('a[href],button,input,textarea,select,[role="button"]')];
                  const keys = window.__webmorphElementKeys || (window.__webmorphElementKeys = new WeakMap());
                  let nextKey = window.__webmorphNextElementKey || 0;
                  nodes.forEach(node => {{ if (!keys.has(node)) keys.set(node, `wm-${{nextKey++}}`); }});
                  window.__webmorphNextElementKey = nextKey;
                  const clone = document.documentElement.cloneNode(true);
                  const clonedNodes = [...clone.querySelectorAll('a[href],button,input,textarea,select,[role="button"]')];
                  nodes.forEach((node, index) => {{
                    const cloneNode = clonedNodes[index];
                    if (!cloneNode) return;
                    cloneNode.setAttribute('data-webmorph-key', keys.get(node));
                    if (!cloneNode.hasAttribute('aria-label') && node.hasAttribute('aria-labelledby')) {{
                      const label = node.getAttribute('aria-labelledby').split(/\s+/).map(id => document.getElementById(id)?.textContent?.trim()).filter(Boolean).join(' ');
                      if (label) cloneNode.setAttribute('aria-label', label);
                    }}
                  }});
                  const sourceControls = [...document.querySelectorAll('input,textarea,select')];
                  const clonedControls = [...clone.querySelectorAll('input,textarea,select')];
                  sourceControls.forEach((source, index) => {{
                    const target = clonedControls[index];
                    if (!target) return;
                    if (source instanceof HTMLInputElement) {{
                      if (source.type === 'password') target.removeAttribute('value');
                      else {{ target.setAttribute('value', source.value); if (source.checked) target.setAttribute('checked', ''); else target.removeAttribute('checked'); }}
                    }} else if (source instanceof HTMLTextAreaElement) target.textContent = source.value;
                    else if (source instanceof HTMLSelectElement) [...target.options].forEach((option, i) => {{ if (source.options[i]?.selected) option.setAttribute('selected', ''); else option.removeAttribute('selected'); }});
                  }});
                  const html = clone.outerHTML;
                  if (html.length > {max_bytes}) throw new Error('rendered page exceeded WebMorph size limit');
                  return {{url: location.href, html}};
                }})()"#,
                max_bytes = self.config.max_rendered_html_bytes.min(MAX_RENDERED_HTML)
            );
            let value = session
                .page
                .evaluate_expression(expression)
                .await
                .map_err(|error| WebMorphError::Browser(error.to_string()))?
                .value()
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let snapshot: DomSnapshot = serde_json::from_value(value).map_err(|error| {
                WebMorphError::Browser(format!("invalid DOM snapshot: {error}"))
            })?;
            let size_limit = self.config.max_rendered_html_bytes.min(MAX_RENDERED_HTML);
            if snapshot.html.len() > size_limit {
                return Err(WebMorphError::ResponseTooLarge { limit: size_limit });
            }
            let final_url = HttpClient::parse_url(&snapshot.url)?;
            if final_url.host_str() != Some(session.hostname.as_str()) {
                return Err(WebMorphError::Browser(
                    "Chromium navigation left the permitted host".into(),
                ));
            }
            let document = html::parse(final_url.as_str(), &snapshot.html);
            session.element_map = document
                .interactive
                .iter()
                .filter_map(|element| {
                    element
                        .html_id
                        .as_ref()
                        .map(|key| (element.id.clone(), key.clone()))
                })
                .collect();
            Ok((document, final_url.to_string()))
        }

        async fn navigate_session(&self, session_id: u64, url: &str) -> Result<(Document, String)> {
            let url = HttpClient::parse_url(url)?;
            let hostname = url.host_str().unwrap_or_default().to_owned();
            let existing = self.sessions.lock().await.get(&session_id).cloned();
            let replaced = if let Some(existing) = existing {
                let is_different_host = existing.lock().await.hostname != hostname;
                if is_different_host {
                    let mut sessions = self.sessions.lock().await;
                    if sessions
                        .get(&session_id)
                        .is_some_and(|current| std::sync::Arc::ptr_eq(current, &existing))
                    {
                        sessions.remove(&session_id)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(session) = replaced {
                close_chromium_session(&mut *session.lock().await).await;
            }
            let session = {
                let mut sessions = self.sessions.lock().await;
                if let Some(session) = sessions.get(&session_id) {
                    session.clone()
                } else {
                    let session_limit = self.config.max_sessions.clamp(1, MAX_BROWSER_SESSIONS);
                    if sessions.len() >= session_limit {
                        return Err(WebMorphError::Browser(format!(
                            "Chromium session limit ({}) reached; close a tab first",
                            session_limit
                        )));
                    }
                    let session = std::sync::Arc::new(Mutex::new(self.create_session(&url).await?));
                    sessions.insert(session_id, session.clone());
                    session
                }
            };
            let mut session = session.lock().await;
            session
                .page
                .goto(url.as_str())
                .await
                .map_err(|error| WebMorphError::Browser(error.to_string()))?;
            self.wait_for_stable_dom(&session.page).await?;
            self.session_snapshot(&mut session).await
        }

        async fn interact_session(
            &self,
            session_id: u64,
            interaction: BrowserInteraction,
        ) -> Result<(Document, String)> {
            let session = self
                .sessions
                .lock()
                .await
                .get(&session_id)
                .cloned()
                .ok_or_else(|| {
                    WebMorphError::Browser("browser tab session is no longer available".into())
                })?;
            let mut session = session.lock().await;
            let expression = match interaction {
                BrowserInteraction::Click { interactive_id } => {
                    let key = session.element_map.get(&interactive_id).ok_or_else(|| {
                        WebMorphError::Browser(
                            "selected browser element is no longer available".into(),
                        )
                    })?;
                    let lookup = browser_element_lookup(key)?;
                    format!(
                        "(() => {{ const element = {lookup}; if (!element) return false; element.click(); return true; }})()"
                    )
                }
                BrowserInteraction::SetValue {
                    interactive_id,
                    value,
                } => {
                    let key = session.element_map.get(&interactive_id).ok_or_else(|| {
                        WebMorphError::Browser("selected input is no longer available".into())
                    })?;
                    let lookup = browser_element_lookup(key)?;
                    let value = serde_json::to_string(&value)
                        .map_err(|error| WebMorphError::Browser(error.to_string()))?;
                    format!(
                        "(() => {{ const element = {lookup}; if (!element) return false; const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), 'value')?.set; if (setter) setter.call(element, {value}); else element.value = {value}; element.dispatchEvent(new InputEvent('input', {{bubbles:true, inputType:'insertText', data:{value}}})); element.dispatchEvent(new Event('change', {{bubbles:true}})); return true; }})()"
                    )
                }
                BrowserInteraction::SelectOption {
                    interactive_id,
                    value,
                } => {
                    let key = session.element_map.get(&interactive_id).ok_or_else(|| {
                        WebMorphError::Browser("selected dropdown is no longer available".into())
                    })?;
                    let lookup = browser_element_lookup(key)?;
                    let value = serde_json::to_string(&value)
                        .map_err(|error| WebMorphError::Browser(error.to_string()))?;
                    format!(
                        "(() => {{ const element = {lookup}; if (!element) return false; const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), 'value')?.set; if (setter) setter.call(element, {value}); else element.value = {value}; element.dispatchEvent(new Event('input', {{bubbles:true}})); element.dispatchEvent(new Event('change', {{bubbles:true}})); return true; }})()"
                    )
                }
                BrowserInteraction::SubmitForm { form_index } => format!(
                    "(() => {{ const form = document.forms[{form_index}]; if (!form) return false; form.requestSubmit(); return true; }})()"
                ),
                BrowserInteraction::Scroll { delta } => format!(
                    "(() => {{ window.scrollBy({{top: {}, behavior: 'instant'}}); return true; }})()",
                    delta
                ),
            };
            let applied: bool = session
                .page
                .evaluate_expression(expression)
                .await
                .map_err(|error| WebMorphError::Browser(error.to_string()))?
                .into_value()
                .map_err(|error| WebMorphError::Browser(error.to_string()))?;
            if !applied {
                return Err(WebMorphError::Browser(
                    "browser action could not find its DOM target".into(),
                ));
            }
            self.wait_for_stable_dom(&session.page).await?;
            self.session_snapshot(&mut session).await
        }

        async fn wait_for_stable_dom(&self, page: &chromiumoxide::Page) -> Result<()> {
            let expression = r#"(async () => { await new Promise(resolve => { let timer; let finished = false; const done = () => { if (finished) return; finished = true; observer.disconnect(); clearTimeout(timer); resolve(); }; const observer = new MutationObserver(() => { clearTimeout(timer); timer = setTimeout(done, 250); }); observer.observe(document, {subtree:true, childList:true, characterData:true}); timer = setTimeout(done, 350); setTimeout(done, 2500); }); return document.readyState; })()"#;
            timeout(
                self.config.navigation_timeout.min(Duration::from_secs(4)),
                async {
                    page.evaluate_expression(expression)
                        .await
                        .map_err(|error| WebMorphError::Browser(error.to_string()))?;
                    Ok::<(), WebMorphError>(())
                },
            )
            .await
            .map_err(|_| WebMorphError::Timeout)??;
            Ok(())
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
                if let Some(object) = value.as_object()
                    && is_snapshot
                {
                    let html_size = object.get("html").and_then(serde_json::Value::as_str).map(str::len).unwrap_or(0);
                    if html_size > self.config.max_rendered_html_bytes {
                        return Err(WebMorphError::ResponseTooLarge { limit: self.config.max_rendered_html_bytes });
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

    async fn close_chromium_session(session: &mut ChromiumSession) {
        let _ = session.page.clone().close().await;
        let _ = session.browser.close().await;
        let _ = session.browser.wait().await;
        session.handler.abort();
        if let Some(proxy) = session.proxy.take() {
            proxy.close().await;
        }
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
                    result = timeout(self.config.navigation_timeout, self.navigate_session(0, url.as_str())) => {
                        result.map_err(|_| WebMorphError::Timeout)?
                    }
                }
            })
        }

        fn navigate<'a>(
            &'a self,
            session_id: u64,
            input: &'a str,
            cancellation: CancellationToken,
        ) -> super::BrowserFuture<'a> {
            Box::pin(async move {
                tokio::select! {
                    _ = cancellation.cancelled() => Err(WebMorphError::Cancelled),
                    result = timeout(self.config.navigation_timeout, self.navigate_session(session_id, input)) => {
                        result.map_err(|_| WebMorphError::Timeout)?
                    }
                }
            })
        }

        fn interact<'a>(
            &'a self,
            session_id: u64,
            interaction: BrowserInteraction,
            cancellation: CancellationToken,
        ) -> super::BrowserFuture<'a> {
            Box::pin(async move {
                tokio::select! {
                    _ = cancellation.cancelled() => Err(WebMorphError::Cancelled),
                    result = timeout(self.config.navigation_timeout, self.interact_session(session_id, interaction)) => {
                        result.map_err(|_| WebMorphError::Timeout)?
                    }
                }
            })
        }

        fn close_session<'a>(
            &'a self,
            session_id: u64,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                if let Some(session) = self.sessions.lock().await.remove(&session_id) {
                    close_chromium_session(&mut *session.lock().await).await;
                }
            })
        }

        fn close_all<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                let sessions = std::mem::take(&mut *self.sessions.lock().await);
                for (_, session) in sessions {
                    close_chromium_session(&mut *session.lock().await).await;
                }
            })
        }
    }

    pub use ChromiumConfig as Config;
    pub use ChromiumEngine as Engine;
}

#[cfg(feature = "browser-engine")]
pub use chromium::{Config as ChromiumConfig, Engine as ChromiumEngine};
