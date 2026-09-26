use std::{env, fmt, fs, path::PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Authentication material loaded from OpenCode's service configuration.
///
/// The password is deliberately private and redacted from debug output.
pub struct Credentials {
    password: String,
}

impl Credentials {
    pub fn load() -> Result<Self, AuthError> {
        let path = config_path()?;
        Self::load_from(path)
    }

    pub fn from_password(password: impl Into<String>) -> Result<Self, AuthError> {
        let password = password.into();
        if password.is_empty() {
            return Err(AuthError::EmptyPassword);
        }
        Ok(Self { password })
    }

    fn load_from(path: PathBuf) -> Result<Self, AuthError> {
        let contents = fs::read_to_string(&path).map_err(|source| AuthError::Read {
            path: path.clone(),
            source,
        })?;
        let config: ServiceConfig =
            serde_json::from_str(&contents).map_err(|source| AuthError::Invalid {
                path: path.clone(),
                source,
            })?;
        let password = config
            .password
            .filter(|password| !password.is_empty())
            .ok_or(AuthError::MissingPassword { path: path.clone() })?;
        Ok(Self { password })
    }

    pub(crate) fn password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("password", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("neither XDG_CONFIG_HOME nor HOME is set; cannot locate OpenCode credentials")]
    ConfigHomeUnavailable,
    #[error("OpenCode password cannot be empty")]
    EmptyPassword,
    #[error("failed to read OpenCode credentials at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid OpenCode credentials file at {path}: {source}")]
    Invalid {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("OpenCode credentials at {path} do not contain a non-empty `password` key")]
    MissingPassword { path: PathBuf },
}

#[derive(Deserialize)]
struct ServiceConfig {
    password: Option<String>,
}

fn config_path() -> Result<PathBuf, AuthError> {
    config_path_from(env::var_os("XDG_CONFIG_HOME"), env::var_os("HOME"))
}

fn config_path_from(
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf, AuthError> {
    if let Some(path) = xdg_config_home {
        return Ok(PathBuf::from(path).join("opencode/service.json"));
    }
    home.map(PathBuf::from)
        .map(|path| path.join(".config/opencode/service.json"))
        .ok_or(AuthError::ConfigHomeUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_debug_output_redacts_password() {
        let debug = format!("{:?}", Credentials::from_password("super-secret").unwrap());
        assert_eq!(debug, "Credentials { password: \"[REDACTED]\" }");
        assert!(!debug.contains("super-secret"));
    }

    #[test]
    fn xdg_config_home_takes_precedence() {
        let path = config_path_from(Some("/xdg".into()), Some("/home/user".into())).unwrap();
        assert_eq!(path, PathBuf::from("/xdg/opencode/service.json"));
    }

    #[test]
    fn missing_password_names_the_file_and_key() {
        let path = std::env::temp_dir().join(format!(
            "octonomous-auth-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, "{}").unwrap();
        let error = Credentials::load_from(path.clone()).unwrap_err();
        fs::remove_file(&path).unwrap();

        let message = error.to_string();
        assert!(message.contains(path.to_str().unwrap()));
        assert!(message.contains("password"));
    }
}
