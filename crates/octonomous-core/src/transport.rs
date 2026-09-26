use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use thiserror::Error;

use crate::{auth::Credentials, discovery, envelope, generated};

#[derive(Clone, Debug)]
pub struct Client {
    inner: generated::Client,
    event_transport: reqwest::Client,
    base_url: String,
}

impl Client {
    /// Discover the OpenCode service and load its credentials from disk.
    pub fn discover(server_flag: Option<&str>) -> Result<Self, ConnectError> {
        let base_url = discovery::discover_server(server_flag)?;
        let credentials = Credentials::load()?;
        Ok(Self::new(base_url.as_str(), &credentials)?)
    }

    pub fn new(base_url: &str, credentials: &Credentials) -> Result<Self, BuildError> {
        let token = STANDARD.encode(format!("opencode:{}", credentials.password()));
        let authorization = HeaderValue::from_str(&format!("Basic {token}"))?;
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);

        let transport = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(15))
            .default_headers(headers.clone())
            .build()?;
        // An SSE request is intentionally unbounded. Applying the REST
        // transport's whole-request timeout would disconnect a healthy stream
        // every 15 seconds.
        let event_transport = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .default_headers(headers)
            .build()?;
        let base_url = base_url.trim_end_matches('/').to_owned();
        Ok(Self {
            inner: generated::Client::new_with_client(&base_url, transport),
            event_transport,
            base_url,
        })
    }

    pub fn generated(&self) -> &generated::Client {
        &self.inner
    }

    pub(crate) fn event_request(&self) -> reqwest::RequestBuilder {
        self.event_transport
            .get(format!("{}/api/event", self.base_url))
            .header("api-version", "0.0.1")
    }

    pub async fn server_info(&self) -> Result<generated::types::ServerInfo, envelope::Error> {
        self.inner
            .server_info()
            .await
            .map(progenitor_client::ResponseValue::into_inner)
            .map_err(envelope::Error::from_generated)
    }
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error(transparent)]
    Discovery(#[from] discovery::DiscoveryError),
    #[error(transparent)]
    Auth(#[from] crate::auth::AuthError),
    #[error(transparent)]
    Build(#[from] BuildError),
}

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("OpenCode password cannot be represented as an HTTP header: {0}")]
    InvalidHeader(#[from] reqwest::header::InvalidHeaderValue),
    #[error("failed to build HTTP transport: {0}")]
    Transport(#[from] reqwest::Error),
}
