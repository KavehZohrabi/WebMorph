#![cfg(feature = "browser-engine")]

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;
use webmorph::browser::{BrowserBackend, ChromiumConfig, ChromiumEngine};

#[test]
fn detects_an_existing_browser_without_installing_one() {
    if let Ok(engine) = ChromiumEngine::detect() {
        assert!(engine.executable().exists());
    }
}

#[tokio::test]
async fn renders_a_deterministic_local_fixture_when_chromium_is_available() {
    let Ok(engine) = ChromiumEngine::detect() else {
        eprintln!("Chromium integration skipped: no browser executable installed");
        return;
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 2048];
        let _ = stream.read(&mut request).await;
        let body = include_str!("fixtures/static.html");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let engine = engine.with_config(ChromiumConfig {
        allow_private_network: true,
        ..ChromiumConfig::default()
    });
    let (document, final_url) = engine
        .navigate(
            1001,
            &format!("http://{address}/fixture"),
            CancellationToken::new(),
        )
        .await
        .expect("local fixture should render");
    assert!(final_url.ends_with("/fixture"));
    assert_eq!(document.title, "Static fixture");
    engine.close_session(1001).await;
    assert_eq!(engine.active_session_count().await, 0);
}
