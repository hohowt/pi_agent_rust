use std::future::Future;
use std::io;
use std::pin::Pin;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Position};
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

#[derive(Debug, Clone)]
pub struct ChatOptions {
    pub title: String,
    pub model_label: String,
    pub status: String,
    pub resource_summary: String,
    pub command_hints: Vec<String>,
    pub key_hints: Vec<String>,
}

impl ChatOptions {
    #[must_use]
    pub fn new(model_label: impl Into<String>) -> Self {
        Self {
            title: "Pi".to_string(),
            model_label: model_label.into(),
            status: "就绪".to_string(),
            resource_summary: "资源: 0 技能, 0 提示, 0 主题".to_string(),
            command_hints: Vec::new(),
            key_hints: vec![
                "Enter: 发送".to_string(),
                "Shift+Enter: newline".to_string(),
                "Ctrl+C/Esc: 退出".to_string(),
                "/help".to_string(),
            ],
        }
    }
}

#[derive(Debug)]
struct ChatApp {
    options: ChatOptions,
    input: String,
    lines: Vec<ChatLine>,
    busy: bool,
    should_quit: bool,
}

impl ChatApp {
    const fn new(options: ChatOptions) -> Self {
        Self {
            options,
            input: String::new(),
            lines: Vec::new(),
            busy: false,
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

pub type SubmitFuture = Pin<Box<dyn Future<Output = anyhow::Result<ChatLine>> + Send>>;

pub async fn run_minimal_chat_loop(
    options: ChatOptions,
    initial_lines: Vec<ChatLine>,
    mut on_submit: impl FnMut(String) -> SubmitFuture + Send,
) -> anyhow::Result<()> {
    let _terminal_guard = TerminalModeGuard::enter()?;
    let _alternate_scroll = AlternateScrollGuard::enable()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let (draw_tx, draw_rx) = broadcast::channel(32);
    let frame_requester = FrameRequester::new(draw_tx);
    let mut events = RatatuiEventStream::new(draw_rx);
    let mut app = ChatApp::new(options);

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
                app.busy = true;
                frame_requester.schedule_frame();
                terminal.draw(|frame| render(frame, &app))?;
                match on_submit(input).await {
                    Ok(line) => app.push_line(line),
                    Err(err) => app.push_line(ChatLine::status(format!("Error: {err}"))),
                }
                app.busy = false;
            }
        }
        frame_requester.schedule_frame();

        terminal.draw(|frame| render(frame, &app))?;
    }

    Ok(())
}

fn render(frame: &mut ratatui::Frame<'_>, app: &ChatApp) {
    let area = frame.area();
    let [body, input, footer] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(area);

    let dim = Style::default().fg(Color::DarkGray);
    let accent = Style::default().fg(Color::Cyan);

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
    let input_cursor_x = input
        .x
        .saturating_add(1)
        .saturating_add(u16::try_from(app.input.chars().count()).unwrap_or(u16::MAX))
        .min(input.right().saturating_sub(1));
    frame.set_cursor_position(Position::new(input_cursor_x, input.y.saturating_add(1)));

    let status = if app.busy {
        "运行中"
    } else {
        app.options.status.as_str()
    };
    frame.render_widget(Paragraph::new(footer_line(app, status)).style(dim), footer);
}

fn footer_line(app: &ChatApp, status: &str) -> String {
    if app.input.trim_start().starts_with('/') {
        let hints = if app.options.command_hints.is_empty() {
            vec![
                "/help".to_string(),
                "/model".to_string(),
                "/thinking".to_string(),
                "/session".to_string(),
                "/tree".to_string(),
                "/codegraph status".to_string(),
            ]
        } else {
            app.options.command_hints.clone()
        };
        return format!("commands: {}", hints.join("  "));
    }
    let hints = if app.options.key_hints.is_empty() {
        String::new()
    } else {
        format!("  |  {}", app.options.key_hints.join("  |  "))
    };
    format!(
        "{}  |  {}  |  模型: {}  |  {}{}",
        app.options.title, status, app.options.model_label, app.options.resource_summary, hints
    )
}
