use progenitor_client::Error as GeneratedError;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct Envelope<T> {
    pub data: T,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("unauthorized: {message}")]
    Unauthorized { message: String },
    #[error("session {session_id} was not found: {message}")]
    SessionNotFound { session_id: String, message: String },
    #[error("request conflicted with {resource:?}: {message}")]
    Conflict {
        resource: Option<String>,
        message: String,
    },
    #[error("session {session_id} is busy: {message}")]
    SessionBusy { session_id: String, message: String },
    #[error("permission request {request_id} is unavailable or already settled: {message}")]
    PermissionUnavailable { request_id: String, message: String },
    #[error("form {form_id} is unavailable: {message}")]
    FormUnavailable { form_id: String, message: String },
    #[error("form {form_id} is already settled: {message}")]
    FormSettled { form_id: String, message: String },
    #[error("OpenCode API error {tag}: {message}")]
    Api { tag: String, message: String },
    #[error("OpenCode returned unexpected HTTP status {0}")]
    UnexpectedStatus(StatusCode),
    #[error("OpenCode request failed: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("OpenCode returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error(
        "OpenCode created the session in {actual:?}, not the requested directory {requested:?}"
    )]
    WrongDirectory { requested: String, actual: String },
}

impl Error {
    pub fn from_generated(error: GeneratedError<Value>) -> Self {
        match error {
            GeneratedError::ErrorResponse(response) => decode_error(response.into_inner()),
            GeneratedError::CommunicationError(error)
            | GeneratedError::InvalidUpgrade(error)
            | GeneratedError::ResponseBodyError(error) => Self::Transport(error),
            GeneratedError::UnexpectedResponse(response) => {
                Self::UnexpectedStatus(response.status())
            }
            GeneratedError::InvalidRequest(message) | GeneratedError::Custom(message) => {
                Self::InvalidResponse(message)
            }
            GeneratedError::InvalidResponsePayload(_, error) => {
                Self::InvalidResponse(error.to_string())
            }
        }
    }
}

pub fn decode_error(value: Value) -> Error {
    let tag = value
        .get("_tag")
        .and_then(Value::as_str)
        .unwrap_or("UnknownError");
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("server returned no error message")
        .to_owned();

    match tag {
        "UnauthorizedError" => Error::Unauthorized { message },
        "SessionNotFoundError" => Error::SessionNotFound {
            session_id: string_field(&value, "sessionID"),
            message,
        },
        "ConflictError" => Error::Conflict {
            resource: value
                .get("resource")
                .and_then(Value::as_str)
                .map(str::to_owned),
            message,
        },
        "SessionBusyError" => Error::SessionBusy {
            session_id: string_field(&value, "sessionID"),
            message,
        },
        "PermissionNotFoundError" => Error::PermissionUnavailable {
            request_id: string_field(&value, "requestID"),
            message,
        },
        "FormNotFoundError" => Error::FormUnavailable {
            form_id: string_field(&value, "id"),
            message,
        },
        "FormAlreadySettledError" => Error::FormSettled {
            form_id: string_field(&value, "id"),
            message,
        },
        _ => Error::Api {
            tag: tag.to_owned(),
            message,
        },
    }
}

fn string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn unwraps_data_envelope() {
        let envelope: Envelope<Vec<u8>> = serde_json::from_value(json!({"data": [1, 2]})).unwrap();
        assert_eq!(envelope.data, vec![1, 2]);
    }

    #[test]
    fn decodes_each_milestone_error_tag() {
        assert!(matches!(
            decode_error(json!({"_tag": "UnauthorizedError", "message": "no"})),
            Error::Unauthorized { message } if message == "no"
        ));
        assert!(matches!(
            decode_error(json!({"_tag": "SessionNotFoundError", "sessionID": "ses_1", "message": "gone"})),
            Error::SessionNotFound { session_id, .. } if session_id == "ses_1"
        ));
        assert!(matches!(
            decode_error(json!({"_tag": "ConflictError", "resource": "prompt", "message": "conflict"})),
            Error::Conflict { resource: Some(resource), .. } if resource == "prompt"
        ));
        assert!(matches!(
            decode_error(json!({"_tag": "SessionBusyError", "sessionID": "ses_2", "message": "busy"})),
            Error::SessionBusy { session_id, .. } if session_id == "ses_2"
        ));
    }
}
