use std::{env, process::Command};

use reqwest::Url;
use thiserror::Error;

pub const DEFAULT_SERVER: &str = "http://127.0.0.1:4096";

/// Use an explicit server when supplied, otherwise discover the running
/// service and then use the configured fallbacks in order.
pub fn discover_server(server_flag: Option<&str>) -> Result<Url, DiscoveryError> {
    let status = Command::new("opencode")
        .args(["service", "status"])
        .output()
        .map_err(|error| error.to_string())
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).map_err(|error| error.to_string())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
            }
        });

    resolve_server(
        status.as_ref().map(String::as_str).map_err(String::as_str),
        server_flag,
        env::var("OPENCODE_SERVER").ok().as_deref(),
    )
}

fn resolve_server(
    status_output: Result<&str, &str>,
    server_flag: Option<&str>,
    environment: Option<&str>,
) -> Result<Url, DiscoveryError> {
    if let Some(candidate) = server_flag {
        return parse_url(candidate)
            .ok_or_else(|| DiscoveryError::InvalidUrl(candidate.to_owned()));
    }
    if let Ok(output) = status_output
        && let Some(url) = output.lines().find_map(parse_url)
    {
        return Ok(url);
    }

    let candidate = environment.unwrap_or(DEFAULT_SERVER);
    parse_url(candidate).ok_or_else(|| DiscoveryError::InvalidUrl(candidate.to_owned()))
}

fn parse_url(value: &str) -> Option<Url> {
    value.split_whitespace().find_map(|word| {
        Url::parse(word.trim_matches(|c: char| ",;()[]{}<>".contains(c)))
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
    })
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DiscoveryError {
    #[error("invalid OpenCode server URL: {0}")]
    InvalidUrl(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_flag_wins_then_service_status_environment_and_default_follow() {
        let flag = resolve_server(
            Ok("OpenCode service: http://127.0.0.1:49374\n"),
            Some("https://flag.example"),
            Some("https://environment.example"),
        )
        .unwrap();
        assert_eq!(flag.as_str(), "https://flag.example/");

        let discovered = resolve_server(
            Ok("OpenCode service: http://127.0.0.1:49374\n"),
            None,
            Some("https://environment.example"),
        )
        .unwrap();
        assert_eq!(discovered.as_str(), "http://127.0.0.1:49374/");

        let environment = resolve_server(
            Err("not running"),
            None,
            Some("https://environment.example"),
        )
        .unwrap();
        assert_eq!(environment.as_str(), "https://environment.example/");

        let default = resolve_server(Err("not running"), None, None).unwrap();
        assert_eq!(default.as_str(), "http://127.0.0.1:4096/");
    }

    #[test]
    fn invalid_configured_url_is_actionable() {
        assert_eq!(
            resolve_server(Err("not running"), Some("not a URL"), None),
            Err(DiscoveryError::InvalidUrl("not a URL".into()))
        );
    }
}
