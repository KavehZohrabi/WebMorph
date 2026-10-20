#![cfg(feature = "browser-engine")]

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;
use webmorph::{
    browser::{BrowserBackend, BrowserInteraction, ChromiumConfig, ChromiumEngine},
    network::HttpClient,
    transform::transform,
};

async fn local_page() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request).await;
            let body = r#"<!doctype html><html><head><title>Interactive fixture</title></head><body>
                <main><h1 id="result">Ready</h1>
                <span id="aria-action-label">Accessible action</span><button aria-labelledby="aria-action-label" onclick="document.querySelector('#result').textContent='Accessible click'"></button>
                <details><summary>More details</summary><p>Revealed disclosure content</p></details>
                <dialog open aria-modal="true"><p>Modal dialog content</p><button onclick="this.closest('dialog').close()">Close dialog</button></dialog>
                <a id="route" href="/client-route" aria-label="Client route">Route</a>
                <button id="dynamic" aria-label="Dynamic button" onclick="document.querySelector('#result').textContent='Clicked'; document.body.insertAdjacentHTML('afterbegin', '<button aria-label=&quot;Inserted later&quot;>Later</button>')">Click</button>
                <form id="form" onsubmit="event.preventDefault(); document.querySelector('#result').textContent='Submitted: '+document.querySelector('#message').value+' / '+document.querySelector('#choice').value">
                  <input id="message" aria-label="Message" value="">
                  <select id="choice" aria-label="Choice"><option value="one">One</option><option value="two">Two</option></select>
                  <button id="send" type="submit">Send</button>
                </form>
                <p id="scroll-indicator">Scroll: 0</p><div style="height:2400px"></div>
                <script>
                  document.querySelector('#route').addEventListener('click', event => { event.preventDefault(); history.pushState({}, '', '/client-route'); document.querySelector('#result').textContent='Client route loaded'; });
                  addEventListener('scroll', () => document.querySelector('#scroll-indicator').textContent='Scroll: '+Math.round(scrollY));
                </script></main></body></html>"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            if socket.write_all(response.as_bytes()).await.is_err() {
                return;
            }
        }
    });
    format!("http://{address}/")
}

fn text(page: &webmorph::dom::Document) -> String {
    transform(page).lines.join(" ")
}

#[tokio::test]
async fn persistent_chromium_session_applies_real_dom_actions_and_refreshes_tui_document() {
    let url = local_page().await;
    let Ok(engine) = ChromiumEngine::detect() else {
        eprintln!("Chromium interaction integration skipped: no browser executable installed");
        return;
    };
    let engine = engine.with_config(ChromiumConfig {
        allow_private_network: true,
        navigation_timeout: std::time::Duration::from_secs(12),
        ..ChromiumConfig::default()
    });
    let started = std::time::Instant::now();
    let token = CancellationToken::new();
    let (page, _) = engine.navigate(17, &url, token.clone()).await.unwrap();
    assert!(text(&page).contains("Ready"));
    assert!(
        page.interactive
            .iter()
            .any(|element| element.label == "Click")
    );
    assert!(
        page.interactive
            .iter()
            .any(|element| element.label == "Route")
    );
    let accessible_action = page
        .interactive
        .iter()
        .find(|element| element.label == "Accessible action")
        .unwrap()
        .id
        .clone();
    let (page, _) = engine
        .interact(
            17,
            BrowserInteraction::Click {
                interactive_id: accessible_action,
            },
            token.clone(),
        )
        .await
        .unwrap();
    assert!(text(&page).contains("Accessible click"));
    let disclosure = page
        .interactive
        .iter()
        .find(|element| element.label == "More details")
        .unwrap()
        .id
        .clone();
    let (page, _) = engine
        .interact(
            17,
            BrowserInteraction::Click {
                interactive_id: disclosure,
            },
            token.clone(),
        )
        .await
        .unwrap();
    assert!(text(&page).contains("Revealed disclosure content"));
    let (page, _) = engine
        .interact(17, BrowserInteraction::DismissDialog, token.clone())
        .await
        .unwrap();
    assert!(!text(&page).contains("Modal dialog content"));
    let route_key = page
        .interactive
        .iter()
        .find(|element| element.label == "Route")
        .unwrap()
        .html_id
        .clone();

    let button = page
        .interactive
        .iter()
        .find(|element| element.label == "Click")
        .unwrap()
        .id
        .clone();
    let (page, _) = engine
        .interact(
            17,
            BrowserInteraction::Click {
                interactive_id: button,
            },
            token.clone(),
        )
        .await
        .unwrap();
    assert!(text(&page).contains("Clicked"));
    assert_eq!(
        page.interactive
            .iter()
            .find(|element| element.label == "Route")
            .unwrap()
            .html_id,
        route_key,
        "existing browser nodes keep their keys when new controls are inserted"
    );

    let input = page
        .interactive
        .iter()
        .find(|element| element.label == "Message")
        .unwrap()
        .id
        .clone();
    let (page, _) = engine
        .interact(
            17,
            BrowserInteraction::SetValue {
                interactive_id: input,
                value: "héllo".into(),
            },
            token.clone(),
        )
        .await
        .unwrap();
    assert!(
        page.forms
            .iter()
            .flat_map(|form| &form.controls)
            .any(|control| control.label == "Message" && control.value == "héllo")
    );

    let select = page
        .interactive
        .iter()
        .find(|element| element.label == "Choice")
        .unwrap()
        .id
        .clone();
    let (page, _) = engine
        .interact(
            17,
            BrowserInteraction::SelectOption {
                interactive_id: select,
                value: "two".into(),
            },
            token.clone(),
        )
        .await
        .unwrap();
    assert!(
        page.forms
            .iter()
            .flat_map(|form| &form.controls)
            .any(|control| control.label == "Choice" && control.value == "two")
    );

    let submit = page
        .interactive
        .iter()
        .find(|element| element.label == "Send")
        .unwrap()
        .id
        .clone();
    let (page, _) = engine
        .interact(
            17,
            BrowserInteraction::Click {
                interactive_id: submit,
            },
            token.clone(),
        )
        .await
        .unwrap();
    assert!(text(&page).contains("Submitted: héllo / two"));

    let route = page
        .interactive
        .iter()
        .find(|element| element.label == "Route")
        .unwrap()
        .id
        .clone();
    let (page, final_url) = engine
        .interact(
            17,
            BrowserInteraction::Click {
                interactive_id: route,
            },
            token.clone(),
        )
        .await
        .unwrap();
    assert!(final_url.ends_with("/client-route"));
    assert!(text(&page).contains("Client route loaded"));

    let (page, _) = engine
        .interact(17, BrowserInteraction::Scroll { delta: 600 }, token)
        .await
        .unwrap();
    assert!(text(&page).contains("Scroll: 600"));
    engine.close_session(17).await;
    assert_eq!(engine.active_session_count().await, 0);
    eprintln!(
        "Chromium local compatibility: navigation, dynamic action, form, SPA route, scrolling; {:.2} ms; sessions after close: 0",
        started.elapsed().as_secs_f64() * 1_000.0
    );
}

#[test]
fn invalid_browser_address_is_rejected_before_launch() {
    assert!(HttpClient::parse_url("file:///etc/passwd").is_err());
}
