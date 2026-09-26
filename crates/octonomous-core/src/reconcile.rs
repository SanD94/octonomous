//! Reducer-backed session state with authoritative REST reconciliation.

use std::collections::BTreeMap;

use serde_json::Value as JsonValue;

use crate::{
    envelope,
    events::{Envelope, Event, Signal},
    transport::Client,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptMessage {
    pub id: String,
    pub role: Role,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionState {
    pub running: bool,
    pub transcript: Vec<TranscriptMessage>,
    streaming: BTreeMap<(String, u64), String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reduction {
    pub assistant_delta: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reconciliation {
    pub before: usize,
    pub after: usize,
    pub changed: bool,
    /// Text absent from the SSE projection but present in authoritative state.
    /// A streaming client can append these fragments to repair reconnect gaps.
    pub assistant_appends: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub reduction: Reduction,
    pub reconciliation: Option<Reconciliation>,
}

impl SessionState {
    /// Apply one low-latency SSE event. Events belonging to another session are
    /// ignored; an authoritative poll can replace all projected transcript data.
    pub fn reduce(&mut self, session_id: &str, envelope: &Envelope) -> Reduction {
        let mut reduction = Reduction::default();
        match &envelope.data {
            Event::SessionExecutionStarted(value) if value.session_id == session_id => {
                self.running = true;
            }
            Event::SessionExecutionSucceeded(value) if value.session_id == session_id => {
                self.running = false;
            }
            Event::SessionTextStarted(value) if value.session_id == session_id => {
                self.streaming
                    .entry((value.assistant_message_id.clone(), value.ordinal))
                    .or_default();
            }
            Event::SessionTextDelta(value) if value.session_id == session_id => {
                self.streaming
                    .entry((value.assistant_message_id.clone(), value.ordinal))
                    .or_default()
                    .push_str(&value.delta);
                reduction.assistant_delta = Some(value.delta.clone());
            }
            Event::SessionTextEnded(value) if value.session_id == session_id => {
                self.streaming.insert(
                    (value.assistant_message_id.clone(), value.ordinal),
                    value.text.clone(),
                );
                self.upsert_streamed(value.assistant_message_id.clone());
            }
            _ => {}
        }
        reduction
    }

    fn upsert_streamed(&mut self, message_id: String) {
        let text = self
            .streaming
            .range((message_id.clone(), 0)..=(message_id.clone(), u64::MAX))
            .map(|(_, text)| text.as_str())
            .collect();
        let message = TranscriptMessage {
            id: message_id.clone(),
            role: Role::Assistant,
            text,
        };
        if let Some(existing) = self
            .transcript
            .iter_mut()
            .find(|item| item.id == message_id)
        {
            *existing = message;
        } else {
            self.transcript.push(message);
        }
    }

    fn overwrite(&mut self, messages: Vec<TranscriptMessage>) -> Reconciliation {
        let before = self.transcript.len();
        let changed = self.transcript != messages;
        let after = messages.len();
        let mut projected: BTreeMap<String, String> = self
            .transcript
            .iter()
            .filter(|message| message.role == Role::Assistant)
            .map(|message| (message.id.clone(), message.text.clone()))
            .collect();
        for ((message_id, _), text) in &self.streaming {
            projected
                .entry(message_id.clone())
                .or_default()
                .push_str(text);
        }
        let assistant_appends = messages
            .iter()
            .filter(|message| message.role == Role::Assistant)
            .filter_map(|message| match projected.get(&message.id) {
                Some(current) if message.text.starts_with(current) => {
                    let missing = &message.text[current.len()..];
                    (!missing.is_empty()).then(|| missing.to_owned())
                }
                None => Some(message.text.clone()),
                Some(_) => None,
            })
            .collect();
        self.transcript = messages;
        self.streaming.clear();
        Reconciliation {
            before,
            after,
            changed,
            assistant_appends,
        }
    }
}

pub struct Reconciler {
    client: Client,
    session_id: String,
    page_size: usize,
    state: SessionState,
}

impl Reconciler {
    pub fn new(client: Client, session_id: impl Into<String>) -> Self {
        Self {
            client,
            session_id: session_id.into(),
            page_size: 100,
            state: SessionState::default(),
        }
    }

    pub fn state(&self) -> &SessionState {
        &self.state
    }

    pub async fn reconcile(&mut self) -> Result<Reconciliation, envelope::Error> {
        let messages = self
            .client
            .session_messages(&self.session_id, self.page_size)
            .await?;
        let messages = messages.into_iter().map(transcript_message).collect();
        Ok(self.state.overwrite(messages))
    }

    /// Reduce ordinary events and re-poll truth whenever the stream announces
    /// `server.connected`, including after an automatic reconnect.
    pub async fn handle(&mut self, signal: &Signal) -> Result<Update, envelope::Error> {
        match signal {
            Signal::Event(envelope) if matches!(envelope.data, Event::ServerConnected) => {
                Ok(Update {
                    reconciliation: Some(self.reconcile().await?),
                    ..Default::default()
                })
            }
            Signal::Event(envelope) => Ok(Update {
                reduction: self.state.reduce(&self.session_id, envelope),
                reconciliation: None,
            }),
            _ => Ok(Update::default()),
        }
    }
}

fn transcript_message(value: JsonValue) -> TranscriptMessage {
    let kind = value
        .get("type")
        .and_then(JsonValue::as_str)
        .unwrap_or("unknown");
    let id = value
        .get("id")
        .and_then(JsonValue::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let (role, text) = match kind {
        "user" => (
            Role::User,
            value
                .get("text")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        "assistant" => {
            let text = value
                .get("content")
                .and_then(JsonValue::as_array)
                .into_iter()
                .flatten()
                .filter(|part| part.get("type").and_then(JsonValue::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(JsonValue::as_str))
                .collect();
            (Role::Assistant, text)
        }
        other => (Role::Other(other.to_owned()), String::new()),
    };
    TranscriptMessage { id, role, text }
}

#[cfg(test)]
mod tests {
    use crate::events::Envelope;

    use super::*;

    fn event(event_type: &str, data: JsonValue) -> Envelope {
        Envelope::parse(
            &serde_json::json!({"id":"evt_1","type":event_type,"data":data}).to_string(),
        )
        .unwrap()
    }

    #[test]
    fn reducer_assembles_deltas_and_ignores_other_sessions() {
        let mut state = SessionState::default();
        state.reduce(
            "ses_target",
            &event("session.text.delta", serde_json::json!({
                "sessionID":"ses_other", "assistantMessageID":"msg_1", "ordinal":0, "delta":"wrong"
            })),
        );
        let reduction = state.reduce(
            "ses_target",
            &event("session.text.delta", serde_json::json!({
                "sessionID":"ses_target", "assistantMessageID":"msg_1", "ordinal":0, "delta":"hel"
            })),
        );
        state.reduce(
            "ses_target",
            &event("session.text.ended", serde_json::json!({
                "sessionID":"ses_target", "assistantMessageID":"msg_1", "ordinal":0, "text":"hello"
            })),
        );

        assert_eq!(reduction.assistant_delta.as_deref(), Some("hel"));
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(state.transcript[0].text, "hello");
    }

    #[test]
    fn authoritative_messages_replace_sse_projection() {
        let mut state = SessionState {
            transcript: vec![TranscriptMessage {
                id: "msg_stale".into(),
                role: Role::Assistant,
                text: "com".into(),
            }],
            ..Default::default()
        };
        let authoritative = vec![TranscriptMessage {
            id: "msg_stale".into(),
            role: Role::Assistant,
            text: "complete".into(),
        }];

        let report = state.overwrite(authoritative.clone());

        assert_eq!(
            report,
            Reconciliation {
                before: 1,
                after: 1,
                changed: true,
                assistant_appends: vec!["plete".into()],
            }
        );
        assert_eq!(state.transcript, authoritative);
    }

    #[test]
    fn authoritative_assistant_text_joins_only_text_parts() {
        let message = transcript_message(serde_json::json!({
            "id": "msg_1",
            "type": "assistant",
            "content": [
                {"type":"text", "text":"one"},
                {"type":"reasoning", "text":"hidden"},
                {"type":"text", "text":" two"}
            ]
        }));
        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.text, "one two");
    }
}
