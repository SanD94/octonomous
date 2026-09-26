use std::{collections::HashSet, time::Duration};

use futures_util::StreamExt;
use octonomous_core::{
    auth::Credentials,
    events::{Envelope, Event, EventStream, Signal},
    transport::Client,
};
use reqwest_eventsource::{Event as SseEvent, EventSource, retry::Never};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const FIXTURE: &str = include_str!("../../../docs/fixtures/prompt-basic-v2.0.18.sse");
const FIXTURE_SHA256: &str = "ee815cb8447964beddebdb4912cbc4aa20ef9137cdc116919b1f6a4049d0282c";

async fn fixture_server(body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        let mut request = [0; 2048];
        let read = connection.read(&mut request).await.unwrap();
        assert!(read > 0);
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        connection.write_all(response.as_bytes()).await.unwrap();
    });
    format!("http://{address}")
}

async fn reconnecting_server(first: &'static str, second: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for body in [first, second] {
            let (mut connection, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            let read = connection.read(&mut request).await.unwrap();
            assert!(read > 0);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            connection.write_all(response.as_bytes()).await.unwrap();
        }
    });
    format!("http://{address}")
}

#[test]
fn fixture_replays_the_expected_typed_event_sequence() {
    let actual: Vec<_> = FIXTURE
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|json| Envelope::parse(json).unwrap())
        .collect();

    assert_eq!(actual.len(), 69);
    assert_eq!(actual.first().unwrap().event_type, "server.connected");
    assert!(matches!(
        actual.first().unwrap().data,
        Event::ServerConnected
    ));
    assert!(
        actual
            .iter()
            .all(|event| !matches!(event.data, Event::Unknown(_)))
    );
    assert!(
        actual
            .iter()
            .any(|event| matches!(event.data, Event::SessionTextDelta(_)))
    );
    assert_eq!(
        actual.last().unwrap().event_type,
        "session.tool.input.started"
    );
}

#[test]
fn committed_fixture_has_not_drifted() {
    let actual = format!("{:x}", Sha256::digest(FIXTURE.as_bytes()));
    assert_eq!(
        actual, FIXTURE_SHA256,
        "the recorded fixture changed; verify it against OpenCode v2.0.18 and update the pinned digest deliberately"
    );
}

#[test]
fn unknown_event_preserves_its_payload() {
    let envelope = Envelope::parse(
        r#"{"id":"evt_new","created":42,"type":"session.future.changed","location":{"directory":"/tmp"},"data":{"answer":42},"durable":{"aggregateID":"ses_1","seq":9,"version":1}}"#,
    )
    .unwrap();

    assert_eq!(envelope.created, Some(42));
    assert_eq!(envelope.location.unwrap().directory, "/tmp");
    assert!(matches!(
        envelope.data,
        Event::Unknown(value) if value == serde_json::json!({"answer": 42})
    ));
}

#[tokio::test]
async fn eventsource_consumes_heartbeat_comments_without_emitting_messages() {
    let url = fixture_server(FIXTURE).await;
    let mut source = EventSource::get(url);
    source.set_retry_policy(Box::new(Never));
    let mut messages = 0;
    while let Some(item) = source.next().await {
        match item {
            Ok(SseEvent::Open) => {}
            Ok(SseEvent::Message(message)) => {
                Envelope::parse(&message.data).unwrap();
                messages += 1;
            }
            Err(_) => break,
        }
    }

    assert_eq!(
        messages, 69,
        "the three heartbeat comments must not surface"
    );
}

#[tokio::test]
async fn broadcast_delivers_events_to_multiple_consumers() {
    let first = r#"data: {"id":"evt_1","created":1,"type":"session.execution.started","data":{"sessionID":"ses_1"},"durable":{"aggregateID":"ses_1","seq":1,"version":1}}

"#;
    let url = fixture_server(first).await;
    let client = Client::new(&url, &Credentials::from_password("secret").unwrap()).unwrap();
    let stream = EventStream::connect(&client, 16).unwrap();
    let mut one = stream.subscribe();
    let mut two = stream.subscribe();

    let one = tokio::time::timeout(Duration::from_secs(2), one.recv())
        .await
        .unwrap()
        .unwrap();
    let two = tokio::time::timeout(Duration::from_secs(2), two.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(one, Signal::Event(_)));
    assert_eq!(one, two);
}

#[tokio::test]
async fn reconnect_reports_gap_and_suppresses_duplicate_durable_events() {
    let first = r#"data: {"id":"evt_1","created":1,"type":"session.execution.started","data":{"sessionID":"ses_1"},"durable":{"aggregateID":"ses_1","seq":1,"version":1}}

"#;
    let second = r#"data: {"id":"evt_connected","type":"server.connected","data":{}}

data: {"id":"evt_1_duplicate","created":1,"type":"session.execution.started","data":{"sessionID":"ses_1"},"durable":{"aggregateID":"ses_1","seq":1,"version":1}}

data: {"id":"evt_3","created":3,"type":"session.execution.succeeded","data":{"sessionID":"ses_1"},"durable":{"aggregateID":"ses_1","seq":3,"version":1}}

"#;
    let url = reconnecting_server(first, second).await;
    let client = Client::new(&url, &Credentials::from_password("secret").unwrap()).unwrap();
    let stream = EventStream::connect(&client, 32).unwrap();
    let mut receiver = stream.subscribe();
    let mut signals = Vec::new();

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let signal = receiver.recv().await.unwrap();
            let complete = matches!(
                &signal,
                Signal::Event(event)
                    if matches!(event.data, Event::SessionExecutionSucceeded(_))
            );
            signals.push(signal);
            if complete {
                break;
            }
        }
    })
    .await
    .unwrap();

    assert!(signals.contains(&Signal::Reconnected));
    assert!(signals.contains(&Signal::Duplicate {
        aggregate_id: "ses_1".into(),
        seq: 1,
    }));
    assert!(signals.contains(&Signal::Gap {
        aggregate_id: "ses_1".into(),
        expected: 2,
        observed: 3,
    }));
    assert_eq!(
        signals
            .iter()
            .filter(|signal| matches!(signal, Signal::Event(event) if matches!(event.data, Event::SessionExecutionStarted(_))))
            .count(),
        1
    );
}

#[test]
fn fixture_contains_the_documented_vocabulary() {
    let types: HashSet<_> = FIXTURE
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|json| Envelope::parse(json).unwrap().event_type)
        .collect();
    for expected in [
        "server.connected",
        "session.execution.started",
        "session.execution.succeeded",
        "session.text.started",
        "session.text.delta",
        "session.text.ended",
        "session.step.started",
        "session.step.streamed",
        "session.step.ended",
        "session.inbox.enqueued",
        "session.inbox.delivered",
        "session.usage.updated",
        "session.renamed",
        "session.instructions.updated",
        "agent.updated",
        "command.updated",
        "model.updated",
        "plugin.updated",
        "provider.updated",
        "skill.updated",
        "reference.updated",
        "integration.updated",
        "project.updated",
        "websearch.updated",
    ] {
        assert!(types.contains(expected), "fixture is missing {expected}");
    }
}
