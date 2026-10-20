use std::sync::Arc;

use tracing::info;

use crate::{
    browser::BrowserBackend,
    cli::{Cli, EngineChoice},
    error::Result,
    network::{HttpClient, HttpConfig},
    storage::Storage,
    tui::TerminalSession,
};

#[cfg(not(feature = "browser-engine"))]
use crate::error::WebMorphError;

pub async fn run(cli: Cli) -> Result<()> {
    let storage = Storage::open_default(cli.private)?;
    let settings = storage.settings()?;
    let input = cli.url.as_deref().unwrap_or(&settings.default_homepage);
    info!(%input, "opening website");
    let browser: Option<Arc<dyn BrowserBackend>> = match cli.engine {
        EngineChoice::Lightweight => None,
        EngineChoice::Chromium => {
            #[cfg(feature = "browser-engine")]
            {
                Some(Arc::new(
                    crate::browser::ChromiumEngine::detect()?.with_config(
                        crate::browser::ChromiumConfig {
                            navigation_timeout: std::time::Duration::from_secs(
                                settings.request_timeout_seconds,
                            ),
                            ..crate::browser::ChromiumConfig::default()
                        },
                    ),
                ))
            }
            #[cfg(not(feature = "browser-engine"))]
            {
                return Err(WebMorphError::BrowserUnavailable(
                    "this build does not include Chromium support; rebuild with --features browser-engine".into(),
                ));
            }
        }
    };
    let fallback_browser: Option<Arc<dyn BrowserBackend>> = if cli.fallback_chromium {
        #[cfg(feature = "browser-engine")]
        {
            Some(Arc::new(
                crate::browser::ChromiumEngine::detect()?.with_config(
                    crate::browser::ChromiumConfig {
                        navigation_timeout: std::time::Duration::from_secs(
                            settings.request_timeout_seconds,
                        ),
                        ..crate::browser::ChromiumConfig::default()
                    },
                ),
            ))
        }
        #[cfg(not(feature = "browser-engine"))]
        {
            return Err(WebMorphError::BrowserUnavailable(
                "--fallback-chromium requires a build with --features browser-engine".into(),
            ));
        }
    } else {
        None
    };
    let mut terminal = TerminalSession::new()?;
    let http_config = HttpConfig {
        request_timeout: std::time::Duration::from_secs(settings.request_timeout_seconds),
        user_agent: settings.user_agent,
        ..HttpConfig::default()
    };
    terminal
        .run_url(
            input.to_owned(),
            HttpClient::new(http_config)?,
            storage,
            browser,
            fallback_browser,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn no_url_is_a_valid_startup_state() {
        let cli = Cli::try_parse_from(["webmorph"]).expect("no URL is allowed");
        assert!(cli.url.is_none());
    }
}
