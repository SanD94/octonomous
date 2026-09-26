//! Typed OpenCode event ingestion and resilient SSE subscription.

use std::{collections::HashMap, time::Duration};

use futures_util::StreamExt;
use reqwest_eventsource::{Event as SseEvent, EventSource, retry::RetryPolicy};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use thiserror::Error;
use tokio::{sync::broadcast, task::JoinHandle};

use crate::transport::Client;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Location {
    pub directory: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Durable {
    #[serde(rename = "aggregateID")]
    pub aggregate_id: String,
    pub seq: u64,
    pub version: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionExecution {
    #[serde(rename = "sessionID")]
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionTextStarted {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: String,
    pub ordinal: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionTextDelta {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: String,
    pub ordinal: u64,
    pub delta: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionTextEnded {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: String,
    pub ordinal: u64,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionStepStarted {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub agent: String,
    pub model: JsonValue,
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: String,
    pub started: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionStepStreamed {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "assistantMessageID")]
    pub assistant_message_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionInboxEnqueued {
    #[serde(rename = "inboxID")]
    pub inbox_id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub item: JsonValue,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionInboxDelivered {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "inboxID")]
    pub inbox_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionRenamed {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub title: String,
}

/// Known event payloads. Payloads that are still evolving remain JSON values;
/// the event name is nevertheless typed so consumers can match it safely.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    ServerConnected,
    SessionExecutionStarted(SessionExecution),
    SessionExecutionSucceeded(SessionExecution),
    SessionTextStarted(SessionTextStarted),
    SessionTextDelta(SessionTextDelta),
    SessionTextEnded(SessionTextEnded),
    SessionStepStarted(SessionStepStarted),
    SessionStepStreamed(SessionStepStreamed),
    SessionStepEnded(JsonValue),
    SessionInboxEnqueued(SessionInboxEnqueued),
    SessionInboxDelivered(SessionInboxDelivered),
    SessionUsageUpdated(JsonValue),
    SessionRenamed(SessionRenamed),
    SessionInstructionsUpdated(JsonValue),
    SessionReasoningStarted(JsonValue),
    SessionReasoningDelta(JsonValue),
    SessionReasoningEnded(JsonValue),
    SessionToolInputStarted(JsonValue),
    SessionToolInputEnded(JsonValue),
    SessionToolCalled(JsonValue),
    SessionToolProgress(JsonValue),
    SessionToolSuccess(JsonValue),
    PermissionAsked(JsonValue),
    PermissionReplied(JsonValue),
    FormCreated(JsonValue),
    FormReplied(JsonValue),
    FormCancelled(JsonValue),
    SessionDeleted(JsonValue),
    ShellCreated(JsonValue),
    ShellExited(JsonValue),
    AgentUpdated(JsonValue),
    CommandUpdated(JsonValue),
    ModelUpdated(JsonValue),
    PluginUpdated(JsonValue),
    ProviderUpdated(JsonValue),
    SkillUpdated(JsonValue),
    ReferenceUpdated(JsonValue),
    IntegrationUpdated(JsonValue),
    ProjectUpdated(JsonValue),
    WebsearchUpdated(JsonValue),
    Unknown(JsonValue),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    pub id: String,
    pub created: Option<u64>,
    pub event_type: String,
    pub location: Option<Location>,
    pub data: Event,
    pub durable: Option<Durable>,
}

#[derive(Deserialize)]
struct RawEnvelope {
    id: String,
    created: Option<u64>,
    #[serde(rename = "type")]
    event_type: String,
    location: Option<Location>,
    data: JsonValue,
    durable: Option<Durable>,
}

impl Envelope {
    pub fn parse(json: &str) -> Result<Self, ParseError> {
        let raw: RawEnvelope = serde_json::from_str(json)?;
        let data = Event::parse(&raw.event_type, raw.data)?;
        Ok(Self {
            id: raw.id,
            created: raw.created,
            event_type: raw.event_type,
            location: raw.location,
            data,
            durable: raw.durable,
        })
    }
}

impl Event {
    fn parse(event_type: &str, value: JsonValue) -> Result<Self, ParseError> {
        fn typed<T: for<'de> Deserialize<'de>>(
            event_type: &str,
            value: JsonValue,
        ) -> Result<T, ParseError> {
            serde_json::from_value(value).map_err(|source| ParseError::Payload {
                event_type: event_type.to_owned(),
                source,
            })
        }

        Ok(match event_type {
            "server.connected" => Self::ServerConnected,
            "session.execution.started" => Self::SessionExecutionStarted(typed(event_type, value)?),
            "session.execution.succeeded" => {
                Self::SessionExecutionSucceeded(typed(event_type, value)?)
            }
            "session.text.started" => Self::SessionTextStarted(typed(event_type, value)?),
            "session.text.delta" => Self::SessionTextDelta(typed(event_type, value)?),
            "session.text.ended" => Self::SessionTextEnded(typed(event_type, value)?),
            "session.step.started" => Self::SessionStepStarted(typed(event_type, value)?),
            "session.step.streamed" => Self::SessionStepStreamed(typed(event_type, value)?),
            "session.step.ended" => Self::SessionStepEnded(value),
            "session.inbox.enqueued" => Self::SessionInboxEnqueued(typed(event_type, value)?),
            "session.inbox.delivered" => Self::SessionInboxDelivered(typed(event_type, value)?),
            "session.usage.updated" => Self::SessionUsageUpdated(value),
            "session.renamed" => Self::SessionRenamed(typed(event_type, value)?),
            "session.instructions.updated" => Self::SessionInstructionsUpdated(value),
            "session.reasoning.started" => Self::SessionReasoningStarted(value),
            "session.reasoning.delta" => Self::SessionReasoningDelta(value),
            "session.reasoning.ended" => Self::SessionReasoningEnded(value),
            "session.tool.input.started" => Self::SessionToolInputStarted(value),
            "session.tool.input.ended" => Self::SessionToolInputEnded(value),
            "session.tool.called" => Self::SessionToolCalled(value),
            "session.tool.progress" => Self::SessionToolProgress(value),
            "session.tool.success" => Self::SessionToolSuccess(value),
            "permission.asked" => Self::PermissionAsked(value),
            "permission.replied" => Self::PermissionReplied(value),
            "form.created" => Self::FormCreated(value),
            "form.replied" => Self::FormReplied(value),
            "form.cancelled" => Self::FormCancelled(value),
            "session.deleted" => Self::SessionDeleted(value),
            "shell.created" => Self::ShellCreated(value),
            "shell.exited" => Self::ShellExited(value),
            "agent.updated" => Self::AgentUpdated(value),
            "command.updated" => Self::CommandUpdated(value),
            "model.updated" => Self::ModelUpdated(value),
            "plugin.updated" => Self::PluginUpdated(value),
            "provider.updated" => Self::ProviderUpdated(value),
            "skill.updated" => Self::SkillUpdated(value),
            "reference.updated" => Self::ReferenceUpdated(value),
            "integration.updated" => Self::IntegrationUpdated(value),
            "project.updated" => Self::ProjectUpdated(value),
            "websearch.updated" => Self::WebsearchUpdated(value),
            _ => Self::Unknown(value),
        })
    }
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("invalid event envelope: {0}")]
    Envelope(#[from] serde_json::Error),
    #[error("invalid payload for {event_type}: {source}")]
    Payload {
        event_type: String,
        source: serde_json::Error,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Signal {
    Event(Envelope),
    Gap {
        aggregate_id: String,
        expected: u64,
        observed: u64,
    },
    Duplicate {
        aggregate_id: String,
        seq: u64,
    },
    Reconnected,
    ConnectionError(String),
}

pub struct EventStream {
    sender: broadcast::Sender<Signal>,
    task: JoinHandle<()>,
}

impl EventStream {
    pub fn connect(client: &Client, capacity: usize) -> Result<Self, ConnectError> {
        Self::connect_with_backoff(
            client,
            capacity,
            Backoff::new(Duration::from_millis(300), Duration::from_secs(30)),
        )
    }

    fn connect_with_backoff(
        client: &Client,
        capacity: usize,
        backoff: Backoff,
    ) -> Result<Self, ConnectError> {
        if capacity == 0 {
            return Err(ConnectError::ZeroCapacity);
        }
        let mut source = EventSource::new(client.event_request())?;
        source.set_retry_policy(Box::new(backoff));
        let (sender, _) = broadcast::channel(capacity);
        let task_sender = sender.clone();
        let task = tokio::spawn(async move {
            ingest(&mut source, &task_sender).await;
        });
        Ok(Self { sender, task })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.sender.subscribe()
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("event broadcast capacity must be greater than zero")]
    ZeroCapacity,
    #[error("event request cannot be retried: {0}")]
    Request(#[from] reqwest_eventsource::CannotCloneRequestError),
}

async fn ingest(source: &mut EventSource, sender: &broadcast::Sender<Signal>) {
    let mut opened = false;
    let mut sequences = SequenceTracker::default();
    while let Some(item) = source.next().await {
        let signal = match item {
            Ok(SseEvent::Open) if opened => Some(Signal::Reconnected),
            Ok(SseEvent::Open) => {
                opened = true;
                None
            }
            Ok(SseEvent::Message(message)) => match Envelope::parse(&message.data) {
                Ok(envelope) => {
                    match sequences.observe(&envelope) {
                        Some(SequenceIssue::Gap { expected, observed }) => {
                            let aggregate_id = envelope
                                .durable
                                .as_ref()
                                .expect("sequence issue requires durable metadata")
                                .aggregate_id
                                .clone();
                            let _ = sender.send(Signal::Gap {
                                aggregate_id,
                                expected,
                                observed,
                            });
                        }
                        Some(SequenceIssue::Duplicate { seq }) => {
                            let aggregate_id = envelope
                                .durable
                                .as_ref()
                                .expect("sequence issue requires durable metadata")
                                .aggregate_id
                                .clone();
                            let _ = sender.send(Signal::Duplicate { aggregate_id, seq });
                            continue;
                        }
                        None => {}
                    }
                    Some(Signal::Event(envelope))
                }
                Err(error) => Some(Signal::ConnectionError(error.to_string())),
            },
            Err(error) => Some(Signal::ConnectionError(error.to_string())),
        };
        if let Some(signal) = signal {
            let _ = sender.send(signal);
        }
    }
}

#[derive(Default)]
pub struct SequenceTracker {
    last: HashMap<String, u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceIssue {
    Gap { expected: u64, observed: u64 },
    Duplicate { seq: u64 },
}

impl SequenceTracker {
    pub fn observe(&mut self, envelope: &Envelope) -> Option<SequenceIssue> {
        let durable = envelope.durable.as_ref()?;
        let previous = self.last.get(&durable.aggregate_id).copied();
        if previous.is_none_or(|seq| durable.seq > seq) {
            self.last.insert(durable.aggregate_id.clone(), durable.seq);
        }
        let previous = previous?;
        if durable.seq <= previous {
            return Some(SequenceIssue::Duplicate { seq: durable.seq });
        }
        let expected = previous.saturating_add(1);
        (durable.seq > expected).then_some(SequenceIssue::Gap {
            expected,
            observed: durable.seq,
        })
    }
}

#[derive(Clone, Debug)]
struct Backoff {
    start: Duration,
    max: Duration,
}

impl Backoff {
    fn new(start: Duration, max: Duration) -> Self {
        Self { start, max }
    }

    fn jitter(duration: Duration) -> Duration {
        // Full jitter in [50%, 100%] avoids synchronized reconnect storms while
        // retaining an exponential upper bound.
        duration.mul_f64(0.5 + fastrand::f64() * 0.5)
    }
}

impl RetryPolicy for Backoff {
    fn retry(
        &self,
        _error: &reqwest_eventsource::Error,
        last_retry: Option<(usize, Duration)>,
    ) -> Option<Duration> {
        let upper = last_retry
            .map(|(_, previous)| previous.saturating_mul(2))
            .unwrap_or(self.start)
            .min(self.max);
        Some(Self::jitter(upper))
    }

    fn set_reconnection_time(&mut self, duration: Duration) {
        self.start = duration.min(self.max);
    }
}
