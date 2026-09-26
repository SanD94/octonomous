use std::{collections::HashMap, env, error::Error, io::Write};

use octonomous_core::{
    events::{EventStream, Signal},
    interaction::PermissionReply,
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
    let session = client.create_session(directory.clone()).await?;
    let session_id = session.id.to_string();
    eprintln!(
        "session={session_id} directory={}",
        session.location.directory
    );

    let mut reconciler = Reconciler::new(client.clone(), &session_id);
    print_reconciliation(reconciler.reconcile().await?);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut delivery = Delivery::Steer;
    eprintln!("delivery=steer; type /help for interactive commands");

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
                    "/help" => print_help(),
                    "/permissions" => print_permissions(&reconciler),
                    "/forms" => print_forms(&reconciler),
                    "/interrupt" => match client.interrupt(&session_id).await {
                        Ok(interrupted) => eprintln!("interrupt interrupted={interrupted}"),
                        Err(error) => eprintln!("interrupt failed: {error}"),
                    },
                    command if command.starts_with("/once ")
                        || command.starts_with("/always ")
                        || command.starts_with("/reject ") => {
                        let (decision, number) = if let Some(value) = command.strip_prefix("/once ") {
                            (PermissionReply::Once, value)
                        } else if let Some(value) = command.strip_prefix("/always ") {
                            (PermissionReply::Always, value)
                        } else {
                            (PermissionReply::Reject, command.trim_start_matches("/reject "))
                        };
                        match numbered_id(number, reconciler.state().pending_permissions.iter().map(|item| item.id.as_str())) {
                            Ok(request_id) => match client.reply_permission(&session_id, &request_id, decision).await {
                                Ok(()) => {
                                    eprintln!("permission {request_id} settled decision={decision}");
                                    print_reconciliation(reconciler.reconcile_interactions().await?);
                                    print_permissions(&reconciler);
                                }
                                Err(error) => eprintln!("permission reply failed: {error}"),
                            },
                            Err(error) => eprintln!("permission reply failed: {error}"),
                        }
                    }
                    command if command.starts_with("/answer ") => {
                        match parse_form_answer(command, &reconciler) {
                            Ok((form_id, answer)) => match client.reply_form(&session_id, &form_id, answer).await {
                                Ok(()) => {
                                    eprintln!("form {form_id} answered");
                                    print_reconciliation(reconciler.reconcile_interactions().await?);
                                    print_forms(&reconciler);
                                }
                                Err(error) => eprintln!("form reply failed: {error}"),
                            },
                            Err(error) => eprintln!("form reply failed: {error}"),
                        }
                    }
                    command if command.starts_with("/cancel-form ") => {
                        let number = command.trim_start_matches("/cancel-form ");
                        match numbered_id(number, reconciler.state().pending_forms.iter().map(|item| item.id.as_str())) {
                            Ok(form_id) => match client.cancel_form(&session_id, &form_id).await {
                                Ok(()) => {
                                    eprintln!("form {form_id} cancelled");
                                    print_reconciliation(reconciler.reconcile_interactions().await?);
                                    print_forms(&reconciler);
                                }
                                Err(error) => eprintln!("form cancellation failed: {error}"),
                            },
                            Err(error) => eprintln!("form cancellation failed: {error}"),
                        }
                    }
                    command if command.starts_with("/find ") => {
                        match client.find_files(&directory, command.trim_start_matches("/find "), 20).await {
                            Ok(entries) => for entry in entries {
                                eprintln!("{}\t{}", entry.type_, entry.path);
                            },
                            Err(error) => eprintln!("file search failed: {error}"),
                        }
                    }
                    prompt => {
                        if let Err(error) = client.prompt(&session_id, prompt, delivery).await {
                            eprintln!("prompt failed: {error}");
                        }
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
                let permissions_before = reconciler.state().pending_permissions.len();
                let forms_before = reconciler.state().pending_forms.len();
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
                if permissions_before != reconciler.state().pending_permissions.len() {
                    print_permissions(&reconciler);
                }
                if forms_before != reconciler.state().pending_forms.len() {
                    print_forms(&reconciler);
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
    eprintln!(
        "pending permissions={} -> {} forms={} -> {}",
        report.permissions_before,
        report.permissions_after,
        report.forms_before,
        report.forms_after
    );
    if !report.assistant_appends.is_empty() {
        eprintln!(
            "repaired {} assistant stream gap(s) from authoritative history",
            report.assistant_appends.len()
        );
    }
}

fn print_help() {
    eprintln!("/permissions | /once N | /always N | /reject N");
    eprintln!(r#"/forms | /answer N {{"field":value}} | /cancel-form N"#);
    eprintln!("/interrupt | /find QUERY | /steer | /queue");
}

fn print_permissions(reconciler: &Reconciler) {
    if reconciler.state().pending_permissions.is_empty() {
        eprintln!("permissions: none pending");
    }
    for (index, item) in reconciler.state().pending_permissions.iter().enumerate() {
        eprintln!(
            "permission {}: id={} action={} resources={:?}{}",
            index + 1,
            item.id,
            item.action,
            item.resources,
            item.message
                .as_deref()
                .map(|message| format!(" message={message:?}"))
                .unwrap_or_default()
        );
    }
}

fn print_forms(reconciler: &Reconciler) {
    if reconciler.state().pending_forms.is_empty() {
        eprintln!("forms: none pending");
    }
    for (index, item) in reconciler.state().pending_forms.iter().enumerate() {
        eprintln!(
            "form {}: id={} title={:?} fields={}",
            index + 1,
            item.id,
            item.title,
            item.fields
        );
    }
}

fn numbered_id<'a>(input: &str, ids: impl Iterator<Item = &'a str>) -> Result<String, String> {
    let number = input
        .trim()
        .parse::<usize>()
        .map_err(|_| format!("expected a positive item number, got {input:?}"))?;
    if number == 0 {
        return Err("item numbers start at 1".into());
    }
    ids.into_iter()
        .nth(number - 1)
        .map(str::to_owned)
        .ok_or_else(|| format!("no pending item numbered {number}"))
}

fn parse_form_answer(
    command: &str,
    reconciler: &Reconciler,
) -> Result<(String, HashMap<String, serde_json::Value>), String> {
    let input = command.trim_start_matches("/answer ");
    let (number, json) = input
        .split_once(char::is_whitespace)
        .ok_or("usage: /answer N {\"field\":value}")?;
    let form_id = numbered_id(
        number,
        reconciler
            .state()
            .pending_forms
            .iter()
            .map(|item| item.id.as_str()),
    )?;
    let answer = serde_json::from_str(json.trim())
        .map_err(|error| format!("answer must be a JSON object: {error}"))?;
    Ok((form_id, answer))
}
