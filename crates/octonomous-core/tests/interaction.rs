use std::collections::HashMap;

use octonomous_core::{
    auth::Credentials, envelope::Error, interaction::PermissionReply, transport::Client,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

async fn read_request(connection: &mut TcpStream) -> String {
    let mut request = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let read = connection.read(&mut chunk).await.unwrap();
        request.extend_from_slice(&chunk[..read]);
        let header_end = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap_or(usize::MAX);
        if header_end == usize::MAX {
            continue;
        }
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .map(str::to_owned)
            })
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_default();
        if request.len() >= header_end + 4 + length {
            return String::from_utf8(request).unwrap();
        }
    }
}

async fn serve(status: &str, body: Value) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let status = status.to_owned();
    let task = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        let request = read_request(&mut connection).await;
        let body = body.to_string();
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        connection.write_all(response.as_bytes()).await.unwrap();
        request
    });
    (format!("http://{address}"), task)
}

fn client(url: &str) -> Client {
    Client::new(url, &Credentials::from_password("secret").unwrap()).unwrap()
}

#[tokio::test]
async fn permission_reply_sends_decision_and_surfaces_a_duplicate() {
    let (url, server) = serve("204 No Content", Value::Null).await;
    client(&url)
        .reply_permission("ses_1", "per_1", PermissionReply::Always)
        .await
        .unwrap();
    let request = server.await.unwrap();
    assert!(request.starts_with("POST /api/session/ses_1/permission/per_1/reply "));
    let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body, json!({"decision": "always"}));

    let (url, _) = serve(
        "404 Not Found",
        json!({"_tag":"PermissionNotFoundError", "requestID":"per_1", "message":"not pending"}),
    )
    .await;
    let error = client(&url)
        .reply_permission("ses_1", "per_1", PermissionReply::Reject)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::PermissionUnavailable { request_id, .. } if request_id == "per_1")
    );
}

#[tokio::test]
async fn form_reply_and_cancel_use_the_selected_form() {
    let (url, reply_server) = serve("204 No Content", Value::Null).await;
    client(&url)
        .reply_form(
            "ses_1",
            "frm_1",
            HashMap::from([("approved".into(), json!(true))]),
        )
        .await
        .unwrap();
    let request = reply_server.await.unwrap();
    assert!(request.starts_with("POST /api/session/ses_1/form/frm_1/reply "));
    let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body, json!({"answer":{"approved":true}}));

    let (url, cancel_server) = serve("204 No Content", Value::Null).await;
    client(&url).cancel_form("ses_1", "frm_1").await.unwrap();
    assert!(
        cancel_server
            .await
            .unwrap()
            .starts_with("DELETE /api/session/ses_1/form/frm_1 ")
    );
}

#[tokio::test]
async fn interrupt_and_file_search_preserve_protocol_parameters() {
    let (url, interrupt_server) = serve("200 OK", json!({"interrupted":true})).await;
    assert!(client(&url).interrupt("ses_1").await.unwrap());
    assert!(
        interrupt_server
            .await
            .unwrap()
            .starts_with("POST /api/session/ses_1/interrupt ")
    );

    let (url, find_server) = serve(
        "200 OK",
        json!({"location":{"directory":"/project"}, "data":[{"path":"src/lib.rs", "type":"file"}]}),
    )
    .await;
    let entries = client(&url).find_files("/project", "lib", 7).await.unwrap();
    assert_eq!(entries[0].path, "src/lib.rs");
    let request = find_server.await.unwrap();
    assert!(request.starts_with("GET /api/fs/find?"), "{request}");
    assert!(request.contains("limit=7"), "{request}");
    assert!(request.contains("query=lib"), "{request}");
    assert!(request.contains("directory"), "{request}");
    assert!(request.contains("%2Fproject"), "{request}");
}
