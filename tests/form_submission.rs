use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};
use webmorph::{
    html,
    network::{HttpClient, HttpConfig},
};

const FORM_PAGE: &str = r#"<html><body>
<form id="search" action="/results" method="get"><input name="q" type="search" required></form>
<form id="login" action="/login" method="post"><input name="username"><input name="password" type="password"></form>
</body></html>"#;

async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut request = Vec::new();
    let mut chunk = [0; 2048];
    let mut expected = None;
    loop {
        let count = stream.read(&mut chunk).await.unwrap();
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
        let text = String::from_utf8_lossy(&request);
        if let Some(split) = text.find("\r\n\r\n") {
            let headers = &text[..split];
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            expected = Some(split + 4 + length);
        }
        if expected.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    String::from_utf8_lossy(&request).into_owned()
}

async fn test_site() -> (String, oneshot::Receiver<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let mut requests = Vec::new();
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            requests.push(request.clone());
            let body = if request.starts_with("GET /forms ") {
                FORM_PAGE
            } else {
                "<p>submitted</p>"
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
        let _ = tx.send(requests);
    });
    (format!("http://{address}"), rx)
}

fn local_client() -> HttpClient {
    HttpClient::new(HttpConfig {
        allow_private_network: true,
        ..HttpConfig::default()
    })
    .unwrap()
}

#[tokio::test]
async fn local_site_forms_cover_search_login_get_and_post() {
    let (origin, requests_rx) = test_site().await;
    let client = local_client();
    let page_url = HttpClient::parse_url(&format!("{origin}/forms")).unwrap();
    let source = client.get_text(&page_url).await.unwrap();
    let document = html::parse(page_url.as_str(), &source);
    assert_eq!(document.forms.len(), 2);
    assert_eq!(document.forms[0].method, "get");
    assert_eq!(document.forms[1].method, "post");
    assert_eq!(document.forms[1].controls[1].control_type, "password");
    assert!(document.forms[1].controls[1].value.is_empty());

    let get_url = HttpClient::append_form_query(
        &HttpClient::parse_url(&format!("{origin}/results")).unwrap(),
        &[("q".into(), "cats & cafés".into())],
    );
    client.get_text(&get_url).await.unwrap();
    client
        .submit_form_cancellable(
            &HttpClient::parse_url(&format!("{origin}/login")).unwrap(),
            vec![
                ("username".into(), "Zoë".into()),
                ("password".into(), "secret value".into()),
            ],
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    let requests = requests_rx.await.unwrap();
    assert!(requests[1].starts_with("GET /results?q=cats+%26+caf%C3%A9s "));
    assert!(requests[2].starts_with("POST /login HTTP/1.1"));
    assert!(
        requests[2]
            .to_ascii_lowercase()
            .contains("content-type: application/x-www-form-urlencoded")
    );
    assert!(requests[2].ends_with("username=Zo%C3%AB&password=secret+value"));
}
