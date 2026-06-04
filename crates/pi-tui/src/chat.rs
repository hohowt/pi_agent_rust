use std::io;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;

use crate::event::{RatatuiEvent, RatatuiEventStream};
use crate::frame::FrameRequester;
use crate::terminal::{AlternateScrollGuard, TerminalModeGuard};

#[derive(Debug, Clone)]
pub struct ChatLine {
    role: &'static str,
    text: String,
}

impl ChatLine {
    #[must_use]
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: "You",
            text: text.into(),
        }
    }

    #[must_use]
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: "Assistant",
            text: text.into(),
        }
    }

    #[must_use]
    pub fn status(text: impl Into<String>) -> Self {
        Self {
            role: "Status",
            text: text.into(),
        }
    }
}

#[derive(Debug)]
struct ChatApp {
    model_label: String,
    input: String,
    lines: Vec<ChatLine>,
    should_quit: bool,
}

impl ChatApp {
    fn new(model_label: impl Into<String>) -> Self {
        Self {
            model_label: model_label.into(),
            input: String::new(),
            lines: Vec::new(),
            should_quit: false,
        }
    }

    fn take_submitted_input(&mut self) -> Option<String> {
        let input = self.input.trim().to_string();
        if input.is_empty() {
            return None;
        }
        self.input.clear();
        self.lines.push(ChatLine::user(input.clone()));
        Some(input)
    }

    fn push_line(&mut self, line: ChatLine) {
        self.lines.push(line);
    }

    fn handle_event(&mut self, event: RatatuiEvent) {
        match event {
            RatatuiEvent::Key(key) => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.should_quit = true;
                }
                KeyCode::Esc => {
                    self.should_quit = true;
                }
                KeyCode::Char(ch) => {
                    self.input.push(ch);
                }
                KeyCode::Backspace => {
                    self.input.pop();
                }
                _ => {}
            },
            RatatuiEvent::Paste(text) => self.input.push_str(&text),
            RatatuiEvent::Resize | RatatuiEvent::Draw | RatatuiEvent::FocusGained => {}
        }
    }

    const fn should_quit(&self) -> bool {
        self.should_quit
    }
}

pub async fn run_minimal_chat_loop(
    model_label: String,
    initial_lines: Vec<ChatLine>,
) -> anyhow::Result<()> {
    let _terminal_guard = TerminalModeGuard::enter()?;
    let _alternate_scroll = AlternateScrollGuard::enable()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let (draw_tx, draw_rx) = broadcast::channel(32);
    let frame_requester = FrameRequester::new(draw_tx);
    let mut events = RatatuiEventStream::new(draw_rx);
    let mut app = ChatApp::new(model_label);

    for line in initial_lines {
        app.push_line(line);
    }
    frame_requester.schedule_frame();

    terminal.draw(|frame| render(frame, &app))?;
    loop {
        if app.should_quit() {
            break;
        }

        let Some(event) = events.next().await else {
            break;
        };

        let submit = matches!(
            event,
            RatatuiEvent::Key(crossterm::event::KeyEvent {
                code: KeyCode::Enter,
                ..
            })
        );
        app.handle_event(event);
        if submit {
            if let Some(input) = app.take_submitted_input() {
                frame_requester.schedule_frame();
                app.push_line(ChatLine::status(format!(
                    "Interactive submit is pending full ratatui chat port: {input}"
                )));
            }
        }
        frame_requester.schedule_frame();

        terminal.draw(|frame| render(frame, &app))?;
    }

    Ok(())
}

fn render(frame: &mut ratatui::Frame<'_>, app: &ChatApp) {
    let area = frame.area();
    let [header, body, input, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(area);

    let dim = Style::default().fg(Color::DarkGray);
    let accent = Style::default().fg(Color::Cyan);
    let title = Style::default().add_modifier(Modifier::BOLD);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Pi", title),
            Span::raw("  "),
            Span::styled(app.model_label.as_str(), dim),
        ])),
        header,
    );

    let mut body_lines = Vec::new();
    for line in &app.lines {
        body_lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", line.role),
                match line.role {
                    "Assistant" => Style::default().fg(Color::Green),
                    "Status" => dim,
                    _ => accent,
                }
                .add_modifier(Modifier::BOLD),
            ),
            Span::raw(line.text.as_str()),
        ]));
    }
    frame.render_widget(
        Paragraph::new(body_lines)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(dim)
                    .border_type(BorderType::Plain),
            )
            .wrap(Wrap { trim: false }),
        body,
    );

    frame.render_widget(
        Paragraph::new(app.input.as_str()).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(dim)
                .title(Span::styled(" 输入 ", accent)),
        ),
        input,
    );

    frame.render_widget(
        Paragraph::new("Enter: 发送  Esc/Ctrl+C: 退出").style(dim),
        footer,
    );
}
