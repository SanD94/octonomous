mod app;
mod cli;

use std::{
    env,
    error::Error,
    io, panic,
    process::{Child, Command as ProcessCommand, Stdio},
    time::Duration,
};

use app::{App, Command, PermissionChoice};
use cli::{Action, Cli, USAGE};
use crossterm::{
    event::{Event as TerminalEvent, EventStream as TerminalEventStream},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use octonomous_core::{
    events::{Event, EventStream, Signal},
    interaction::PermissionReply,
    reconcile::Reconciler,
    session::{Delivery, SessionOptions},
    transport::Client,
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::time;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let options = Cli::parse(env::args().skip(1))?;
    match options.action {
        Action::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        Action::Version => {
            println!("octonomous {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Action::Run => {}
    }

    // `server_process` owns the service this run started, if any. It is held
    // until the end of `main` so the service is stopped when the client exits.
    let (client, server_version, server_process) = connect(options.server.as_deref()).await?;
    let version_warning = version_warning(&server_version);
    if let Some(warning) = &version_warning {
        eprintln!("warning: {warning}");
    }
    if options.check {
        println!(
            "OpenCode {server_version} is reachable{}",
            if server_process.is_some() {
                " (started by octonomous)"
            } else {
                ""
            }
        );
        return Ok(());
    }

    let directory = options
        .directory
        .canonicalize()
        .map_err(|error| format!("invalid working directory {:?}: {error}", options.directory))?
        .to_string_lossy()
        .into_owned();
    let (session_id, directory, resumed) = select_session(&client, &options, directory).await?;
    let event_stream = EventStream::connect(&client, 1_024)?;
    let mut events = event_stream.subscribe();
    let mut reconciler = Reconciler::new(client.clone(), &session_id);
    reconciler.reconcile().await?;

    let mut terminal = TerminalGuard::new()?;
    let mut app = App::new(session_id.clone(), directory);
    app.sync(reconciler.state());
    app.set_status(version_warning.unwrap_or_else(|| {
        if resumed {
            "resumed session".into()
        } else {
            "created session".into()
        }
    }));
    let result = run(
        &mut terminal.terminal,
        &mut app,
        &client,
        &session_id,
        &mut reconciler,
        &mut events,
    )
    .await;
    terminal.restore()?;
    result
}

async fn connect(
    server: Option<&str>,
) -> Result<(Client, String, Option<ServerGuard>), Box<dyn Error>> {
    let initial = Client::discover(server);
    if let Ok(client) = initial
        && let Ok(info) = client.server_info().await
    {
        return Ok((client, info.version, None));
    }
    if server.is_some() {
        return Err(
            "the configured OpenCode server is unavailable; check --server and credentials".into(),
        );
    }

    // The guard keeps ownership of the process: a bare `Child` would be dropped
    // here and the service would keep running after octonomous exits.
    let mut server_process =
        Some(ServerGuard::start().map_err(|error| {
            format!("OpenCode is unavailable and could not be started: {error}")
        })?);

    let mut last_error = "server did not become ready".to_owned();
    for _ in 0..30 {
        time::sleep(Duration::from_millis(200)).await;
        match Client::discover(None) {
            Ok(client) => match client.server_info().await {
                Ok(info) => {
                    return Ok((client, info.version, server_process.take()));
                }
                Err(error) => last_error = error.to_string(),
            },
            Err(error) => last_error = error.to_string(),
        }
    }
    // Dropping `server_process` stops the service that never became ready.
    Err(format!("OpenCode was started but did not become ready: {last_error}").into())
}

/// Owns a service process started by octonomous so that it is stopped when the
/// client exits. `std::process::Child` does not stop the process when dropped,
/// so ownership has to be tracked and released explicitly.
struct ServerGuard {
    child: Child,
}

impl ServerGuard {
    fn start() -> io::Result<Self> {
        let child = ProcessCommand::new("opencode")
            .args(["serve", "--service"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self { child })
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        // A service that already exited, or one that refused the port because
        // another instance won the race, reports an error here. Reap it either
        // way so no zombie is left behind.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn select_session(
    client: &Client,
    options: &Cli,
    directory: String,
) -> Result<(String, String, bool), Box<dyn Error>> {
    if let Some(requested) = &options.session {
        let session = client
            .list_sessions(100)
            .await?
            .into_iter()
            .find(|session| session.id.as_str() == requested)
            .ok_or_else(|| format!("session {requested:?} was not found"))?;
        return Ok((session.id.to_string(), session.location.directory, true));
    }
    if !options.new_session
        && let Some(session) = client.latest_session(&directory).await?
    {
        return Ok((session.id.to_string(), directory, true));
    }

    let session = client
        .create_session_with(
            directory.clone(),
            SessionOptions {
                agent: options.agent.clone(),
                model: options.model.clone(),
            },
        )
        .await?;
    Ok((session.id.to_string(), directory, false))
}

fn version_warning(server_version: &str) -> Option<String> {
    const SUPPORTED_MAJOR: &str = "2";
    let major = server_version.trim_start_matches('v').split('.').next()?;
    (major != SUPPORTED_MAJOR).then(|| {
        format!(
            "server version {server_version} has major version {major}; this client targets OpenCode 2.x"
        )
    })
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    client: &Client,
    session_id: &str,
    reconciler: &mut Reconciler,
    events: &mut tokio::sync::broadcast::Receiver<Signal>,
) -> Result<(), Box<dyn Error>> {
    let mut terminal_events = TerminalEventStream::new();
    let mut tick = time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = tick.tick() => {
                app.advance_tick();
                terminal.draw(|frame| app.render(frame))?;
            }
            event = terminal_events.next() => {
                match event {
                    Some(Ok(TerminalEvent::Key(key))) => {
                        if let Some(command) = app.handle_key(key)
                            && handle_command(command, app, client, session_id, reconciler).await?
                        {
                            break;
                        }
                    }
                    Some(Ok(TerminalEvent::Resize(_, _))) => {
                        terminal.autoresize()?;
                        terminal.draw(|frame| app.render(frame))?;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(error.into()),
                    None => break,
                }
            }
            signal = events.recv() => {
                match signal {
                    Ok(signal) => handle_signal(signal, app, reconciler).await?,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        app.set_status(format!("event lag ({count}); reconciling"));
                        reconciler.reconcile().await?;
                        app.sync(reconciler.state());
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Err("event stream closed".into());
                    }
                }
            }
            result = &mut shutdown => {
                result?;
                break;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() -> io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> io::Result<()> {
    tokio::signal::ctrl_c().await
}

async fn handle_command(
    command: Command,
    app: &mut App,
    client: &Client,
    session_id: &str,
    reconciler: &mut Reconciler,
) -> Result<bool, Box<dyn Error>> {
    match command {
        Command::Send(text) => match client
            .prompt(session_id, text.clone(), Delivery::Steer)
            .await
        {
            Ok(_) => app.set_status("prompt sent"),
            Err(error) => {
                app.reject_pending_user(&text);
                app.set_status(format!("send failed: {error}"));
            }
        },
        Command::Interrupt => match client.interrupt(session_id).await {
            Ok(true) => app.set_status("interrupt requested"),
            Ok(false) => app.set_status("already idle"),
            Err(error) => app.set_status(format!("interrupt failed: {error}")),
        },
        Command::ReplyPermission { request_id, choice } => {
            let decision = match choice {
                PermissionChoice::Once => PermissionReply::Once,
                PermissionChoice::Always => PermissionReply::Always,
                PermissionChoice::Reject => PermissionReply::Reject,
            };
            match client
                .reply_permission(session_id, &request_id, decision)
                .await
            {
                Ok(()) => {
                    reconciler.reconcile_interactions().await?;
                    app.sync(reconciler.state());
                    app.set_status(format!("permission {choice:?}"));
                }
                Err(error) => app.set_status(format!("permission reply failed: {error}")),
            }
        }
        Command::Quit => return Ok(true),
    }
    Ok(false)
}

async fn handle_signal(
    signal: Signal,
    app: &mut App,
    reconciler: &mut Reconciler,
) -> Result<(), Box<dyn Error>> {
    match &signal {
        Signal::Event(envelope) => match &envelope.data {
            Event::SessionTextEnded(_) => app.finish_assistant_message(),
            Event::ServerConnected => app.set_status("connected; reconciled"),
            _ => {}
        },
        Signal::Gap { .. } => app.set_status("event gap detected"),
        Signal::Duplicate { .. } => return Ok(()),
        Signal::Reconnected => app.set_status("reconnected"),
        Signal::ConnectionError(error) => app.set_status(format!("connection: {error}")),
    }
    let update = reconciler.handle(&signal).await?;
    if let Some(delta) = update.reduction.assistant_delta {
        app.push_assistant_delta(&delta);
    }
    if update.reconciliation.is_some() {
        app.finish_assistant_message();
    }
    app.sync(reconciler.state());
    Ok(())
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    restored: bool,
}

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        install_panic_restore();
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = restore_terminal();
                return Err(error);
            }
        };
        Ok(Self {
            terminal,
            restored: false,
        })
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.restored {
            restore_terminal()?;
            self.terminal.show_cursor()?;
            self.restored = true;
        }
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)
}

fn install_panic_restore() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::{ProcessCommand, ServerGuard, version_warning};

    #[test]
    fn warns_only_when_server_major_differs() {
        assert!(version_warning("2.0.18").is_none());
        assert!(version_warning("v2.9.0").is_none());
        assert_eq!(
            version_warning("3.0.0").as_deref(),
            Some("server version 3.0.0 has major version 3; this client targets OpenCode 2.x")
        );
    }

    #[cfg(unix)]
    fn process_is_running(pid: u32) -> bool {
        ProcessCommand::new("ps")
            .args(["-p", &pid.to_string()])
            .output()
            .is_ok_and(|output| output.status.success())
    }

    #[cfg(unix)]
    #[test]
    fn server_guard_stops_the_process_it_owns() {
        // Control case: an unowned `Child` keeps running once dropped, so this
        // test would be vacuous if the guard had nothing to do.
        let unowned = ProcessCommand::new("sleep").arg("60").spawn().unwrap();
        let unowned_pid = unowned.id();
        drop(unowned);
        assert!(
            process_is_running(unowned_pid),
            "an unowned child should outlive its handle"
        );
        let _ = ProcessCommand::new("kill")
            .arg(unowned_pid.to_string())
            .status();

        let guard = ServerGuard {
            child: ProcessCommand::new("sleep").arg("60").spawn().unwrap(),
        };
        let guarded_pid = guard.child.id();
        assert!(process_is_running(guarded_pid));
        drop(guard);
        assert!(
            !process_is_running(guarded_pid),
            "ServerGuard must stop the process it owns"
        );
    }
}
