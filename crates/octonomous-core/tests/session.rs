use octonomous_core::{
    auth::Credentials,
    envelope::Error,
    events::{Envelope, Signal},
    reconcile::{Reconciler, Role},
    session::Delivery,
    transport::Client,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

fn session(directory: &str, id: &str) -> Value {
    json!({
        "id": id,
        "projectID": "project",
        "cost": 0,
        "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}},
        "time": {"created": 1, "updated": 1},
        "location": {"directory": directory}
    })
}

async fn read_request(connection: &mut TcpStream) -> String {
    let mut request = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let read = connection.read(&mut chunk).await.unwrap();
        request.extend_from_slice(&chunk[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .map(str::to_owned)
            })
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            return String::from_utf8(request).unwrap();
        }
    }
}

async fn respond(connection: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    connection.write_all(response.as_bytes()).await.unwrap();
}

async fn one_response(body: Value) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        let request = read_request(&mut connection).await;
        respond(&mut connection, &body.to_string()).await;
        request
    });
    (format!("http://{address}"), task)
}

fn client(url: &str) -> Client {
    Client::new(url, &Credentials::from_password("secret").unwrap()).unwrap()
}

#[tokio::test]
async fn create_sends_location_in_body_and_accepts_an_exact_match() {
    let (url, server) = one_response(json!({"data": session("/wanted", "ses_1")})).await;

    let created = client(&url).create_session("/wanted").await.unwrap();

    assert_eq!(created.location.directory, "/wanted");
    let request = server.await.unwrap();
    let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body, json!({"location": {"directory": "/wanted"}}));
}

#[tokio::test]
async fn create_rejects_a_silently_wrong_working_directory() {
    let (url, _server) = one_response(json!({"data": session("/server-cwd", "ses_1")})).await;

    let error = client(&url).create_session("/wanted").await.unwrap_err();

    assert!(matches!(
        error,
        Error::WrongDirectory { requested, actual }
            if requested == "/wanted" && actual == "/server-cwd"
    ));
}

#[tokio::test]
async fn list_sessions_follows_next_cursors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for body in [
            json!({"data": [session("/one", "ses_1")], "cursor": {"next": "page-2"}}),
            json!({"data": [session("/two", "ses_2")], "cursor": {"previous": "page-1"}}),
        ] {
            let (mut connection, _) = listener.accept().await.unwrap();
            requests.push(read_request(&mut connection).await);
            respond(&mut connection, &body.to_string()).await;
        }
        requests
    });

    let sessions = client(&format!("http://{address}"))
        .list_sessions(1)
        .await
        .unwrap();

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].id.to_string(), "ses_1");
    assert_eq!(sessions[1].id.to_string(), "ses_2");
    let requests = server.await.unwrap();
    assert!(requests[0].starts_with("GET /api/session?limit=1&order=asc "));
    assert!(requests[1].contains("cursor=page-2"));
}

#[tokio::test]
async fn message_history_follows_next_cursors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for body in [
            json!({"data": [{"id": "msg_1", "type": "user", "text": "one"}], "cursor": {"next": "page-2"}}),
            json!({"data": [{"id": "msg_2", "type": "user", "text": "two"}], "cursor": {"previous": "page-1"}}),
        ] {
            let (mut connection, _) = listener.accept().await.unwrap();
            requests.push(read_request(&mut connection).await);
            respond(&mut connection, &body.to_string()).await;
        }
        requests
    });

    let messages = client(&format!("http://{address}"))
        .session_messages("ses_1", 1)
        .await
        .unwrap();

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["id"], "msg_1");
    assert_eq!(messages[1]["id"], "msg_2");
    let requests = server.await.unwrap();
    assert!(requests[0].contains("limit=1&order=asc"));
    assert!(requests[1].contains("cursor=page-2"));
}

#[tokio::test]
async fn prompt_sends_the_selected_delivery_and_returns_the_inbox_item() {
    let response = json!({"data": {
        "id": "msg_1",
        "sessionID": "ses_1",
        "delivery": "queue",
        "payload": {"text": "hello"},
        "time": {"created": 1},
        "type": "user"
    }});
    let (url, server) = one_response(response).await;

    let inbox = client(&url)
        .prompt("ses_1", "hello", Delivery::Queue)
        .await
        .unwrap();

    assert_eq!(inbox.delivery, Delivery::Queue);
    let request = server.await.unwrap();
    let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body, json!({"delivery": "queue", "text": "hello"}));
}

#[tokio::test]
async fn server_connected_repolls_authoritative_messages() {
    let response = json!({
        "data": [{
            "id": "msg_1",
            "text": "authoritative",
            "time": {"created": 1},
            "type": "user"
        }],
        "cursor": {}
    });
    let (url, server) = one_response(response).await;
    let mut reconciler = Reconciler::new(client(&url), "ses_1");
    let connected = Signal::Event(
        Envelope::parse(r#"{"id":"evt_connected","type":"server.connected","data":{}}"#).unwrap(),
    );

    let update = reconciler.handle(&connected).await.unwrap();

    assert_eq!(update.reconciliation.unwrap().after, 1);
    assert_eq!(reconciler.state().transcript[0].role, Role::User);
    assert_eq!(reconciler.state().transcript[0].text, "authoritative");
    let request = server.await.unwrap();
    assert!(request.starts_with("GET /api/session/ses_1/message?limit=100&order=asc "));
}
