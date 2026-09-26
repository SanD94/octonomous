use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use octonomous_core::reconcile::{PendingPermission, Role, SessionState, TranscriptMessage};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionChoice {
    Once,
    Always,
    Reject,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Send(String),
    Interrupt,
    ReplyPermission {
        request_id: String,
        choice: PermissionChoice,
    },
    Quit,
}

pub struct App {
    session_id: String,
    directory: String,
    transcript: Vec<TranscriptMessage>,
    pending_user: Vec<String>,
    assistant_draft: String,
    permissions: Vec<PendingPermission>,
    composer: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    history_scratch: String,
    running: bool,
    status: String,
    follow: bool,
    scroll: u16,
    max_scroll: u16,
    tick: usize,
}

impl App {
    pub fn new(session_id: String, directory: String) -> Self {
        Self {
            session_id,
            directory,
            transcript: Vec::new(),
            pending_user: Vec::new(),
            assistant_draft: String::new(),
            permissions: Vec::new(),
            composer: Vec::new(),
            cursor: 0,
            history: Vec::new(),
            history_index: None,
            history_scratch: String::new(),
            running: false,
            status: "connected".into(),
            follow: true,
            scroll: 0,
            max_scroll: 0,
            tick: 0,
        }
    }

    pub fn sync(&mut self, state: &SessionState) {
        for message in &state.transcript {
            if message.role == Role::User
                && !self
                    .transcript
                    .iter()
                    .any(|current| current.id == message.id)
                && let Some(index) = self
                    .pending_user
                    .iter()
                    .position(|text| text == &message.text)
            {
                self.pending_user.remove(index);
            }
        }
        self.transcript = state.transcript.clone();
        self.running = state.running;
        self.permissions = state.pending_permissions.clone();
    }

    pub fn push_assistant_delta(&mut self, delta: &str) {
        self.assistant_draft.push_str(delta);
        self.follow = true;
    }

    pub fn finish_assistant_message(&mut self) {
        self.assistant_draft.clear();
    }

    pub fn reject_pending_user(&mut self, text: &str) {
        if let Some(index) = self
            .pending_user
            .iter()
            .rposition(|pending| pending == text)
        {
            self.pending_user.remove(index);
        }
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub fn advance_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Command> {
        if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
            return None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Command::Quit);
        }

        if let Some(permission) = self.permissions.first() {
            let choice = match key.code {
                KeyCode::Char('1') | KeyCode::Char('o') => PermissionChoice::Once,
                KeyCode::Char('a') => PermissionChoice::Always,
                KeyCode::Char('r') => PermissionChoice::Reject,
                _ => return None,
            };
            return Some(Command::ReplyPermission {
                request_id: permission.id.clone(),
                choice,
            });
        }

        match key.code {
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
            {
                self.insert('\n');
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert('\n');
            }
            KeyCode::Enter => return self.submit(),
            KeyCode::Esc if self.running => return Some(Command::Interrupt),
            KeyCode::Esc => self.clear_composer(),
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert(character);
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.composer.remove(self.cursor);
                self.leave_history();
            }
            KeyCode::Delete if self.cursor < self.composer.len() => {
                self.composer.remove(self.cursor);
                self.leave_history();
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.composer.len()),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Up => self.history_up(),
            KeyCode::Down => self.history_down(),
            KeyCode::PageUp => {
                self.follow = false;
                self.scroll = self.scroll.saturating_sub(5);
            }
            KeyCode::PageDown => {
                self.scroll = (self.scroll + 5).min(self.max_scroll);
                self.follow = self.scroll == self.max_scroll;
            }
            _ => {}
        }
        None
    }

    fn insert(&mut self, character: char) {
        self.composer.insert(self.cursor, character);
        self.cursor += 1;
        self.leave_history();
    }

    fn submit(&mut self) -> Option<Command> {
        let text: String = self.composer.iter().collect();
        if text.trim().is_empty() {
            return None;
        }
        self.history.push(text.clone());
        self.pending_user.push(text.clone());
        self.clear_composer();
        self.follow = true;
        Some(Command::Send(text))
    }

    fn clear_composer(&mut self) {
        self.composer.clear();
        self.cursor = 0;
        self.history_index = None;
        self.history_scratch.clear();
    }

    fn leave_history(&mut self) {
        self.history_index = None;
        self.history_scratch.clear();
    }

    fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let index = match self.history_index {
            Some(index) => index.saturating_sub(1),
            None => {
                self.history_scratch = self.composer.iter().collect();
                self.history.len() - 1
            }
        };
        self.set_history(index);
    }

    fn history_down(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            self.set_history(index + 1);
        } else {
            self.composer = self.history_scratch.chars().collect();
            self.cursor = self.composer.len();
            self.history_index = None;
        }
    }

    fn set_history(&mut self, index: usize) {
        self.composer = self.history[index].chars().collect();
        self.cursor = self.composer.len();
        self.history_index = Some(index);
    }

    fn line_start(&self) -> usize {
        self.composer[..self.cursor]
            .iter()
            .rposition(|character| *character == '\n')
            .map_or(0, |index| index + 1)
    }

    fn line_end(&self) -> usize {
        self.composer[self.cursor..]
            .iter()
            .position(|character| *character == '\n')
            .map_or(self.composer.len(), |index| self.cursor + index)
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(4),
                Constraint::Length(5),
                Constraint::Length(1),
            ])
            .split(area);
        self.render_transcript(frame, chunks[0]);
        self.render_composer(frame, chunks[1]);
        self.render_status(frame, chunks[2]);
        if let Some(permission) = self.permissions.first() {
            render_permission(frame, area, permission);
        }
    }

    fn render_transcript(&mut self, frame: &mut Frame, area: Rect) {
        let mut lines = Vec::new();
        for message in &self.transcript {
            let (label, color) = match &message.role {
                Role::User => ("You", Color::Cyan),
                Role::Assistant => ("Assistant", Color::Green),
                Role::Other(role) => (role.as_str(), Color::Yellow),
            };
            append_message(&mut lines, label, color, &message.text);
        }
        for message in &self.pending_user {
            append_message(&mut lines, "You", Color::Cyan, message);
        }
        if !self.assistant_draft.is_empty() {
            append_message(&mut lines, "Assistant", Color::Green, &self.assistant_draft);
        }

        let inner_width = area.width.saturating_sub(2).max(1) as usize;
        let visual_lines: usize = lines
            .iter()
            .map(|line| line.width().max(1).div_ceil(inner_width))
            .sum();
        let viewport = area.height.saturating_sub(2) as usize;
        self.max_scroll = visual_lines.saturating_sub(viewport).min(u16::MAX as usize) as u16;
        if self.follow {
            self.scroll = self.max_scroll;
        } else {
            self.scroll = self.scroll.min(self.max_scroll);
        }

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(Block::default().borders(Borders::ALL).title(" Transcript "))
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            area,
        );
    }

    fn render_composer(&self, frame: &mut Frame, area: Rect) {
        let text: String = self.composer.iter().collect();
        frame.render_widget(
            Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Message · Enter send · Shift+Enter/Ctrl+J newline "),
                )
                .wrap(Wrap { trim: false }),
            area,
        );
        if self.permissions.is_empty() {
            let before: String = self.composer[..self.cursor].iter().collect();
            let row = before
                .chars()
                .filter(|character| *character == '\n')
                .count() as u16;
            let column = before
                .rsplit('\n')
                .next()
                .map(str::chars)
                .map(Iterator::count)
                .unwrap_or(0) as u16;
            frame.set_cursor_position((
                area.x + 1 + column.min(area.width.saturating_sub(3)),
                area.y + 1 + row.min(area.height.saturating_sub(3)),
            ));
        }
    }

    fn render_status(&self, frame: &mut Frame, area: Rect) {
        let activity = if self.running {
            const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            format!(
                "{} running · Esc interrupt",
                SPINNER[self.tick % SPINNER.len()]
            )
        } else {
            "idle · Esc clear · Ctrl+C quit".into()
        };
        let line = Line::from(vec![
            Span::styled(activity, Style::default().fg(Color::Green)),
            Span::raw(format!(
                "  {}  {}  {}",
                self.session_id, self.directory, self.status
            )),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }
}

fn append_message<'a>(lines: &mut Vec<Line<'a>>, label: &'a str, color: Color, text: &'a str) {
    lines.push(Line::from(Span::styled(
        label,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )));
    if text.is_empty() {
        lines.push(Line::default());
    } else {
        lines.extend(text.lines().map(Line::raw));
    }
    lines.push(Line::default());
}

fn render_permission(frame: &mut Frame, area: Rect, permission: &PendingPermission) {
    let popup = centered_rect(72, 12, area);
    let mut lines = vec![
        Line::styled(
            "Interaction is blocked until this request is answered.",
            Style::default().fg(Color::Yellow),
        ),
        Line::default(),
        Line::from(vec![
            Span::styled("Action: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&permission.action),
        ]),
    ];
    if !permission.resources.is_empty() {
        lines.push(Line::from(format!(
            "Resources: {}",
            permission.resources.join(", ")
        )));
    }
    if let Some(message) = &permission.message {
        lines.push(Line::default());
        lines.push(Line::raw(message));
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        "[1/o] once    [a] always    [r] reject",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Permission required ")
                    .style(Style::default().bg(Color::Black)),
            ),
        popup,
    );
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use ratatui::{Terminal, backend::TestBackend};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn composer_supports_multiline_history_and_submission() {
        let mut app = App::new("ses_1".into(), "/project".into());
        app.handle_key(key(KeyCode::Char('h')));
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('i')));

        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Command::Send("h\ni".into()))
        );
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.composer.iter().collect::<String>(), "h\ni");
        app.handle_key(key(KeyCode::Down));
        assert!(app.composer.is_empty());
    }

    #[test]
    fn permission_modal_blocks_composer_until_answered() {
        let mut app = App::new("ses_1".into(), "/project".into());
        app.permissions.push(PendingPermission {
            id: "per_1".into(),
            action: "bash".into(),
            resources: vec!["cargo test".into()],
            message: Some("Run tests?".into()),
        });

        assert_eq!(app.handle_key(key(KeyCode::Char('x'))), None);
        assert!(app.composer.is_empty());
        assert_eq!(
            app.handle_key(key(KeyCode::Char('a'))),
            Some(Command::ReplyPermission {
                request_id: "per_1".into(),
                choice: PermissionChoice::Always,
            })
        );
    }

    #[test]
    fn reconciliation_settles_each_repeated_prompt_only_once() {
        let mut app = App::new("ses_1".into(), "/project".into());
        app.pending_user = vec!["same".into(), "same".into()];
        let mut state = SessionState::default();
        state.transcript.push(TranscriptMessage {
            id: "msg_1".into(),
            role: Role::User,
            text: "same".into(),
        });

        app.sync(&state);
        app.sync(&state);
        assert_eq!(app.pending_user, vec!["same"]);

        state.transcript.push(TranscriptMessage {
            id: "msg_2".into(),
            role: Role::User,
            text: "same".into(),
        });
        app.sync(&state);
        assert!(app.pending_user.is_empty());
    }

    #[test]
    fn render_reflows_and_shows_permission_details() {
        let mut app = App::new("ses_1".into(), "/project".into());
        app.transcript.push(TranscriptMessage {
            id: "msg_1".into(),
            role: Role::Assistant,
            text: "A response that wraps naturally when the terminal width changes.".into(),
        });

        let mut wide = Terminal::new(TestBackend::new(80, 24)).unwrap();
        wide.draw(|frame| app.render(frame)).unwrap();
        let wide_display = wide.backend().to_string();
        assert!(
            wide_display
                .contains("A response that wraps naturally when the terminal width changes.")
        );

        app.permissions.push(PendingPermission {
            id: "per_1".into(),
            action: "bash".into(),
            resources: vec!["cargo test".into()],
            message: Some("Run the test suite?".into()),
        });

        let mut narrow = Terminal::new(TestBackend::new(52, 18)).unwrap();
        narrow.draw(|frame| app.render(frame)).unwrap();
        let display = narrow.backend().to_string();

        assert!(display.contains("Permission required"));
        assert!(display.contains("Run the test suite?"));
        assert!(display.contains("[1/o] once"));
    }
}
