mod app;

use std::{env, error::Error, io, panic, time::Duration};

use app::{App, Command, PermissionChoice};
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
    session::Delivery,
    transport::Client,
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::time;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let directory = args
        .next()
        .ok_or("usage: octonomous-tui DIRECTORY [SERVER_URL]")?;
    let server = args.next();
    if args.next().is_some() {
        return Err("usage: octonomous-tui DIRECTORY [SERVER_URL]".into());
    }

    let client = Client::discover(server.as_deref())?;
    let event_stream = EventStream::connect(&client, 1_024)?;
    let mut events = event_stream.subscribe();
    let session = client.create_session(directory.clone()).await?;
    let session_id = session.id.to_string();
    let mut reconciler = Reconciler::new(client.clone(), &session_id);
    reconciler.reconcile().await?;

    let mut terminal = TerminalGuard::new()?;
    let mut app = App::new(session_id.clone(), directory);
    app.sync(reconciler.state());
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
