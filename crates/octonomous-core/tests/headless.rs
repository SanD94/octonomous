use octonomous_core::{
    auth::Credentials,
    events::{Envelope, Signal},
    interaction::PermissionReply,
    reconcile::Reconciler,
    session::Delivery,
    transport::Client,
};
use serde_json::json;
use wiremock::{
    Mock, MockBuilder, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

fn authenticated(mock: MockBuilder) -> MockBuilder {
    mock.and(header("authorization", "Basic b3BlbmNvZGU6c2VjcmV0"))
}

#[tokio::test]
async fn wiremock_drives_the_headless_rest_lifecycle_and_reconnect_poll() {
    let server = MockServer::start().await;
    let session = json!({
        "id": "ses_1",
        "projectID": "project",
        "cost": 0,
        "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}},
        "time": {"created": 1, "updated": 1},
        "location": {"directory": "/project"}
    });
    authenticated(
        Mock::given(method("POST"))
            .and(path("/api/session"))
            .and(body_json(json!({"location":{"directory":"/project"}}))),
    )
    .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":session})))
    .expect(1)
    .mount(&server)
    .await;
    authenticated(Mock::given(method("GET")).and(path("/api/session/ses_1/message")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data":[{"id":"msg_1", "type":"user", "text":"existing"}],
            "cursor":{}
        })))
        .expect(2)
        .mount(&server)
        .await;
    authenticated(Mock::given(method("GET")).and(path("/api/session/ses_1/permission")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{
            "id":"per_1", "sessionID":"ses_1", "action":"bash",
            "resources":["cargo test"]
        }]})))
        .expect(2)
        .mount(&server)
        .await;
    authenticated(Mock::given(method("GET")).and(path("/api/session/ses_1/form")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[]})))
        .expect(2)
        .mount(&server)
        .await;
    authenticated(
        Mock::given(method("POST"))
            .and(path("/api/session/ses_1/prompt"))
            .and(body_json(json!({"delivery":"queue", "text":"hello"}))),
    )
    .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{
        "id":"msg_prompt", "sessionID":"ses_1", "delivery":"queue",
        "payload":{"text":"hello"}, "time":{"created":2}, "type":"user"
    }})))
    .expect(1)
    .mount(&server)
    .await;
    authenticated(
        Mock::given(method("POST"))
            .and(path("/api/session/ses_1/permission/per_1/reply"))
            .and(body_json(json!({"decision":"once"}))),
    )
    .respond_with(ResponseTemplate::new(204))
    .expect(1)
    .mount(&server)
    .await;
    authenticated(Mock::given(method("POST")).and(path("/api/session/ses_1/interrupt")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"interrupted":true})))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::new(
        &server.uri(),
        &Credentials::from_password("secret").unwrap(),
    )
    .unwrap();
    let created = client.create_session("/project").await.unwrap();
    let mut reconciler = Reconciler::new(client.clone(), created.id.to_string());
    let connected = Signal::Event(
        Envelope::parse(r#"{"id":"evt_connected","type":"server.connected","data":{}}"#).unwrap(),
    );

    reconciler.handle(&connected).await.unwrap();
    assert_eq!(reconciler.state().transcript[0].text, "existing");
    assert_eq!(reconciler.state().pending_permissions[0].id, "per_1");
    let prompt = client
        .prompt("ses_1", "hello", Delivery::Queue)
        .await
        .unwrap();
    assert_eq!(prompt.id.to_string(), "msg_prompt");
    client
        .reply_permission("ses_1", "per_1", PermissionReply::Once)
        .await
        .unwrap();
    assert!(client.interrupt("ses_1").await.unwrap());

    // A second server.connected models the authoritative refresh performed
    // after the stream reconnects.
    reconciler.handle(&connected).await.unwrap();
}
