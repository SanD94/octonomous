//! Interactive session operations: permissions, forms, interruption, and file search.

use std::collections::HashMap;

use serde_json::Value as JsonValue;

use crate::{envelope, generated, transport::Client};

pub use generated::types::{FileSystemEntry, FsFindType, PermissionReply};

impl Client {
    /// Fetch the server's authoritative set of unresolved permission requests.
    pub async fn pending_permissions(
        &self,
        session_id: &str,
    ) -> Result<Vec<generated::types::PermissionRequest>, envelope::Error> {
        let session_id = parse_id(session_id, "session ID")?;
        self.generated()
            .session_permission_list(&session_id)
            .await
            .map(progenitor_client::ResponseValue::into_inner)
            .map(|response| response.data)
            .map_err(envelope::Error::from_generated)
    }

    /// Resolve one permission request. A second reply is reported as an
    /// unavailable/already-settled permission rather than treated as success.
    pub async fn reply_permission(
        &self,
        session_id: &str,
        request_id: &str,
        decision: PermissionReply,
    ) -> Result<(), envelope::Error> {
        let session_id = parse_id(session_id, "session ID")?;
        let request_id = parse_id(request_id, "permission request ID")?;
        let body = generated::types::SessionPermissionReplyBody {
            decision,
            message: None,
        };
        self.generated()
            .session_permission_reply(&session_id, &request_id, &body)
            .await
            .map(|_| ())
            .map_err(envelope::Error::from_generated)
    }

    /// Fetch the server's authoritative set of pending forms for a session.
    pub async fn pending_forms(
        &self,
        session_id: &str,
    ) -> Result<Vec<generated::types::FormInfo>, envelope::Error> {
        self.generated()
            .session_form_list(session_id)
            .await
            .map(progenitor_client::ResponseValue::into_inner)
            .map(|response| response.data)
            .map_err(envelope::Error::from_generated)
    }

    /// Submit an answer object whose keys correspond to the form's field keys.
    pub async fn reply_form(
        &self,
        session_id: &str,
        form_id: &str,
        answer: HashMap<String, JsonValue>,
    ) -> Result<(), envelope::Error> {
        let form_id = parse_id(form_id, "form ID")?;
        let body = generated::types::FormReply {
            answer: generated::types::FormAnswer(
                answer
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
            ),
        };
        self.generated()
            .session_form_reply(session_id, &form_id, &body)
            .await
            .map(|_| ())
            .map_err(envelope::Error::from_generated)
    }

    pub async fn cancel_form(
        &self,
        session_id: &str,
        form_id: &str,
    ) -> Result<(), envelope::Error> {
        let form_id = parse_id(form_id, "form ID")?;
        self.generated()
            .session_form_cancel(session_id, &form_id)
            .await
            .map(|_| ())
            .map_err(envelope::Error::from_generated)
    }

    /// Interrupt active execution. `false` means the session was already idle.
    pub async fn interrupt(&self, session_id: &str) -> Result<bool, envelope::Error> {
        let session_id = parse_id(session_id, "session ID")?;
        self.generated()
            .session_interrupt(&session_id, None)
            .await
            .map(progenitor_client::ResponseValue::into_inner)
            .map(|response| response.interrupted)
            .map_err(envelope::Error::from_generated)
    }

    /// Find file and directory candidates for `@` completion.
    pub async fn find_files(
        &self,
        directory: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<FileSystemEntry>, envelope::Error> {
        let location = generated::types::FsFindLocation {
            directory: Some(directory.to_owned()),
        };
        self.generated()
            .fs_find(
                Some(&limit.max(1).to_string()),
                Some(&location),
                query,
                None,
            )
            .await
            .map(progenitor_client::ResponseValue::into_inner)
            .map(|response| response.data)
            .map_err(envelope::Error::from_generated)
    }
}

fn parse_id<T>(value: &str, kind: &str) -> Result<T, envelope::Error>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value.parse().map_err(|error| {
        envelope::Error::InvalidResponse(format!("invalid {kind} {value:?}: {error}"))
    })
}
