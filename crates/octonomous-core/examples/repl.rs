use std::{env, error::Error, io::Write};

use octonomous_core::{
    events::{EventStream, Signal},
    reconcile::Reconciler,
    session::Delivery,
    transport::Client,
};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let directory = args.next().ok_or("usage: repl DIRECTORY [SERVER_URL]")?;
    let server = args.next();
    if args.next().is_some() {
        return Err("usage: repl DIRECTORY [SERVER_URL]".into());
    }

    let client = Client::discover(server.as_deref())?;
    let stream = EventStream::connect(&client, 1_024)?;
    let mut events = stream.subscribe();
    let session = client.create_session(directory).await?;
    let session_id = session.id.to_string();
    eprintln!(
        "session={session_id} directory={}",
        session.location.directory
    );

    let mut reconciler = Reconciler::new(client.clone(), &session_id);
    print_reconciliation(reconciler.reconcile().await?);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut delivery = Delivery::Steer;
    eprintln!("delivery=steer; use /steer or /queue to change it");

    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                match line.trim() {
                    "" => continue,
                    "/steer" => {
                        delivery = Delivery::Steer;
                        eprintln!("delivery=steer");
                    }
                    "/queue" => {
                        delivery = Delivery::Queue;
                        eprintln!("delivery=queue");
                    }
                    prompt => {
                        client.prompt(&session_id, prompt, delivery).await?;
                    }
                }
            }
            signal = events.recv() => {
                let signal = signal?;
                match &signal {
                    Signal::Gap { aggregate_id, expected, observed } =>
                        eprintln!("gap aggregate={aggregate_id} expected={expected} observed={observed}"),
                    Signal::Duplicate { aggregate_id, seq } =>
                        eprintln!("duplicate aggregate={aggregate_id} seq={seq}"),
                    Signal::Reconnected => eprintln!("event stream reconnected; awaiting server.connected"),
                    Signal::ConnectionError(error) => eprintln!("event stream error: {error}"),
                    Signal::Event(_) => {}
                }
                let update = reconciler.handle(&signal).await?;
                if let Some(delta) = update.reduction.assistant_delta {
                    print!("{delta}");
                    std::io::stdout().flush()?;
                }
                if let Some(report) = update.reconciliation {
                    for missing in &report.assistant_appends {
                        print!("{missing}");
                    }
                    std::io::stdout().flush()?;
                    print_reconciliation(report);
                }
            }
        }
    }
    Ok(())
}

fn print_reconciliation(report: octonomous_core::reconcile::Reconciliation) {
    eprintln!(
        "reconciled changed={} messages={} -> {}",
        report.changed, report.before, report.after
    );
    if !report.assistant_appends.is_empty() {
        eprintln!(
            "repaired {} assistant stream gap(s) from authoritative history",
            report.assistant_appends.len()
        );
    }
}
