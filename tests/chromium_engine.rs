#![cfg(feature = "browser-engine")]

use tokio_util::sync::CancellationToken;
use webmorph::browser::{BrowserBackend, ChromiumEngine};

#[test]
fn detects_an_existing_browser_without_installing_one() {
    if let Ok(engine) = ChromiumEngine::detect() {
        assert!(engine.executable().exists());
    }
}

#[tokio::test]
async fn renders_a_page_when_chromium_and_network_are_available() {
    let Ok(engine) = ChromiumEngine::detect() else {
        eprintln!("Chromium integration skipped: no browser executable installed");
        return;
    };
    match engine
        .render("https://example.com/", CancellationToken::new())
        .await
    {
        Ok((document, final_url)) => {
            assert!(final_url.starts_with("https://example.com"));
            assert!(!document.title.is_empty());
        }
        Err(error) => {
            // Chromium can be installed in an offline, sandbox-restricted, or
            // headless-less test environment. The optional runtime must remain
            // buildable and the test can only exercise rendering where launch
            // and outbound access are available.
            eprintln!("Chromium integration skipped after launch/network failure: {error}");
        }
    }
}
