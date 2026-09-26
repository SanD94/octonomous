use std::time::Duration;

use futures_util::StreamExt;
use octonomous_core::{
    auth::Credentials,
    events::{Envelope, Event, EventStream, Signal},
    transport::Client,
};
use reqwest_eventsource::{Event as SseEvent, EventSource, retry::Never};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const EVENT_STREAM: &str = r#"data: {"id":"evt_connected","type":"server.connected","data":{}}

: heartbeat

data: {"id":"evt_1","created":1,"type":"session.execution.started","data":{"sessionID":"ses_1"},"durable":{"aggregateID":"ses_1","seq":1,"version":1}}

"#;

async fn event_server(body: &'static str) -> String {
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
    let url = event_server(EVENT_STREAM).await;
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

    assert_eq!(messages, 2, "the heartbeat comment must not surface");
}

#[tokio::test]
async fn broadcast_delivers_events_to_multiple_consumers() {
    let first = r#"data: {"id":"evt_1","created":1,"type":"session.execution.started","data":{"sessionID":"ses_1"},"durable":{"aggregateID":"ses_1","seq":1,"version":1}}

"#;
    let url = event_server(first).await;
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
