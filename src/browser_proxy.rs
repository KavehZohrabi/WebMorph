//! Per-session loopback egress proxy for Chromium. Every destination is
//! resolved and checked before the proxy connects to the selected IP, so page
//! scripts can use public CDNs without giving Chromium unrestricted DNS access.

use std::{net::SocketAddr, time::Duration};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, copy_bidirectional},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use crate::{
    error::{Result, WebMorphError},
    network::{HttpClient, resolve_http_destination},
};

pub(crate) struct EgressProxy {
    port: u16,
    cancellation: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl EgressProxy {
    pub(crate) async fn bind(allow_private_network: bool) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(WebMorphError::Terminal)?;
        let port = listener
            .local_addr()
            .map_err(WebMorphError::Terminal)?
            .port();
        let cancellation = CancellationToken::new();
        let shutdown = cancellation.clone();
        let task = tokio::spawn(async move {
            let permits = std::sync::Arc::new(Semaphore::new(32));
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = connections.join_next(), if !connections.is_empty() => {},
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break; };
                        let Ok(permit) = permits.clone().try_acquire_owned() else { continue; };
                        let shutdown = shutdown.clone();
                        connections.spawn(async move {
                            let _permit = permit;
                            tokio::select! {
                                _ = shutdown.cancelled() => {},
                                result = handle_connection(stream, allow_private_network) => {
                                    if let Err(error) = result {
                                        tracing::debug!(%error, "Chromium egress proxy rejected or lost a request");
                                    }
                                }
                            }
                        });
                    }
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Ok(Self {
            port,
            cancellation,
            task: Some(task),
        })
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) async fn close(mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for EgressProxy {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn handle_connection(client: TcpStream, allow_private_network: bool) -> Result<()> {
    let mut reader = BufReader::new(client);
    let mut head = Vec::with_capacity(1024);
    loop {
        let mut line = Vec::new();
        let read = timeout(Duration::from_secs(10), reader.read_until(b'\n', &mut line))
            .await
            .map_err(|_| WebMorphError::Timeout)?
            .map_err(WebMorphError::Terminal)?;
        if read == 0 {
            return Err(WebMorphError::Connection("incomplete proxy request".into()));
        }
        head.extend_from_slice(&line);
        if head.len() > 64 * 1024 {
            return Err(WebMorphError::ResponseTooLarge { limit: 64 * 1024 });
        }
        if line == b"\r\n" {
            break;
        }
    }
    let request = std::str::from_utf8(&head)
        .map_err(|_| WebMorphError::InvalidUrl("proxy request headers are not UTF-8".into()))?;
    let first_line = request
        .lines()
        .next()
        .ok_or_else(|| WebMorphError::InvalidUrl("missing proxy request line".into()))?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or("HTTP/1.1");

    if method.eq_ignore_ascii_case("CONNECT") {
        let url = HttpClient::parse_url(&format!("https://{target}/"))?;
        let port = url.port().unwrap_or(443);
        let destination = validated_socket(&url, port, allow_private_network).await?;
        let mut upstream = timeout(Duration::from_secs(10), TcpStream::connect(destination))
            .await
            .map_err(|_| WebMorphError::Timeout)?
            .map_err(|error| WebMorphError::Connection(error.to_string()))?;
        let mut client = reader.into_inner();
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .map_err(WebMorphError::Terminal)?;
        copy_bidirectional(&mut client, &mut upstream)
            .await
            .map_err(WebMorphError::Terminal)?;
        return Ok(());
    }

    let url = HttpClient::parse_url(target)?;
    if url.scheme() != "http" {
        return Err(WebMorphError::InvalidUrl(
            "non-CONNECT proxy requests must use HTTP".into(),
        ));
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let destination = validated_socket(&url, port, allow_private_network).await?;
    let mut upstream = timeout(Duration::from_secs(10), TcpStream::connect(destination))
        .await
        .map_err(|_| WebMorphError::Timeout)?
        .map_err(|error| WebMorphError::Connection(error.to_string()))?;
    let path = match (url.path(), url.query()) {
        (path, Some(query)) => format!("{path}?{query}"),
        (path, None) => path.to_owned(),
    };
    let mut forwarded = format!("{method} {path} {version}\r\n");
    for line in request.lines().skip(1) {
        if !line.is_empty()
            && !line.split_once(':').is_some_and(|(name, _)| {
                name.eq_ignore_ascii_case("proxy-connection")
                    || name.eq_ignore_ascii_case("connection")
            })
        {
            forwarded.push_str(line);
            forwarded.push_str("\r\n");
        }
    }
    forwarded.push_str("Connection: close\r\n\r\n");
    upstream
        .write_all(forwarded.as_bytes())
        .await
        .map_err(WebMorphError::Terminal)?;
    let buffered = reader.buffer().to_vec();
    reader.consume(buffered.len());
    upstream
        .write_all(&buffered)
        .await
        .map_err(WebMorphError::Terminal)?;
    let mut client = reader.into_inner();
    copy_bidirectional(&mut client, &mut upstream)
        .await
        .map_err(WebMorphError::Terminal)?;
    Ok(())
}

async fn validated_socket(
    url: &reqwest::Url,
    port: u16,
    allow_private_network: bool,
) -> Result<SocketAddr> {
    let address = resolve_http_destination(url, allow_private_network)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| WebMorphError::Dns("hostname resolved to no addresses".into()))?;
    Ok(SocketAddr::new(address, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn forwards_http_requests_through_the_per_session_proxy() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = origin.accept().await.unwrap();
            let mut request = vec![0; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                )
                .await
                .unwrap();
        });
        let proxy = EgressProxy::bind(true).await.unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", proxy.port()))
            .await
            .unwrap();
        client
            .write_all(
                format!("GET http://{origin_addr}/path HTTP/1.1\r\nHost: {origin_addr}\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        timeout(Duration::from_secs(2), client.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.ends_with(b"hello"));
        server.await.unwrap();
        proxy.close().await;
    }

    #[tokio::test]
    async fn rejects_local_destinations_when_private_access_is_disabled() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        let proxy = EgressProxy::bind(false).await.unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", proxy.port()))
            .await
            .unwrap();
        client
            .write_all(
                format!("GET http://{origin_addr}/ HTTP/1.1\r\nHost: {origin_addr}\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        timeout(Duration::from_secs(2), client.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.is_empty());
        assert!(
            timeout(Duration::from_millis(50), origin.accept())
                .await
                .is_err()
        );
        proxy.close().await;
    }
}
