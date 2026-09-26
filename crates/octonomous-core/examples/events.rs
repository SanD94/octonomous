use std::{env, error::Error, fs};

use octonomous_core::{
    events::{Envelope, Event, EventStream, SequenceIssue, SequenceTracker, Signal},
    transport::Client,
};

const DEFAULT_FIXTURE: &str = "docs/fixtures/prompt-basic-v2.0.18.sse";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("fixture") => replay(args.next().as_deref().unwrap_or(DEFAULT_FIXTURE)),
        Some("live") => live(args.next().as_deref()).await,
        Some(other) => Err(format!(
            "unknown mode {other:?}; use `fixture [path]` or `live [server-url]`"
        )
        .into()),
    }
}

fn replay(path: &str) -> Result<(), Box<dyn Error>> {
    let fixture = fs::read_to_string(path)?;
    let mut sequences = SequenceTracker::default();
    for line in fixture.lines() {
        let Some(json) = line.strip_prefix("data: ") else {
            continue;
        };
        let envelope = Envelope::parse(json)?;
        print_sequence_issue(&envelope, sequences.observe(&envelope));
        print_event(&envelope);
    }
    Ok(())
}

async fn live(server: Option<&str>) -> Result<(), Box<dyn Error>> {
    let client = Client::discover(server)?;
    let stream = EventStream::connect(&client, 1_024)?;
    let mut events = stream.subscribe();
    loop {
        match events.recv().await? {
            Signal::Event(envelope) => print_event(&envelope),
            Signal::Gap {
                aggregate_id,
                expected,
                observed,
            } => println!("gap aggregate={aggregate_id} expected={expected} observed={observed}"),
            Signal::Duplicate { aggregate_id, seq } => {
                println!("duplicate aggregate={aggregate_id} seq={seq}")
            }
            Signal::Reconnected => println!("reconnected"),
            Signal::ConnectionError(error) => println!("connection-error {error}"),
        }
    }
}

fn print_sequence_issue(envelope: &Envelope, issue: Option<SequenceIssue>) {
    match issue {
        Some(SequenceIssue::Gap { expected, observed }) => println!(
            "gap aggregate={} expected={expected} observed={observed}",
            envelope.durable.as_ref().unwrap().aggregate_id
        ),
        Some(SequenceIssue::Duplicate { seq }) => println!(
            "duplicate aggregate={} seq={seq}",
            envelope.durable.as_ref().unwrap().aggregate_id
        ),
        None => {}
    }
}

fn print_event(envelope: &Envelope) {
    let durable = envelope
        .durable
        .as_ref()
        .map(|value| format!(" aggregate={} seq={}", value.aggregate_id, value.seq))
        .unwrap_or_default();
    match &envelope.data {
        Event::Unknown(payload) => println!(
            "event type={}{} unknown={payload}",
            envelope.event_type, durable
        ),
        event => println!(
            "event type={}{} data={event:?}",
            envelope.event_type, durable
        ),
    }
}
