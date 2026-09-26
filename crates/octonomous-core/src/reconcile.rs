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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPermission {
    pub id: String,
    pub action: String,
    pub resources: Vec<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingForm {
    pub id: String,
    pub title: String,
    /// The generated field union is retained as JSON so a view can render new
    /// field variants without changing reconciliation state.
    pub fields: JsonValue,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionState {
    pub running: bool,
    pub transcript: Vec<TranscriptMessage>,
    pub pending_permissions: Vec<PendingPermission>,
    pub pending_forms: Vec<PendingForm>,
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
    pub permissions_before: usize,
    pub permissions_after: usize,
    pub forms_before: usize,
    pub forms_after: usize,
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
            Event::PermissionAsked(value) => {
                if let Some(permission) = pending_permission(value, session_id) {
                    self.pending_permissions
                        .retain(|item| item.id != permission.id);
                    self.pending_permissions.push(permission);
                }
            }
            Event::PermissionReplied(value) => {
                if event_session(value) == Some(session_id)
                    && let Some(id) = value.get("requestID").and_then(JsonValue::as_str)
                {
                    self.pending_permissions.retain(|item| item.id != id);
                }
            }
            Event::FormCreated(value) => {
                if let Some(form) = pending_form(value, session_id) {
                    self.pending_forms.retain(|item| item.id != form.id);
                    self.pending_forms.push(form);
                }
            }
            Event::FormReplied(value) | Event::FormCancelled(value) => {
                if event_session(value) == Some(session_id)
                    && let Some(id) = value.get("formID").and_then(JsonValue::as_str)
                {
                    self.pending_forms.retain(|item| item.id != id);
                }
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
            permissions_before: self.pending_permissions.len(),
            permissions_after: self.pending_permissions.len(),
            forms_before: self.pending_forms.len(),
            forms_after: self.pending_forms.len(),
        }
    }

    fn overwrite_interactions(
        &mut self,
        permissions: Vec<PendingPermission>,
        forms: Vec<PendingForm>,
        report: &mut Reconciliation,
    ) {
        report.permissions_before = self.pending_permissions.len();
        report.permissions_after = permissions.len();
        report.forms_before = self.pending_forms.len();
        report.forms_after = forms.len();
        report.changed |= self.pending_permissions != permissions || self.pending_forms != forms;
        self.pending_permissions = permissions;
        self.pending_forms = forms;
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
        let mut report = self.state.overwrite(messages);
        self.reconcile_interactions_into(&mut report).await?;
        Ok(report)
    }

    /// Re-fetch interactive state after a local reply without re-reading the
    /// transcript. Reconnects use [`Self::reconcile`] to refresh both.
    pub async fn reconcile_interactions(&mut self) -> Result<Reconciliation, envelope::Error> {
        let count = self.state.transcript.len();
        let mut report = Reconciliation {
            before: count,
            after: count,
            changed: false,
            assistant_appends: Vec::new(),
            permissions_before: 0,
            permissions_after: 0,
            forms_before: 0,
            forms_after: 0,
        };
        self.reconcile_interactions_into(&mut report).await?;
        Ok(report)
    }

    async fn reconcile_interactions_into(
        &mut self,
        report: &mut Reconciliation,
    ) -> Result<(), envelope::Error> {
        let permissions = self
            .client
            .pending_permissions(&self.session_id)
            .await?
            .into_iter()
            .map(|item| PendingPermission {
                id: item.id.to_string(),
                action: item.action,
                resources: item.resources,
                message: item.message,
            })
            .collect();
        let forms = self
            .client
            .pending_forms(&self.session_id)
            .await?
            .into_iter()
            .map(|item| PendingForm {
                id: item.id.to_string(),
                title: item.title,
                fields: serde_json::to_value(item.fields)
                    .expect("generated form fields must serialize"),
            })
            .collect();
        self.state
            .overwrite_interactions(permissions, forms, report);
        Ok(())
    }

    /// Reduce ordinary events and re-poll truth whenever the stream announces
    /// `server.connected`, including after an automatic reconnect, and at the
    /// end of every turn.
    ///
    /// The end-of-turn poll is what keeps a conversation in order. User
    /// messages never arrive on the event stream, so a prompt a view echoes
    /// optimistically can only be replaced by authoritative state once the
    /// server has finished writing it. `session.execution.succeeded` is the
    /// last event of a turn, so polling there settles the transcript in server
    /// order before the view renders the next prompt.
    pub async fn handle(&mut self, signal: &Signal) -> Result<Update, envelope::Error> {
        match signal {
            Signal::Event(envelope) if matches!(envelope.data, Event::ServerConnected) => {
                Ok(Update {
                    reconciliation: Some(self.reconcile().await?),
                    ..Default::default()
                })
            }
            Signal::Event(envelope) if ends_turn(envelope, &self.session_id) => {
                // Reduce first so the turn is marked finished and any last
                // streamed text is folded in before authoritative state wins.
                let reduction = self.state.reduce(&self.session_id, envelope);
                Ok(Update {
                    reduction,
                    reconciliation: Some(self.reconcile().await?),
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

/// Whether `envelope` is the final event of a turn in `session_id`.
pub fn ends_turn(envelope: &Envelope, session_id: &str) -> bool {
    matches!(
        &envelope.data,
        Event::SessionExecutionSucceeded(value) if value.session_id == session_id
    )
}

fn event_session(value: &JsonValue) -> Option<&str> {
    value.get("sessionID").and_then(JsonValue::as_str)
}

fn pending_permission(value: &JsonValue, session_id: &str) -> Option<PendingPermission> {
    if event_session(value)? != session_id {
        return None;
    }
    Some(PendingPermission {
        id: value.get("id")?.as_str()?.to_owned(),
        action: value.get("action")?.as_str()?.to_owned(),
        resources: value
            .get("resources")?
            .as_array()?
            .iter()
            .filter_map(JsonValue::as_str)
            .map(str::to_owned)
            .collect(),
        message: value
            .get("message")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
    })
}

fn pending_form(value: &JsonValue, session_id: &str) -> Option<PendingForm> {
    if event_session(value)? != session_id {
        return None;
    }
    Some(PendingForm {
        id: value.get("id")?.as_str()?.to_owned(),
        title: value.get("title")?.as_str()?.to_owned(),
        fields: value.get("fields")?.clone(),
    })
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
    fn reducer_tracks_and_settles_interactive_events() {
        let mut state = SessionState::default();
        state.reduce(
            "ses_target",
            &event(
                "permission.asked",
                serde_json::json!({
                    "id":"per_1", "sessionID":"ses_target", "action":"bash",
                    "resources":["cargo test"], "message":"Run tests?"
                }),
            ),
        );
        state.reduce(
            "ses_target",
            &event(
                "form.created",
                serde_json::json!({
                    "id":"frm_1", "sessionID":"ses_target", "title":"Choose",
                    "fields":[{"key":"target", "type":"string"}]
                }),
            ),
        );

        assert_eq!(state.pending_permissions[0].id, "per_1");
        assert_eq!(state.pending_forms[0].id, "frm_1");

        state.reduce(
            "ses_target",
            &event(
                "permission.replied",
                serde_json::json!({
                    "sessionID":"ses_target", "requestID":"per_1", "reply":"once"
                }),
            ),
        );
        state.reduce(
            "ses_target",
            &event(
                "form.cancelled",
                serde_json::json!({
                    "sessionID":"ses_target", "formID":"frm_1"
                }),
            ),
        );

        assert!(state.pending_permissions.is_empty());
        assert!(state.pending_forms.is_empty());
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
                permissions_before: 0,
                permissions_after: 0,
                forms_before: 0,
                forms_after: 0,
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
