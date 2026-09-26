use octonomous_core::{auth::Credentials, envelope::Error, generated, transport::Client};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn server(
    response_status: &'static str,
    response_body: &'static str,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let read = connection.read(&mut request).await.unwrap();
        request.truncate(read);

        let response = format!(
            "HTTP/1.1 {response_status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response_body}",
            response_body.len(),
        );
        connection.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    (format!("http://{address}"), task)
}

#[tokio::test]
async fn authenticated_info_request_sends_basic_auth_and_returns_version() {
    let (url, server) = server(
        "200 OK",
        r#"{"version":"2.0.18","pid":42,"urls":[],"paths":{"tmp":"/tmp"}}"#,
    )
    .await;
    let credentials = Credentials::from_password("secret").unwrap();
    let info = Client::new(&url, &credentials)
        .unwrap()
        .server_info()
        .await
        .unwrap();

    assert_eq!(info.version, "2.0.18");
    let request = server.await.unwrap();
    assert!(
        request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Basic b3BlbmNvZGU6c2VjcmV0")),
        "request did not contain the expected Basic authorization header: {request}"
    );
}

#[tokio::test]
async fn unauthenticated_info_request_returns_typed_unauthorized() {
    let (url, server) = server(
        "401 Unauthorized",
        r#"{"_tag":"UnauthorizedError","message":"Authentication required"}"#,
    )
    .await;
    let error = generated::Client::new(&url)
        .server_info()
        .await
        .map_err(Error::from_generated)
        .unwrap_err();

    assert!(matches!(
        error,
        Error::Unauthorized { message } if message == "Authentication required"
    ));
    let request = server.await.unwrap();
    assert!(!request.to_ascii_lowercase().contains("authorization:"));
}

#[tokio::test]
#[ignore = "requires a running local OpenCode service"]
async fn live_authenticated_server_info() {
    let info = Client::discover(None).unwrap().server_info().await.unwrap();

    assert!(!info.version.is_empty());
}
