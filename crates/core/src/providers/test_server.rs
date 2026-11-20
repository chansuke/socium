//! A one-shot loopback HTTP server for provider tests.
#![expect(
    clippy::disallowed_methods,
    reason = "the server paces its reply to test the client's idle timeout"
)]

/// How long a test server waits for its client before failing the test, so
/// a client that never connects or never finishes its request cannot hang
/// the suite.
const WATCHDOG: std::time::Duration = std::time::Duration::from_secs(10);

/// Serves one HTTP response on a loopback port and returns the request.
pub(crate) async fn serve_once(
    status: &'static str,
    body: String,
) -> (String, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(WATCHDOG, listener.accept())
            .await
            .expect("the client never connected to the test server")
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let read = tokio::time::timeout(WATCHDOG, socket.read(&mut buffer))
                .await
                .expect("the client stopped sending its request to the test server")
                .unwrap();
            request.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&request);
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break;
                }
            }
            if read == 0 {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8_lossy(&request).into_owned()
    });
    (base_url, handle)
}

/// Reads one request and closes the connection without a response, as a
/// server does to a kept-alive connection it already gave up. Returns the
/// base URL.
pub(crate) async fn serve_hang_up() -> String {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(WATCHDOG, listener.accept())
            .await
            .expect("the client never connected to the test server")
            .unwrap();
        let mut buffer = [0u8; 65536];
        let _ = socket.read(&mut buffer).await;
    });
    base_url
}

/// Serves one SSE response whose events arrive with the given delays, then
/// closes. Returns the base URL.
pub(crate) async fn serve_paced(events: Vec<(std::time::Duration, String)>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(WATCHDOG, listener.accept())
            .await
            .expect("the client never connected to the test server")
            .unwrap();
        let mut buffer = [0u8; 65536];
        let _ = socket.read(&mut buffer).await;
        let _ = socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
            )
            .await;
        for (delay, event) in events {
            tokio::time::sleep(delay).await;
            if socket.write_all(event.as_bytes()).await.is_err() {
                return;
            }
        }
    });
    base_url
}
