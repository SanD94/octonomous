//! Session lifecycle operations built on the generated V2 client.

use std::collections::HashSet;

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::{envelope, generated, transport::Client};

pub use generated::types::SessionInboxDelivery as Delivery;

/// Optional agent and model settings applied when a session is created.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionOptions {
    pub agent: Option<String>,
    pub model: Option<ModelSelection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSelection {
    pub id: String,
    pub provider_id: String,
    pub variant: Option<String>,
}

impl Client {
    /// Create a session and verify that OpenCode honored the requested working
    /// directory. The directory lives in the JSON body, not a header or query.
    pub async fn create_session(
        &self,
        directory: impl Into<String>,
    ) -> Result<generated::types::SessionInfo, envelope::Error> {
        self.create_session_with(directory, SessionOptions::default())
            .await
    }

    /// Create a session with an explicit agent and model selection.
    pub async fn create_session_with(
        &self,
        directory: impl Into<String>,
        options: SessionOptions,
    ) -> Result<generated::types::SessionInfo, envelope::Error> {
        let directory = directory.into();
        let body = generated::types::SessionCreateBody {
            agent: options.agent,
            location: Some(generated::types::LocationPublicRef {
                directory: directory.clone(),
            }),
            model: options.model.map(|model| generated::types::ModelRef {
                id: model.id,
                provider_id: model.provider_id,
                variant: model.variant,
            }),
            ..Default::default()
        };
        let session = self
            .generated()
            .session_create(&body)
            .await
            .map_err(envelope::Error::from_generated)?
            .into_inner()
            .data;
        if session.location.directory != directory {
            return Err(envelope::Error::WrongDirectory {
                requested: directory,
                actual: session.location.directory,
            });
        }
        Ok(session)
    }

    /// Return the most recently updated session in `directory`, if one exists.
    pub async fn latest_session(
        &self,
        directory: &str,
    ) -> Result<Option<generated::types::SessionInfo>, envelope::Error> {
        let sessions = self.list_sessions(100).await?;
        Ok(sessions
            .into_iter()
            .filter(|session| session.location.directory == directory)
            .max_by(|left, right| left.time.updated.total_cmp(&right.time.updated)))
    }

    /// Read every page of sessions in server order.
    pub async fn list_sessions(
        &self,
        page_size: usize,
    ) -> Result<Vec<generated::types::SessionInfo>, envelope::Error> {
        let limit = page_size.max(1).to_string();
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut sessions = Vec::new();
        loop {
            let order = cursor
                .is_none()
                .then_some(generated::types::SessionListOrder::Asc);
            let page = self
                .generated()
                .session_list(
                    cursor.as_deref(),
                    None,
                    Some(&limit),
                    order,
                    None,
                    None,
                    None,
                    None,
                )
                .await
                .map_err(envelope::Error::from_generated)?
                .into_inner();
            sessions.extend(page.data);
            let Some(next) = page.cursor.next else {
                break;
            };
            if !seen.insert(next.clone()) {
                return Err(envelope::Error::InvalidResponse(format!(
                    "session pagination repeated cursor {next:?}"
                )));
            }
            cursor = Some(next);
        }
        Ok(sessions)
    }

    /// Read the complete authoritative message timeline in ascending order.
    pub async fn session_messages(
        &self,
        session_id: &str,
        page_size: usize,
    ) -> Result<Vec<JsonValue>, envelope::Error> {
        let _: generated::types::SessionMessageListSessionId =
            session_id.parse().map_err(|error| {
                envelope::Error::InvalidResponse(format!(
                    "invalid session ID {session_id:?}: {error}"
                ))
            })?;
        let limit = page_size.max(1).to_string();
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut messages = Vec::new();
        loop {
            // Progenitor represents Session.Message.Info's undiscriminated
            // anyOf as optional flattened structs. Serde currently accepts a
            // message but leaves every subtype empty, losing its contents.
            // Preserve the authoritative payload as JSON until the spec gains
            // a discriminator or the generator can represent this union.
            let response = self
                .session_messages_request(session_id, cursor.as_deref(), &limit)
                .send()
                .await
                .map_err(envelope::Error::Transport)?;
            let status = response.status();
            let value: JsonValue = response.json().await.map_err(envelope::Error::Transport)?;
            if status != StatusCode::OK {
                return Err(if value.get("_tag").is_some() {
                    envelope::decode_error(value)
                } else {
                    envelope::Error::UnexpectedStatus(status)
                });
            }
            let page: MessagePage = serde_json::from_value(value)
                .map_err(|error| envelope::Error::InvalidResponse(error.to_string()))?;
            messages.extend(page.data);
            let Some(next) = page.cursor.next else {
                break;
            };
            if !seen.insert(next.clone()) {
                return Err(envelope::Error::InvalidResponse(format!(
                    "message pagination repeated cursor {next:?}"
                )));
            }
            cursor = Some(next);
        }
        Ok(messages)
    }

    /// Enqueue a prompt. The returned value is the inbox item; assistant output
    /// is delivered only through the event stream.
    pub async fn prompt(
        &self,
        session_id: &str,
        text: impl Into<String>,
        delivery: Delivery,
    ) -> Result<generated::types::SessionInboxUser, envelope::Error> {
        let session_id = session_id.parse().map_err(|error| {
            envelope::Error::InvalidResponse(format!("invalid session ID {session_id:?}: {error}"))
        })?;
        let body = generated::types::SessionPromptBody {
            agents: Vec::new(),
            delivery: Some(delivery),
            files: Vec::new(),
            id: None,
            metadata: Default::default(),
            resume: None,
            skills: Vec::new(),
            text: text.into(),
        };
        self.generated()
            .session_prompt(&session_id, &body)
            .await
            .map(progenitor_client::ResponseValue::into_inner)
            .map(|response| response.data)
            .map_err(envelope::Error::from_generated)
    }
}

#[derive(Deserialize)]
struct MessagePage {
    data: Vec<JsonValue>,
    cursor: Cursor,
}

#[derive(Deserialize)]
struct Cursor {
    next: Option<String>,
}
