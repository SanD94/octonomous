use octonomous_core::generated::Client;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn generated_client_calls_server_info() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        let mut request = [0; 2048];
        let read = connection.read(&mut request).await.unwrap();
        let request = std::str::from_utf8(&request[..read]).unwrap();
        assert!(request.starts_with("GET /api/info HTTP/1.1\r\n"));

        let body =
            r#"{"version":"2.0.18","pid":42,"urls":["http://127.0.0.1"],"paths":{"tmp":"/tmp"}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body,
        );
        connection.write_all(response.as_bytes()).await.unwrap();
    });

    let client = Client::new(&format!("http://{address}"));
    let info = client.server_info().await.unwrap();

    assert_eq!(info.version, "2.0.18");
    assert_eq!(info.pid, 42);
    assert_eq!(info.paths.tmp, "/tmp");
    server.await.unwrap();
}
