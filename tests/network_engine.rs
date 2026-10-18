use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use webmorph::{
    error::WebMorphError,
    network::{HttpClient, HttpConfig},
};

#[tokio::test]
async fn fetches_html_and_classifies_server_statuses() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for (status, content_type, body) in [
            ("200 OK", "text/html", "<h1>local page</h1>"),
            ("503 Service Unavailable", "text/html", ""),
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await;
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });

    let client = HttpClient::new(HttpConfig {
        allow_private_network: true,
        ..HttpConfig::default()
    })
    .unwrap();
    let first = HttpClient::parse_url(&format!("http://{address}/page")).unwrap();
    assert_eq!(
        client.get_text(&first).await.unwrap(),
        "<h1>local page</h1>"
    );
    let second = HttpClient::parse_url(&format!("http://{address}/error")).unwrap();
    assert!(matches!(
        client.get_text(&second).await,
        Err(WebMorphError::HttpStatus(503))
    ));
}
