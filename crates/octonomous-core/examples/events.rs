use std::{env, error::Error};

use octonomous_core::{
    events::{Envelope, Event, EventStream, Signal},
    transport::Client,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("live") => live(args.next().as_deref()).await,
        Some(other) => Err(format!("unknown mode {other:?}; use `live [server-url]`").into()),
    }
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
