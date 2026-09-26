use std::{env, path::PathBuf};

use octonomous_core::session::ModelSelection;

pub const USAGE: &str = r#"octonomous [OPTIONS] [DIRECTORY]

Native OpenCode terminal client. Existing sessions in DIRECTORY resume by default.

Options:
  --new                    Always create a new session
  --session ID             Resume a specific session
  --agent ID               Agent for a newly created session
  --model PROVIDER/ID[@VARIANT]
                           Model for a newly created session
  --server URL             Use an explicit OpenCode server URL
  --check                  Check connectivity and exit
  -h, --help               Show this help
  -V, --version            Show the client version"#;

#[derive(Debug, PartialEq, Eq)]
pub struct Cli {
    pub directory: PathBuf,
    pub server: Option<String>,
    pub session: Option<String>,
    pub agent: Option<String>,
    pub model: Option<ModelSelection>,
    pub new_session: bool,
    pub check: bool,
    pub action: Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Run,
    Help,
    Version,
}

impl Cli {
    pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut arguments = arguments.into_iter();
        let mut directory = None;
        let mut server = None;
        let mut session = None;
        let mut agent = None;
        let mut model = None;
        let mut new_session = false;
        let mut check = false;
        let mut action = Action::Run;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--new" => new_session = true,
                "--check" => check = true,
                "-h" | "--help" => action = Action::Help,
                "-V" | "--version" => action = Action::Version,
                "--server" => server = Some(value(&mut arguments, "--server")?),
                "--session" => session = Some(value(&mut arguments, "--session")?),
                "--agent" => agent = Some(value(&mut arguments, "--agent")?),
                "--model" => model = Some(parse_model(&value(&mut arguments, "--model")?)?),
                option if option.starts_with('-') => {
                    return Err(format!("unknown option {option:?}\n\n{USAGE}"));
                }
                path if directory.is_none() => directory = Some(PathBuf::from(path)),
                path => return Err(format!("unexpected second directory {path:?}\n\n{USAGE}")),
            }
        }

        if new_session && session.is_some() {
            return Err("--new and --session cannot be used together".into());
        }
        if session.is_some() && (agent.is_some() || model.is_some()) {
            return Err("--agent and --model apply only to new sessions".into());
        }

        Ok(Self {
            directory: directory.unwrap_or(env::current_dir().map_err(|error| error.to_string())?),
            server,
            session,
            agent,
            model,
            new_session,
            check,
            action,
        })
    }
}

fn value(arguments: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    arguments
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_model(value: &str) -> Result<ModelSelection, String> {
    let (provider_id, model) = value
        .split_once('/')
        .ok_or_else(|| "--model must use PROVIDER/ID[@VARIANT]".to_owned())?;
    let (id, variant) = match model.rsplit_once('@') {
        Some((id, variant)) => (id, Some(variant.to_owned())),
        None => (model, None),
    };
    if provider_id.is_empty()
        || id.is_empty()
        || variant.as_ref().is_some_and(|variant| variant.is_empty())
    {
        return Err("--model must use PROVIDER/ID[@VARIANT]".into());
    }
    Ok(ModelSelection {
        id: id.to_owned(),
        provider_id: provider_id.to_owned(),
        variant,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Cli, String> {
        Cli::parse(arguments.iter().map(|argument| (*argument).to_owned()))
    }

    #[test]
    fn parses_new_session_selections() {
        let cli = parse(&[
            "--new",
            "--agent",
            "build",
            "--model",
            "anthropic/claude-opus@high",
            "/project",
        ])
        .unwrap();

        assert!(cli.new_session);
        assert_eq!(cli.agent.as_deref(), Some("build"));
        assert_eq!(
            cli.model,
            Some(ModelSelection {
                id: "claude-opus".into(),
                provider_id: "anthropic".into(),
                variant: Some("high".into()),
            })
        );
        assert_eq!(cli.directory, PathBuf::from("/project"));
    }

    #[test]
    fn rejects_conflicting_resume_options_and_bad_models() {
        assert!(parse(&["--new", "--session", "ses_1"]).is_err());
        assert!(parse(&["--session", "ses_1", "--agent", "build"]).is_err());
        assert!(parse(&["--model", "missing-provider"]).is_err());
        assert!(parse(&["--model", "provider/model@"]).is_err());
    }
}
