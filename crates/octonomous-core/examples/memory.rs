//! Reproducible client RSS probe for the three core acceptance states.

use std::{error::Error, hint::black_box, process::Command};

use octonomous_core::{
    auth::Credentials, events::Envelope, reconcile::SessionState, transport::Client,
};

const LONG_RESPONSE_BYTES: usize = 1024 * 1024;
const TRANSCRIPT_MESSAGES: usize = 100;
const TRANSCRIPT_MESSAGE_BYTES: usize = 100 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    // Keep the actual authenticated HTTP transports resident in every state.
    let client = Client::new(
        "http://127.0.0.1:1",
        &Credentials::from_password("memory-probe")?,
    )?;
    let mut state = SessionState::default();
    sample("idle", &client, &state)?;

    reduce_text(&mut state, "session.text.started", "msg_stream", "")?;
    reduce_text(
        &mut state,
        "session.text.delta",
        "msg_stream",
        &"s".repeat(LONG_RESPONSE_BYTES),
    )?;
    sample("mid-stream", &client, &state)?;

    reduce_text(
        &mut state,
        "session.text.ended",
        "msg_stream",
        &"s".repeat(LONG_RESPONSE_BYTES),
    )?;
    for index in 0..TRANSCRIPT_MESSAGES {
        reduce_text(
            &mut state,
            "session.text.ended",
            &format!("msg_{index}"),
            &"t".repeat(TRANSCRIPT_MESSAGE_BYTES),
        )?;
    }
    sample("full-transcript", &client, &state)?;
    Ok(())
}

fn reduce_text(
    state: &mut SessionState,
    event_type: &str,
    message_id: &str,
    text: &str,
) -> Result<(), Box<dyn Error>> {
    let field = if event_type.ends_with("delta") {
        "delta"
    } else {
        "text"
    };
    let mut data = serde_json::json!({
        "sessionID": "ses_memory",
        "assistantMessageID": message_id,
        "ordinal": 0,
    });
    if event_type.ends_with("started") {
        data.as_object_mut().unwrap().remove(field);
    } else {
        data[field] = text.into();
    }
    let envelope = Envelope::parse(
        &serde_json::json!({"id":"evt_memory", "type":event_type, "data":data}).to_string(),
    )?;
    state.reduce("ses_memory", &envelope);
    Ok(())
}

fn sample(label: &str, client: &Client, state: &SessionState) -> Result<(), Box<dyn Error>> {
    black_box(client);
    black_box(state);
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()?;
    if !output.status.success() {
        return Err("ps failed while measuring resident memory".into());
    }
    let rss_kib: u64 = String::from_utf8(output.stdout)?.trim().parse()?;
    println!("{label}\t{rss_kib}\t{:.1}", rss_kib as f64 / 1024.0);
    Ok(())
}
