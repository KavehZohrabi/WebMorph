use std::io::Write;

use flate2::{Compression, write::GzEncoder};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use webmorph::{
    compatibility::{PageAssessment, assess},
    html,
    network::{HttpClient, HttpConfig},
    transform::transform,
};

fn response_for(path: &str) -> (Vec<u8>, &'static str, bool) {
    let (body, content_type, gzip) = match path {
        "/docs" => (b"<main><h1>Reference</h1><p>Documentation body with enough text to verify content remains visible.</p><pre><code>let x = 1;</code></pre></main>".to_vec(), "text/html; charset=utf-8", false),
        "/blog" => (b"<article><h1>caf\xe9</h1><p>Un texte fran\xe7ais.</p></article>".to_vec(), "text/html; charset=windows-1252", false),
        "/repo" => (b"<main><h1>owner/project</h1><nav><a href=\"/tree/main\">Files</a><a href=\"/issues\">Issues</a></nav><p>Repository overview with descriptive text.</p></main>".to_vec(), "text/html", false),
        "/search" => (b"<main><h1>Search results</h1><ol><li><a href=\"/one\">First result</a></li><li><a href=\"/two\">Second result</a></li></ol></main>".to_vec(), "text/html", false),
        "/shell" => (b"<div id=\"root\"></div><script type=\"module\" src=\"/assets/app.js\"></script>".to_vec(), "text/html", false),
        "/forms" => (b"<main><h1>Data</h1><table><tr><th>Name</th><th>Value</th></tr><tr><td>Count</td><td>3</td></tr></table><form action=\"/submit\"><label for=\"q\">Query</label><input id=\"q\" name=\"q\"><button>Search</button></form></main>".to_vec(), "text/html", false),
        "/compressed" => (b"<main><h1>Compressed</h1><p>Gzip encoded website content successfully decoded.</p></main>".to_vec(), "text/html", true),
        "/redirect" => return (b"".to_vec(), "", false),
        _ => (b"<h1>Redirect destination</h1><p>This final page has visible content.</p>".to_vec(), "text/html", false),
    };
    (body, content_type, gzip)
}

async fn start_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..9 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let size = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..size]);
            let path = request.split_whitespace().nth(1).unwrap_or("/");
            let (mut body, content_type, gzip) = response_for(path);
            if path == "/redirect" {
                stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /destination\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                continue;
            }
            let encoding = if gzip {
                let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(&body).unwrap();
                body = encoder.finish().unwrap();
                "Content-Encoding: gzip\r\n"
            } else {
                ""
            };
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n{encoding}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        }
    });
    format!("http://{address}")
}

#[tokio::test]
async fn local_realistic_pages_are_decoded_parsed_and_renderable() {
    let base = start_server().await;
    let client = HttpClient::new(HttpConfig {
        allow_private_network: true,
        ..HttpConfig::default()
    })
    .unwrap();

    for (path, expected) in [
        ("/docs", "Reference"),
        ("/blog", "café"),
        ("/repo", "owner/project"),
        ("/search", "Search results"),
        ("/forms", "Data"),
        ("/compressed", "Compressed"),
    ] {
        let url = HttpClient::parse_url(&format!("{base}{path}")).unwrap();
        let (final_url, source) = client
            .get_text_with_final_url_cancellable(&url, tokio_util::sync::CancellationToken::new())
            .await
            .unwrap();
        let document = html::parse(final_url.as_str(), &source);
        let page = transform(&document);
        assert!(!page.lines.is_empty(), "{path} rendered no lines");
        assert!(
            page.lines.join(" ").contains(expected),
            "{path} missing {expected}"
        );
        assert_eq!(assess(&source, &document), PageAssessment::Useful);
        if path == "/forms" {
            assert!(!document.forms.is_empty());
            assert!(page.lines.join(" ").contains("Name"));
        }
    }

    let shell_url = HttpClient::parse_url(&format!("{base}/shell")).unwrap();
    let (final_url, source) = client
        .get_text_with_final_url_cancellable(&shell_url, tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    let shell = html::parse(final_url.as_str(), &source);
    assert_eq!(assess(&source, &shell), PageAssessment::JavaScriptShell);
    assert!(
        !transform(&shell).lines.is_empty(),
        "app shell must explain missing content"
    );

    let redirect_url = HttpClient::parse_url(&format!("{base}/redirect")).unwrap();
    let (final_url, source) = client
        .get_text_with_final_url_cancellable(
            &redirect_url,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(final_url.path().ends_with("/destination"));
    assert!(source.contains("Redirect destination"));
}

#[tokio::test]
async fn follows_a_legitimate_cross_host_redirect() {
    let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination_port = destination.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut socket, _) = destination.accept().await.unwrap();
        let mut request = [0u8; 1024];
        let _ = socket.read(&mut request).await.unwrap();
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 20\r\nConnection: close\r\n\r\n<h1>destination</h1>").await.unwrap();
    });
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_address = origin.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = origin.accept().await.unwrap();
        let mut request = [0u8; 1024];
        let _ = socket.read(&mut request).await.unwrap();
        let response = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://localhost:{destination_port}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    let client = HttpClient::new(HttpConfig {
        allow_private_network: true,
        ..HttpConfig::default()
    })
    .unwrap();
    let url = HttpClient::parse_url(&format!("http://{origin_address}/start")).unwrap();
    let (final_url, body) = client
        .get_text_with_final_url_cancellable(&url, tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(final_url.host_str(), Some("localhost"));
    assert!(body.contains("destination"));
}

#[test]
fn parser_preserves_all_siblings_inside_uncommon_standard_containers() {
    let source = "<html><body><summary><p>first</p><p>second</p></summary><pre><code>alpha</code><code>beta</code></pre></body></html>";
    let document = html::parse("https://example.test/", source);
    let page = transform(&document);
    let rendered = page.lines.join(" ");
    assert!(rendered.contains("first"));
    assert!(rendered.contains("second"));
    assert!(rendered.contains("alpha"));
    assert!(rendered.contains("beta"));
}
